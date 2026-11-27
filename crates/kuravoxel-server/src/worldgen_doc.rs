//! Edición de los archivos del terreno (`worldgen/world.toml` y los biomas) para una
//! herramienta: un editor de nodos, o cualquier otra.
//!
//! Cada archivo se edita sobre su propio texto (`toml_edit`): un cambio toca solo el valor
//! que cambia, y los comentarios, el orden y el formato del resto se conservan. Dónde se
//! dibuja cada nodo en un lienzo se guarda en el mismo archivo, en `[editor.layout]`, que
//! el generador ignora.
//!
//! Los valores entran y salen como `toml::Value`: quien edita no necesita saber de
//! `toml_edit`. Qué parámetros tiene cada tipo de nodo lo dice `graph::NODE_TYPES`.
//!
//! Un bioma puede tener **perfiles**: otras versiones enteras de su archivo, en
//! `worldgen/profiles/<bioma>/<perfil>.toml`. Cada mundo elige cuál usa (`Workspace::active`);
//! el archivo de `biomes/` es el perfil por defecto. Un documento se nombra por su clave:
//! `world`, el identificador del bioma (`base:plains`) o el bioma y el perfil
//! (`base:plains@rivers`).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Key, Table, TableLike, Value};

use std::collections::BTreeMap;

use crate::biomes::{profile_path, valid_profile_name, WorldSource, FOLDER, PROFILES};
use crate::graph::{self, ParamKind, BUILTINS};
use kuravoxel_common::content;

/// Las listas de reglas de un bioma.
pub const SECTIONS: [&str; 3] = ["surface", "trees", "plants"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    World,
    Biome,
}

/// Un archivo del terreno abierto para editarlo.
#[derive(Clone, Debug)]
pub struct GenDoc {
    /// `world` o el identificador del bioma (`base:plains`).
    pub id: String,
    pub kind: DocKind,
    pub path: PathBuf,
    /// Perfil del bioma que es este archivo (`None` = el archivo por defecto).
    pub profile: Option<String>,
    doc: DocumentMut,
    /// Texto tal como está en disco.
    saved: String,
}

/// Un nombre de nodo válido: minúsculas, números y `_`, sin empezar por número.
pub fn valid_node_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !BUILTINS.contains(&name)
}

/// Un número tal como se escribe: entero si lo es, y sin los decimales de sobra de un
/// `f32` (`0.3`, no `0.30000001192092896`).
pub fn number(value: f64) -> toml::Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        return toml::Value::Integer(value as i64);
    }
    let short: f64 = format!("{}", value as f32).parse().unwrap_or(value);
    toml::Value::Float(if (short - value).abs() <= 1e-6 * value.abs().max(1.0) { short } else { value })
}

fn to_edit(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(text) => Value::from(text.as_str()),
        toml::Value::Integer(number) => Value::from(*number),
        toml::Value::Float(float) => match number(*float) {
            toml::Value::Integer(integer) => Value::from(integer),
            toml::Value::Float(float) => Value::from(float),
            _ => unreachable!(),
        },
        toml::Value::Boolean(flag) => Value::from(*flag),
        toml::Value::Datetime(date) => Value::from(date.to_string().as_str()),
        toml::Value::Array(items) => {
            let mut array = Array::new();
            for item in items {
                array.push(to_edit(item));
            }
            Value::Array(array)
        }
        toml::Value::Table(table) => {
            let mut inline = InlineTable::new();
            for (key, item) in table {
                inline.insert(key, to_edit(item));
            }
            Value::InlineTable(inline)
        }
    }
}

fn to_plain(value: &Value) -> toml::Value {
    match value {
        Value::String(text) => toml::Value::String(text.value().clone()),
        Value::Integer(number) => toml::Value::Integer(*number.value()),
        Value::Float(float) => toml::Value::Float(*float.value()),
        Value::Boolean(flag) => toml::Value::Boolean(*flag.value()),
        Value::Datetime(date) => toml::Value::String(date.value().to_string()),
        Value::Array(array) => toml::Value::Array(array.iter().map(to_plain).collect()),
        Value::InlineTable(table) => toml::Value::Table(plain_table(table)),
    }
}

fn plain_table(table: &dyn TableLike) -> toml::Table {
    let mut out = toml::Table::new();
    for (key, item) in table.iter() {
        if let Some(value) = item_to_plain(item) {
            out.insert(key.to_string(), value);
        }
    }
    out
}

fn item_to_plain(item: &Item) -> Option<toml::Value> {
    match item {
        Item::None => None,
        Item::Value(value) => Some(to_plain(value)),
        Item::Table(table) => Some(toml::Value::Table(plain_table(table))),
        Item::ArrayOfTables(tables) => {
            Some(toml::Value::Array(tables.iter().map(|t| toml::Value::Table(plain_table(t))).collect()))
        }
    }
}

/// Cambia (o quita, con `None`) un valor de una tabla sin tocar el resto: la clave conserva
/// su comentario y el valor nuevo hereda el formato del anterior.
fn put(table: &mut dyn TableLike, key: &str, value: Option<&toml::Value>) {
    let Some(value) = value else {
        table.remove(key);
        return;
    };
    let mut new = to_edit(value);
    match table.get_mut(key) {
        Some(item) => {
            if let Some(old) = item.as_value() {
                *new.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(new);
        }
        None => {
            table.insert(key, Item::Value(new));
        }
    }
}

/// Nombre de un tipo de nodo en el archivo, sin el `type`.
fn kind_of(node: &toml::Table) -> Option<&str> {
    node.get("type")?.as_str()
}

/// ¿Esta entrada de un nodo es una referencia a `name`?
fn refers(value: &toml::Value, name: &str) -> bool {
    value.as_str() == Some(name)
}

impl GenDoc {
    pub fn parse(id: &str, kind: DocKind, path: PathBuf, text: &str) -> anyhow::Result<Self> {
        let doc: DocumentMut = text.parse().with_context(|| format!("error in {}", path.display()))?;
        Ok(Self { id: id.to_string(), kind, path, profile: None, doc, saved: text.to_string() })
    }

    /// Clave del documento: `world`, `base:plains` o `base:plains@perfil`.
    pub fn key(&self) -> String {
        match &self.profile {
            Some(profile) => format!("{}@{profile}", self.id),
            None => self.id.clone(),
        }
    }

    pub fn open(id: &str, kind: DocKind, path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
        Self::parse(id, kind, path.to_path_buf(), &text)
    }

    pub fn text(&self) -> String {
        self.doc.to_string()
    }

    /// Tiene cambios sin guardar.
    pub fn dirty(&self) -> bool {
        self.text() != self.saved
    }

    /// Escribe el archivo (a un temporal y renombrando).
    pub fn save(&mut self) -> anyhow::Result<()> {
        let text = self.text();
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = self.path.with_extension("toml.tmp");
        std::fs::write(&temp, &text).with_context(|| format!("failed to write {}", temp.display()))?;
        std::fs::rename(&temp, &self.path).with_context(|| format!("failed to write {}", self.path.display()))?;
        self.saved = text;
        Ok(())
    }

    /// Nombre visible: el `name` de un bioma o su identificador.
    pub fn title(&self) -> String {
        match self.kind {
            DocKind::World => "World".into(),
            DocKind::Biome => self.get("name").and_then(|name| name.as_str().map(str::to_string)).unwrap_or(self.id.clone()),
        }
    }

    // --- Campos sueltos (`name`, `color`, `blend`, `biomes`...) ---

    pub fn get(&self, key: &str) -> Option<toml::Value> {
        item_to_plain(self.doc.get(key)?)
    }

    pub fn set(&mut self, key: &str, value: Option<toml::Value>) {
        put(self.doc.as_table_mut(), key, value.as_ref());
    }

    // --- Nodos ---

    fn nodes(&self) -> Option<&dyn TableLike> {
        self.doc.get("nodes")?.as_table_like()
    }

    fn nodes_mut(&mut self) -> &mut dyn TableLike {
        if self.doc.get("nodes").and_then(Item::as_table_like).is_none() {
            // Delante de las reglas, que es donde se espera leerlo.
            let mut table = Table::new();
            shift_positions(self.doc.as_table_mut(), 0);
            table.set_position(0);
            self.doc.insert("nodes", Item::Table(table));
        }
        self.doc.get_mut("nodes").and_then(Item::as_table_like_mut).expect("nodes is a table")
    }

    /// Nombres de los nodos, en el orden del archivo.
    pub fn node_names(&self) -> Vec<String> {
        self.nodes().map_or_else(Vec::new, |nodes| nodes.iter().map(|(name, _)| name.to_string()).collect())
    }

    pub fn has_node(&self, name: &str) -> bool {
        self.nodes().is_some_and(|nodes| nodes.contains_key(name))
    }

    pub fn node(&self, name: &str) -> Option<toml::Table> {
        match item_to_plain(self.nodes()?.get(name)?)? {
            toml::Value::Table(table) => Some(table),
            _ => None,
        }
    }

    /// Cambia (o quita) un parámetro de un nodo.
    pub fn set_param(&mut self, node: &str, key: &str, value: Option<toml::Value>) {
        let Some(item) = self.nodes_mut().get_mut(node) else { return };
        // Un nodo en una línea (`{ ... }`) no lleva comentarios dentro: se vuelve a
        // formatear entero, que si no las comas quedan descolocadas.
        if let Some(inline) = item.as_inline_table_mut() {
            put(inline, key, value.as_ref());
            inline.fmt();
        } else if let Some(table) = item.as_table_like_mut() {
            put(table, key, value.as_ref());
        }
    }

    /// Cambia un nodo entero (por ejemplo, de tipo). Conserva el comentario de su línea.
    pub fn replace_node(&mut self, name: &str, node: &toml::Table) {
        put(self.nodes_mut(), name, Some(&toml::Value::Table(node.clone())));
    }

    /// Un nombre libre a partir de `base`: `base`, `base_2`, `base_3`...
    pub fn free_name(&self, base: &str, taken: impl Fn(&str) -> bool) -> String {
        let base: String = base.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect::<String>().to_lowercase();
        let base = if valid_node_name(&base) { base } else { "node".into() };
        let free = |name: &str| !self.has_node(name) && !taken(name) && valid_node_name(name);
        if free(&base) {
            return base;
        }
        (2..).map(|n| format!("{base}_{n}")).find(|name| free(name)).expect("some name is free")
    }

    /// Añade un nodo nuevo de un tipo, en una posición del lienzo. Devuelve su nombre.
    /// `taken` dice qué otros nombres no conviene usar (los nodos comunes, en un bioma).
    pub fn add_node(&mut self, kind: &str, pos: [f32; 2], taken: impl Fn(&str) -> bool) -> anyhow::Result<String> {
        let schema = graph::schema(kind).with_context(|| format!("unknown node type \"{kind}\""))?;
        let name = self.free_name(kind, taken);
        put(self.nodes_mut(), &name, Some(&toml::Value::Table(schema.new_node())));
        self.set_position(&name, pos);
        Ok(name)
    }

    /// Quita un nodo. Lo que lo usaba pasa a usar un número (el valor inicial de esa
    /// entrada), y las reglas que lo miraban dejan de hacerlo.
    pub fn remove_node(&mut self, name: &str) {
        self.nodes_mut().remove(name);
        self.replace_refs(name, None);
        if let Some(layout) = self.layout_mut(false) {
            layout.remove(name);
        }
    }

    /// Cambia el nombre de un nodo y todas sus referencias en este archivo.
    pub fn rename_node(&mut self, old: &str, new: &str) -> anyhow::Result<()> {
        if old == new {
            return Ok(());
        }
        if !valid_node_name(new) {
            bail!("\"{new}\" is not a valid node name: use lowercase letters, digits and _");
        }
        if self.has_node(new) {
            bail!("there is already a node called \"{new}\"");
        }
        if !self.has_node(old) {
            bail!("there is no node called \"{old}\"");
        }
        rename_key(self.nodes_mut(), old, new);
        if let Some(layout) = self.layout_mut(false) {
            rename_key(layout, old, new);
        }
        self.replace_refs(old, Some(new));
        Ok(())
    }

    /// Sustituye las referencias a `old` por `new` en los nodos, la altura y las reglas;
    /// con `None`, por un número.
    pub fn replace_refs(&mut self, old: &str, new: Option<&str>) {
        for name in self.node_names() {
            let Some(node) = self.node(&name) else { continue };
            let Some(schema) = kind_of(&node).and_then(graph::schema) else { continue };
            for param in schema.params {
                let Some(value) = node.get(param.name) else { continue };
                let fallback = number(param.default as f64);
                let replaced = match (param.kind, value) {
                    (ParamKind::Input, value) if refers(value, old) => {
                        Some(new.map_or(fallback, |new| toml::Value::String(new.into())))
                    }
                    (ParamKind::Inputs, toml::Value::Array(items)) if items.iter().any(|v| refers(v, old)) => {
                        let mut items: Vec<toml::Value> = items
                            .iter()
                            .filter_map(|item| match (refers(item, old), new) {
                                (false, _) => Some(item.clone()),
                                (true, Some(new)) => Some(toml::Value::String(new.into())),
                                (true, None) => None,
                            })
                            .collect();
                        if items.is_empty() {
                            items.push(fallback);
                        }
                        Some(toml::Value::Array(items))
                    }
                    _ => None,
                };
                if let Some(replaced) = replaced {
                    self.set_param(&name, param.name, Some(replaced));
                }
            }
        }
        if self.kind == DocKind::Biome {
            if self.get("height").is_some_and(|v| refers(&v, old)) {
                if let Some(new) = new {
                    self.set("height", Some(toml::Value::String(new.into())));
                }
            }
            for section in SECTIONS {
                for (index, entry) in self.entries(section).into_iter().enumerate() {
                    let key = if section == "surface" { "node" } else { "density" };
                    if !entry.get(key).is_some_and(|v| refers(v, old)) {
                        continue;
                    }
                    match (new, section) {
                        (Some(new), _) => self.set_entry(section, index, key, Some(toml::Value::String(new.into()))),
                        (None, "surface") => {
                            for key in ["node", "node_above", "node_below"] {
                                self.set_entry(section, index, key, None);
                            }
                        }
                        (None, _) => self.set_entry(section, index, key, Some(number(0.1))),
                    }
                }
            }
        }
    }

    /// Nodos que usan a `name` directamente (en una entrada o en una lista).
    pub fn users(&self, name: &str) -> Vec<String> {
        self.node_names()
            .into_iter()
            .filter(|user| {
                self.node(user).is_some_and(|node| {
                    node.values().any(|value| {
                        refers(value, name) || value.as_array().is_some_and(|items| items.iter().any(|v| refers(v, name)))
                    })
                })
            })
            .collect()
    }

    // --- Lienzo ---

    fn layout(&self) -> Option<&dyn TableLike> {
        self.doc.get("editor")?.as_table_like()?.get("layout")?.as_table_like()
    }

    fn layout_mut(&mut self, create: bool) -> Option<&mut dyn TableLike> {
        if create && self.layout().is_none() {
            if self.doc.get("editor").and_then(Item::as_table_like).is_none() {
                let mut editor = Table::new();
                editor.set_implicit(true);
                // Al final del archivo: es lo que menos interesa a quien lo lee.
                editor.set_position(usize::MAX / 2);
                self.doc.insert("editor", Item::Table(editor));
            }
            let mut layout = Table::new();
            layout.set_position(usize::MAX / 2 + 1);
            let editor = self.doc.get_mut("editor").and_then(Item::as_table_like_mut)?;
            editor.insert("layout", Item::Table(layout));
        }
        self.doc.get_mut("editor")?.as_table_like_mut()?.get_mut("layout")?.as_table_like_mut()
    }

    /// Lo guardado de un nodo en el lienzo: `[x, y]` o `[x, y, ancho, alto]`.
    fn layout_numbers(&self, name: &str) -> Vec<f32> {
        let Some(value) = self.layout().and_then(|layout| layout.get(name)).and_then(item_to_plain) else {
            return Vec::new();
        };
        let items = value.as_array().cloned().unwrap_or_default();
        items.iter().map_while(|v| v.as_float().or(v.as_integer().map(|n| n as f64))).map(|v| v as f32).collect()
    }

    fn set_layout_numbers(&mut self, name: &str, numbers: &[f32]) {
        let value = toml::Value::Array(numbers.iter().map(|n| number(n.round() as f64)).collect());
        if let Some(layout) = self.layout_mut(true) {
            put(layout, name, Some(&value));
        }
    }

    /// Dónde se dibuja un nodo en el lienzo, si se ha guardado.
    pub fn position(&self, name: &str) -> Option<[f32; 2]> {
        match self.layout_numbers(name)[..] {
            [x, y, ..] => Some([x, y]),
            _ => None,
        }
    }

    /// Cambia dónde se dibuja un nodo (conserva su tamaño, si tiene uno).
    pub fn set_position(&mut self, name: &str, pos: [f32; 2]) {
        let mut numbers = self.layout_numbers(name);
        numbers.resize(numbers.len().max(2), 0.0);
        numbers[0] = pos[0];
        numbers[1] = pos[1];
        numbers.truncate(if numbers.len() >= 4 { 4 } else { 2 });
        self.set_layout_numbers(name, &numbers);
    }

    /// Tamaño de la caja de un nodo en el lienzo, si se ha cambiado a mano.
    pub fn size(&self, name: &str) -> Option<[f32; 2]> {
        match self.layout_numbers(name)[..] {
            [_, _, w, h, ..] => Some([w, h]),
            _ => None,
        }
    }

    /// Cambia (o quita, con `None`) el tamaño de la caja de un nodo. Necesita su posición.
    pub fn set_size(&mut self, name: &str, size: Option<[f32; 2]>) {
        let Some([x, y]) = self.position(name) else { return };
        match size {
            Some([w, h]) => self.set_layout_numbers(name, &[x, y, w, h]),
            None => self.set_layout_numbers(name, &[x, y]),
        }
    }

    // --- Reglas: `[[surface]]`, `[[trees]]` y `[[plants]]` ---

    fn section(&self, section: &str) -> Option<&ArrayOfTables> {
        self.doc.get(section)?.as_array_of_tables()
    }

    fn section_mut(&mut self, section: &str) -> &mut ArrayOfTables {
        if self.section(section).is_none() {
            self.doc.insert(section, Item::ArrayOfTables(ArrayOfTables::new()));
        }
        self.doc.get_mut(section).and_then(Item::as_array_of_tables_mut).expect("an array of tables")
    }

    pub fn entries(&self, section: &str) -> Vec<toml::Table> {
        self.section(section).map_or_else(Vec::new, |tables| tables.iter().map(|table| plain_table(table)).collect())
    }

    pub fn set_entry(&mut self, section: &str, index: usize, key: &str, value: Option<toml::Value>) {
        if let Some(table) = self.section_mut(section).get_mut(index) {
            put(table, key, value.as_ref());
        }
    }

    pub fn add_entry(&mut self, section: &str, entry: &toml::Table) {
        let mut table = Table::new();
        for (key, value) in entry {
            table.insert(key, Item::Value(to_edit(value)));
        }
        // Detrás de la última de su lista (o al final, si es la primera).
        let position = self.section(section).and_then(|t| t.iter().last()).and_then(Table::position);
        if let Some(position) = position {
            shift_positions(self.doc.as_table_mut(), position + 1);
            table.set_position(position + 1);
        }
        self.section_mut(section).push(table);
    }

    pub fn remove_entry(&mut self, section: &str, index: usize) {
        let tables = self.section_mut(section);
        if index < tables.len() {
            tables.remove(index);
        }
        if self.section(section).is_some_and(ArrayOfTables::is_empty) {
            self.doc.remove(section);
        }
    }

    /// Mueve una regla una posición arriba (`-1`) o abajo (`1`). El orden importa: vale
    /// la primera que se cumple.
    pub fn move_entry(&mut self, section: &str, index: usize, delta: i32) {
        let tables = self.section_mut(section);
        let target = index as i64 + delta as i64;
        if index >= tables.len() || target < 0 || target as usize >= tables.len() {
            return;
        }
        let target = target as usize;
        let mut all: Vec<Table> = tables.iter().cloned().collect();
        // Cada tabla conserva su texto; solo se cambian las posiciones.
        let (a, b) = (all[index].position(), all[target].position());
        all.swap(index, target);
        if let (Some(a), Some(b)) = (a, b) {
            all[index].set_position(a);
            all[target].set_position(b);
        }
        tables.clear();
        for table in all {
            tables.push(table);
        }
    }
}

/// Cambia una clave de nombre sin moverla ni perder su comentario.
fn rename_key(table: &mut dyn TableLike, old: &str, new: &str) {
    let Some(item) = table.get(old).cloned() else { return };
    let decor = table.key(old).map(|key| key.leaf_decor().clone());
    let names: Vec<String> = table.iter().map(|(name, _)| name.to_string()).collect();
    // Las de detrás se sacan y se vuelven a poner, para que la nueva quede en su sitio.
    let after: Vec<(Key, Item)> = names
        .iter()
        .skip_while(|name| name.as_str() != old)
        .skip(1)
        .filter_map(|name| Some((table.key(name)?.clone(), table.remove(name)?)))
        .collect();
    table.remove(old);
    let mut key = Key::new(new);
    if let Some(decor) = decor {
        key = key.with_leaf_decor(decor);
    }
    insert_formatted(table, &key, item);
    for (key, item) in after {
        insert_formatted(table, &key, item);
    }
}

fn insert_formatted(table: &mut dyn TableLike, key: &Key, item: Item) {
    *table.entry_format(key).or_insert(Item::None) = item;
}

/// Desplaza una posición hacia atrás todas las tablas del documento que están en `from` o
/// después: deja hueco para una tabla nueva en `from`.
fn shift_positions(table: &mut Table, from: usize) {
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(table) => {
                if let Some(position) = table.position().filter(|p| *p >= from) {
                    table.set_position(position + 1);
                }
                shift_positions(table, from);
            }
            Item::ArrayOfTables(tables) => {
                for table in tables.iter_mut() {
                    if let Some(position) = table.position().filter(|p| *p >= from) {
                        table.set_position(position + 1);
                    }
                    shift_positions(table, from);
                }
            }
            _ => {}
        }
    }
}

/// Todos los archivos del terreno de una carpeta de contenido, abiertos para editarlos.
#[derive(Clone, Debug)]
pub struct Workspace {
    pub world: GenDoc,
    /// Por identificador: el archivo por defecto de cada bioma.
    pub biomes: Vec<GenDoc>,
    /// Los perfiles de los biomas, por bioma y nombre.
    pub profiles: Vec<GenDoc>,
    /// Perfil elegido de cada bioma (identificador → nombre); los que no están usan su
    /// archivo por defecto. Lo decide el mundo: quien abre el editor lo copia aquí.
    pub active: BTreeMap<String, String>,
}

/// Carpeta `worldgen/` de un archivo de bioma (`worldgen/biomes/x.toml`).
fn worldgen_folder(biome: &GenDoc) -> PathBuf {
    biome.path.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default()
}

/// Carpeta de los perfiles de un bioma (de su archivo por defecto).
fn biome_profiles(biome: &GenDoc) -> PathBuf {
    worldgen_folder(biome).join(PROFILES).join(content::stem(&biome.path))
}

/// Plantilla de un bioma nuevo: llano, de hierba, sobre la costa y la cordillera comunes.
const NEW_BIOME: &str = r##"name = "{name}"
color = "#7FC97F"

[nodes]
rough = { type = "mul", inputs = ["hills", 4] }
lowland = { type = "add", inputs = [20, "rough"] }
shore = { type = "smoothstep", input = "across", from = 0.1, to = 0.18 }
coast = { type = "mix", a = "seafloor", b = "lowland", t = "shore" }
height = { type = "add", inputs = ["coast", "cordillera"] }

[[surface]]
below = 2
top = "base:sand"

[[surface]]
above = 300
top = "base:snow"
under = "base:stone"

[[surface]]
top = "base:grass"
under = "base:dirt"
"##;

impl Workspace {
    /// Abre `worldgen/` de todos los paquetes, con las mismas reglas que el generador:
    /// manda el último `world.toml`, y un bioma de un paquete posterior tapa al anterior.
    pub fn open(content_dir: &Path) -> anyhow::Result<Self> {
        let mut world = None;
        let mut biomes: Vec<GenDoc> = Vec::new();
        for pack in content::packs(content_dir)? {
            let folder = pack.join(FOLDER);
            let world_path = folder.join("world.toml");
            if world_path.is_file() {
                world = Some(GenDoc::open("world", DocKind::World, &world_path)?);
            }
            let biome_dir = folder.join("biomes");
            if !biome_dir.is_dir() {
                continue;
            }
            for path in content::sorted_entries(&biome_dir)? {
                if path.extension().is_none_or(|ext| ext != "toml") {
                    continue;
                }
                let id = format!("{}:{}", content::stem(&pack), content::stem(&path));
                biomes.retain(|doc| doc.id != id);
                biomes.push(GenDoc::open(&id, DocKind::Biome, &path)?);
            }
        }
        let world = world.with_context(|| format!("no {FOLDER}/world.toml in {}", content_dir.display()))?;
        biomes.sort_by(|a, b| a.id.cmp(&b.id));
        let mut profiles = Vec::new();
        for biome in &biomes {
            let folder = biome_profiles(biome);
            if !folder.is_dir() {
                continue;
            }
            for path in content::sorted_entries(&folder)? {
                let name = content::stem(&path);
                if path.extension().is_none_or(|ext| ext != "toml") || !valid_profile_name(&name) {
                    continue;
                }
                let mut doc = GenDoc::open(&biome.id, DocKind::Biome, &path)?;
                doc.profile = Some(name);
                profiles.push(doc);
            }
        }
        Ok(Self { world, biomes, profiles, active: BTreeMap::new() })
    }

    /// Un documento por su clave (`world`, `base:plains`, `base:plains@perfil`).
    pub fn doc(&self, key: &str) -> Option<&GenDoc> {
        match key.split_once('@') {
            _ if key == "world" => Some(&self.world),
            Some((id, profile)) => self.profiles.iter().find(|doc| doc.id == id && doc.profile.as_deref() == Some(profile)),
            None => self.biomes.iter().find(|doc| doc.id == key),
        }
    }

    pub fn doc_mut(&mut self, key: &str) -> Option<&mut GenDoc> {
        match key.split_once('@') {
            _ if key == "world" => Some(&mut self.world),
            Some((id, profile)) => {
                self.profiles.iter_mut().find(|doc| doc.id == id && doc.profile.as_deref() == Some(profile))
            }
            None => self.biomes.iter_mut().find(|doc| doc.id == key),
        }
    }

    /// Todos los documentos, el mundo primero.
    fn all_mut(&mut self) -> impl Iterator<Item = &mut GenDoc> {
        std::iter::once(&mut self.world).chain(self.biomes.iter_mut()).chain(self.profiles.iter_mut())
    }

    pub fn dirty(&self) -> bool {
        self.world.dirty() || self.biomes.iter().chain(&self.profiles).any(GenDoc::dirty)
    }

    /// Perfiles guardados (o recién creados) de un bioma, por nombre.
    pub fn profile_names(&self, id: &str) -> Vec<String> {
        let mut names: Vec<String> =
            self.profiles.iter().filter(|doc| doc.id == id).filter_map(|doc| doc.profile.clone()).collect();
        names.sort();
        names
    }

    /// Clave del documento que usa un bioma ahora: su perfil elegido, si existe, o su
    /// archivo por defecto.
    pub fn active_key(&self, id: &str) -> String {
        match self.active.get(id) {
            Some(profile) if self.doc(&format!("{id}@{profile}")).is_some() => format!("{id}@{profile}"),
            _ => id.to_string(),
        }
    }

    /// Crea un perfil nuevo de un bioma, copia del documento `from` (su archivo por
    /// defecto u otro perfil), y lo guarda. Devuelve su clave.
    pub fn new_profile(&mut self, from: &str, name: &str) -> anyhow::Result<String> {
        if !valid_profile_name(name) {
            bail!("\"{name}\" is not a valid profile name: use lowercase letters, digits and _");
        }
        let source = self.doc(from).filter(|doc| doc.kind == DocKind::Biome).with_context(|| format!("{from} is not a biome"))?;
        let id = source.id.clone();
        if self.doc(&format!("{id}@{name}")).is_some() {
            bail!("{id} already has a profile called \"{name}\"");
        }
        let default = self.doc(&id).with_context(|| format!("unknown biome {id}"))?;
        let path = profile_path(&worldgen_folder(default), &content::stem(&default.path), name);
        let mut doc = GenDoc::parse(&id, DocKind::Biome, path, &source.text())?;
        doc.profile = Some(name.to_string());
        doc.saved.clear();
        doc.save()?;
        let key = doc.key();
        self.profiles.push(doc);
        Ok(key)
    }

    /// Borra un perfil (también del disco). Si estaba elegido, el bioma vuelve a su
    /// archivo por defecto.
    pub fn delete_profile(&mut self, id: &str, name: &str) -> anyhow::Result<()> {
        let key = format!("{id}@{name}");
        let index = self.profiles.iter().position(|doc| doc.key() == key).with_context(|| format!("unknown profile {key}"))?;
        let path = self.profiles[index].path.clone();
        if path.is_file() {
            std::fs::remove_file(&path).with_context(|| format!("failed to delete {}", path.display()))?;
        }
        self.profiles.remove(index);
        if self.active.get(id).is_some_and(|active| active == name) {
            self.active.remove(id);
        }
        Ok(())
    }

    /// El terreno tal como está ahora en el editor (sin guardar), con los perfiles
    /// elegidos, o el primer error.
    pub fn source(&self) -> anyhow::Result<WorldSource> {
        let texts: Vec<(String, String)> = self
            .biomes
            .iter()
            .map(|doc| (doc.id.clone(), self.doc(&self.active_key(&doc.id)).unwrap_or(doc).text()))
            .collect();
        let pairs: Vec<(&str, &str)> = texts.iter().map(|(id, text)| (id.as_str(), text.as_str())).collect();
        Ok(WorldSource::parse(&self.world.text(), &pairs)?.without_editor())
    }

    /// Guarda los archivos con cambios. Devuelve cuántos.
    pub fn save(&mut self) -> anyhow::Result<usize> {
        let mut saved = 0;
        for doc in self.all_mut() {
            if doc.dirty() {
                doc.save()?;
                saved += 1;
            }
        }
        Ok(saved)
    }

    /// Cambia el nombre de un nodo. Si es un nodo común (de `world.toml`), también en los
    /// biomas que lo usan sin tener uno propio con ese nombre.
    pub fn rename_node(&mut self, doc: &str, old: &str, new: &str) -> anyhow::Result<()> {
        if doc == "world" && self.biomes.iter().any(|biome| biome.has_node(new)) {
            bail!("a biome already has a node called \"{new}\"");
        }
        if doc != "world" && self.world.has_node(new) {
            bail!("\"{new}\" is a shared node of world.toml");
        }
        self.doc_mut(doc).with_context(|| format!("unknown file {doc}"))?.rename_node(old, new)?;
        if doc == "world" {
            for biome in self.biomes.iter_mut().chain(self.profiles.iter_mut()).filter(|biome| !biome.has_node(old)) {
                biome.replace_refs(old, Some(new));
            }
        }
        Ok(())
    }

    /// Crea un bioma nuevo (sin guardarlo aún) junto a `world.toml` y lo añade al final de
    /// la lista del mundo. Devuelve su identificador.
    pub fn new_biome(&mut self, file: &str, name: &str) -> anyhow::Result<String> {
        if !valid_node_name(file) {
            bail!("\"{file}\" is not a valid file name: use lowercase letters, digits and _");
        }
        let folder = self.world.path.parent().context("world.toml has no folder")?.to_path_buf();
        let pack = folder.parent().map(content::stem).unwrap_or_default();
        let id = format!("{pack}:{file}");
        if self.doc(&id).is_some() {
            bail!("there is already a biome called {id}");
        }
        let text = NEW_BIOME.replace("{name}", &name.replace('"', "'"));
        let path = folder.join("biomes").join(format!("{file}.toml"));
        let mut doc = GenDoc::parse(&id, DocKind::Biome, path, &text)?;
        // Aún no existe en disco: cuenta como cambio sin guardar.
        doc.saved.clear();
        self.biomes.push(doc);
        self.biomes.sort_by(|a, b| a.id.cmp(&b.id));
        let mut order = self.world.get("biomes").and_then(|v| v.as_array().cloned()).unwrap_or_default();
        order.push(toml::Value::String(file.into()));
        self.world.set("biomes", Some(toml::Value::Array(order)));
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIOME: &str = r#"# Un bioma de prueba.
name = "Plains"

[nodes]
# Colinas suaves.
hills = { type = "noise", scale = 120 }   # el ruido base
lift = { type = "mul", inputs = ["hills", 5] }
height = { type = "add", inputs = [20, "lift", "hills"] }

[[surface]]
below = 2
top = "base:sand"

# Lo demás, hierba.
[[surface]]
top = "base:grass"

[[trees]]
density = "hills"
wood = "base:wood"
leaves = ["base:leaves"]
"#;

    fn doc() -> GenDoc {
        GenDoc::parse("base:plains", DocKind::Biome, "plains.toml".into(), BIOME).unwrap()
    }

    #[test]
    fn numbers_are_written_short() {
        assert_eq!(number(3.0), toml::Value::Integer(3));
        assert_eq!(number(0.3f32 as f64), toml::Value::Float(0.3));
        assert_eq!(number(0.07), toml::Value::Float(0.07));
    }

    #[test]
    fn editing_a_parameter_keeps_the_comments() {
        let mut doc = doc();
        assert!(!doc.dirty());
        doc.set_param("hills", "scale", Some(number(300.0)));
        doc.set_param("hills", "octaves", Some(number(2.0)));
        let text = doc.text();
        assert!(doc.dirty());
        assert!(text.contains("# Colinas suaves.\nhills = { type = \"noise\", scale = 300, octaves = 2 }   # el ruido base"), "{text}");
        assert!(text.contains("# Lo demás, hierba."));
        assert_eq!(doc.node("hills").unwrap()["scale"].as_integer(), Some(300));
        doc.set_param("hills", "octaves", None);
        assert!(doc.text().contains("scale = 300 }"), "{}", doc.text());
    }

    #[test]
    fn renaming_and_removing_nodes_update_their_users() {
        let mut doc = doc();
        doc.rename_node("hills", "bumps").unwrap();
        let text = doc.text();
        assert!(text.contains("# Colinas suaves.\nbumps = { type = \"noise\""), "{text}");
        assert!(text.contains("inputs = [\"bumps\", 5]") && text.contains("[20, \"lift\", \"bumps\"]"));
        assert!(text.contains("density = \"bumps\""));
        assert_eq!(doc.node_names(), ["bumps", "lift", "height"], "the node keeps its place");
        assert!(doc.rename_node("lift", "height").is_err());
        assert!(doc.rename_node("lift", "Bad Name").is_err());
        assert!(doc.rename_node("lift", "x").is_err(), "built-in inputs are reserved");
        assert_eq!(doc.users("bumps"), ["lift", "height"]);

        doc.remove_node("bumps");
        let text = doc.text();
        assert!(!doc.has_node("bumps"));
        assert!(text.contains("inputs = [5]") && text.contains("inputs = [20, \"lift\"]"), "{text}");
        assert!(text.contains("density = 0.1"));
    }

    #[test]
    fn nodes_are_added_with_a_free_name_and_a_position() {
        let mut doc = doc();
        let name = doc.add_node("ridge", [120.4, -30.0], |_| false).unwrap();
        assert_eq!(name, "ridge");
        assert_eq!(doc.add_node("ridge", [0.0, 0.0], |_| false).unwrap(), "ridge_2");
        assert_eq!(doc.add_node("noise", [0.0, 0.0], |name| name == "noise").unwrap(), "noise_2");
        assert_eq!(doc.position("ridge"), Some([120.0, -30.0]));
        // El tamaño de la caja va detrás de la posición, y moverla lo conserva.
        doc.set_size("ridge", Some([260.0, 300.0]));
        doc.set_position("ridge", [10.0, 20.0]);
        assert_eq!((doc.position("ridge"), doc.size("ridge")), (Some([10.0, 20.0]), Some([260.0, 300.0])));
        assert!(doc.text().contains("ridge = [10, 20, 260, 300]"), "{}", doc.text());
        doc.set_size("ridge", None);
        assert_eq!((doc.position("ridge"), doc.size("ridge")), (Some([10.0, 20.0]), None));
        doc.set_position("height", [400.0, 10.0]);
        let text = doc.text();
        assert!(text.trim_end().ends_with("height = [400, 10]"), "the layout goes last:\n{text}");
        assert!(text.contains("[editor.layout]") && !text.contains("[editor]\n"));
        // El resultado se puede leer como bioma.
        let source = WorldSource::parse("biomes = [\"plains\"]", &[("base:plains", &text)]).unwrap();
        assert!(source.biomes["base:plains"].nodes.contains_key("ridge_2"));
        assert!(doc.add_node("nope", [0.0, 0.0], |_| false).is_err());
    }

    #[test]
    fn rules_are_edited_added_moved_and_removed() {
        let mut doc = doc();
        assert_eq!(doc.entries("surface").len(), 2);
        doc.set_entry("surface", 1, "under", Some(toml::Value::String("base:dirt".into())));
        doc.move_entry("surface", 1, -1);
        let surface = doc.entries("surface");
        assert_eq!(surface[0]["top"].as_str(), Some("base:grass"));
        assert_eq!(surface[0]["under"].as_str(), Some("base:dirt"));
        assert_eq!(surface[1]["top"].as_str(), Some("base:sand"));
        let text = doc.text();
        assert!(text.find("base:grass").unwrap() < text.find("base:sand").unwrap(), "{text}");

        let mut plant = toml::Table::new();
        plant.insert("density".into(), number(0.02));
        plant.insert("block".into(), toml::Value::String("base:flower".into()));
        doc.add_entry("plants", &plant);
        doc.add_entry("surface", &plant);
        let text = doc.text();
        assert_eq!(doc.entries("plants").len(), 1);
        assert!(text.contains("[[plants]]\ndensity = 0.02"), "{text}");
        assert!(text.find("[[surface]]\ndensity").unwrap() < text.find("[[trees]]").unwrap(), "{text}");
        doc.remove_entry("plants", 0);
        assert!(!doc.text().contains("[[plants]]"));
    }

    #[test]
    fn the_workspace_reads_and_writes_the_game_files() {
        let workspace_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = std::env::temp_dir().join(format!("kuravoxel-gendoc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let copy = |from: &Path, to: &Path| {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap().flatten().filter(|e| e.path().is_file()) {
                std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
            }
        };
        let worldgen = workspace_dir.join("content/base/worldgen");
        copy(&worldgen, &dir.join("base/worldgen"));
        copy(&worldgen.join("biomes"), &dir.join("base/worldgen/biomes"));

        let mut workspace = Workspace::open(&dir).unwrap();
        assert_eq!(workspace.biomes.len(), 9);
        assert!(!workspace.dirty());
        let original = workspace.source().unwrap();
        assert_eq!(original, WorldSource::load(&dir).unwrap().without_editor());

        // Un nodo común cambia de nombre también en los biomas que lo usan.
        workspace.rename_node("world", "hills", "undulation").unwrap();
        assert!(workspace.doc("base:plains").unwrap().text().contains("\"undulation\""));
        assert!(workspace.rename_node("world", "undulation", "flat_noise").is_err(), "plains has its own flat_noise");
        workspace.doc_mut("base:plains").unwrap().set_position("height", [10.0, 20.0]);
        assert!(workspace.source().is_ok());

        let id = workspace.new_biome("salt_flats", "Salar").unwrap();
        assert_eq!(id, "base:salt_flats");
        assert!(workspace.source().unwrap().biomes.contains_key(&id));
        let biomes = workspace.biomes.iter().filter(|biome| biome.dirty()).count();
        assert!(biomes >= 8, "most biomes use hills, and there is a new one ({biomes})");
        assert_eq!(workspace.save().unwrap(), 1 + biomes);
        assert!(!workspace.dirty());
        let reloaded = Workspace::open(&dir).unwrap();
        assert_eq!(reloaded.doc("base:plains").unwrap().position("height"), Some([10.0, 20.0]));
        assert_eq!(reloaded.source().unwrap(), workspace.source().unwrap());

        // Un perfil: copia del bioma, guardada al momento, que no toca el archivo por defecto.
        let default = workspace.doc("base:plains").unwrap().text();
        let key = workspace.new_profile("base:plains", "rivers").unwrap();
        assert_eq!(key, "base:plains@rivers");
        assert!(dir.join("base/worldgen/profiles/plains/rivers.toml").is_file());
        assert!(workspace.new_profile("base:plains", "rivers").is_err());
        assert!(workspace.new_profile("base:plains", "Bad Name").is_err());
        assert!(workspace.new_profile("world", "nope").is_err());
        workspace.doc_mut(&key).unwrap().set_param("flat_noise", "scale", Some(number(123.0)));
        workspace.save().unwrap();
        assert_eq!(workspace.doc("base:plains").unwrap().text(), default);
        // Solo cuenta si el mundo lo elige.
        let scale = |source: &WorldSource| format!("{:?}", source.biomes["base:plains"].nodes["flat_noise"]);
        assert!(!scale(&workspace.source().unwrap()).contains("123"));
        workspace.active.insert("base:plains".into(), "rivers".into());
        assert_eq!(workspace.active_key("base:plains"), key);
        assert!(scale(&workspace.source().unwrap()).contains("123"));
        let profiles = BTreeMap::from([("base:plains".to_string(), "rivers".to_string())]);
        assert_eq!(workspace.source().unwrap(), WorldSource::load_with(&dir, &profiles).unwrap().without_editor());
        // Al volver a abrir, el perfil sigue ahí; borrado, desaparece y deja de estar elegido.
        let mut reloaded = Workspace::open(&dir).unwrap();
        assert_eq!(reloaded.profile_names("base:plains"), ["rivers"]);
        reloaded.active = workspace.active.clone();
        reloaded.delete_profile("base:plains", "rivers").unwrap();
        assert!(!dir.join("base/worldgen/profiles/plains/rivers.toml").exists());
        assert_eq!(reloaded.active_key("base:plains"), "base:plains");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
