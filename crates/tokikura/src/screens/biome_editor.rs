//! Editor de biomas: el grafo de nodos de cada bioma en un lienzo, sus reglas de
//! superficie y vegetación en formularios, y los ajustes del mundo (`world.toml`).
//!
//! Edita los archivos de `content/<paquete>/worldgen/` a través de `worldgen_doc` (que
//! conserva sus comentarios). Cada cambio pide una vista previa nueva (la miniatura de
//! cada nodo y el mapa de la zona, calculados en otro hilo por `game.rs`) y, al guardar,
//! la recarga en caliente del servidor regenera el mundo a la vista. Con "Auto", guarda
//! solo un momento después del último cambio. Con "Live world" apagado, el mundo no
//! cambia hasta pulsar "Apply to world".
//!
//! Cada bioma tiene su archivo por defecto y, si se quiere, perfiles: otras versiones
//! enteras del bioma. El perfil que se edita es el que usa el mundo abierto (lo guarda el
//! servidor en el `world.toml` del mundo).
//!
//! Se dibuja en una ventana propia (`game.rs`), con el mundo en la ventana del juego.
//!
//! Lo que muestra del juego (contenido, posición del jugador) se lo copia `game.rs`; lo
//! que quiere de fuera (cerrar, teletransportarse, calcular una vista previa) lo pide.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kuravoxel_common::content::ContentData;
use kuravoxel_common::physics::EYE_HEIGHT;
use kuravoxel_common::protocol::{TerrainOp, TerrainStatus};
use kuravoxel_server::biomes::{Terrain, SEA_LEVEL};
use kuravoxel_server::graph::{self, ParamKind, NODE_TYPES};
use kuravoxel_server::preview::{Grid, Window};
use kuravoxel_server::worldgen_doc::{number, DocKind, GenDoc, Workspace};
use kuravoxel_ui::core::{border, mouse, Alignment, Color, Length, Point, Rectangle, Size};
use kuravoxel_ui::widget::canvas::{self, Event, Frame, Geometry, Path, Stroke};
use kuravoxel_ui::widget::{
    button, canvas as canvas_widget, checkbox, column, container, image, pick_list, row, scrollable, space, text,
    text_input, Column, Row,
};
use kuravoxel_ui::{Element, Renderer, Screen, Theme};

use super::node_graph::{GraphEvent, GraphNode, NodeGraph, Row as GraphRow, MAX_ZOOM, MIN_ZOOM, NODE_WIDTH};
use kuravoxel_server::biomes::valid_profile_name;
use super::style;
use crate::biome_preview::{Done, Job};

/// Tras el último cambio, cuánto se espera para guardar solo (con "Auto").
pub const AUTO_SAVE_DELAY: Duration = Duration::from_millis(1200);
/// Lado de las miniaturas de los nodos y del mapa, en píxeles.
const THUMB_SIZE: u32 = 64;
const MAP_SIZE: u32 = 280;
const SIDE_WIDTH: f32 = 300.0;
/// Ventana de la vista previa: lado inicial, mínimo y máximo, en metros.
const DEFAULT_SPAN: f64 = 3072.0;
const SPANS: (f64, f64) = (128.0, 16384.0);
const LABEL_WIDTH: f32 = 92.0;

const NUMBER: &str = "(number)";
/// En la lista de perfiles: el archivo del bioma.
const DEFAULT_PROFILE: &str = "default";
const NONE: &str = "(none)";
const DEFAULT: &str = "(default)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Nodes,
    Surface,
    Vegetation,
    Settings,
}

/// Dónde está un valor del archivo que se edita.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// Un campo suelto del archivo (`name`, `blend`...).
    Doc,
    Node(String),
    /// Una regla: lista (`surface`, `trees`, `plants`) y posición.
    Entry(&'static str, usize),
}

/// Cómo se escribe un campo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Number,
    Integer,
    /// Número que puede faltar: vacío lo quita.
    OptNumber,
    OptInteger,
    Text,
    /// Un número o varios separados por comas (`200` o `200, 500`).
    Numbers,
    /// Un nodo, o un número (el valor inicial en milésimas al dejar de ser nodo).
    Link(i32),
    /// Nombre que puede faltar (un bloque o un nodo opcionales).
    OptName,
    /// Uno de unos nombres, o nada (valor por defecto).
    Choice,
}

/// Un campo editable.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Field {
    pub target: Target,
    pub key: String,
    /// Elemento de una lista (`inputs`, `points`, `leaves`...).
    pub index: Option<usize>,
    pub kind: Kind,
}

impl Field {
    fn new(target: Target, key: &str, kind: Kind) -> Self {
        Self { target, key: key.into(), index: None, kind }
    }

    fn at(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Close,
    /// Llevar al jugador a este punto (sus ojos).
    Teleport([f64; 3]),
    /// Recarga del terreno y perfil de un bioma, para el servidor.
    Terrain(TerrainOp),
}

#[derive(Debug, Clone)]
pub enum Message {
    Doc(String),
    Tab(Tab),
    Close,
    Save,
    Revert,
    Auto(bool),
    Graph(GraphEvent),
    FitView,
    AddNode(String),
    DeleteNode,
    RenameText(String),
    Rename,
    ChangeType(String),
    Text(Field, String),
    Pick(Field, String),
    Toggle(Field, bool),
    /// Pone un valor entero de una vez (reordenar una lista).
    Set(Field, Option<toml::Value>),
    /// Añade un elemento a una lista (`field.index` = dónde).
    ListAdd(Field),
    /// Quita el elemento `field.index` de una lista.
    ListRemove(Field),
    EntryAdd(&'static str),
    EntryRemove(&'static str, usize),
    EntryMove(&'static str, usize, i32),
    /// Acercar (< 1) o alejar (> 1) la ventana de la vista previa.
    Zoom(f64),
    CenterOnPlayer,
    MapClick([f64; 2]),
    Teleport,
    NewBiomeText(String),
    NewBiome,
    /// Editar (y usar en el mundo) otro perfil del bioma, o `DEFAULT_PROFILE`.
    Profile(String),
    NewProfileText(String),
    /// Crear un perfil nuevo, copia del que se edita.
    NewProfile,
    /// Borrar el perfil que se edita (pide un segundo clic).
    DeleteProfile,
    /// Regenerar el mundo en cuanto se guarda un cambio, o esperar a `Apply`.
    LiveWorld(bool),
    /// Guardar y llevar al mundo lo que haya.
    Apply,
}

/// Miniatura de un nodo, ya lista para Iced.
struct Preview {
    handle: Option<image::Handle>,
    footer: String,
    error: bool,
}

pub struct BiomeEditor {
    // --- Copiado del juego ---
    content: Arc<ContentData>,
    player: [f64; 3],
    hosting: bool,

    // --- Propio ---
    content_dir: PathBuf,
    seed: u32,
    workspace: Option<Workspace>,
    doc: String,
    tab: Tab,
    selected: Option<String>,
    /// Vista del lienzo de cada archivo: esquina superior izquierda y zoom.
    views: HashMap<String, ([f32; 2], f32)>,
    /// Lo escrito en cada caja de texto (puede estar a medias).
    texts: HashMap<Field, String>,
    rename: String,
    new_biome: String,
    window: Window,
    previews: HashMap<String, Preview>,
    map: Option<image::Handle>,
    /// Ventana del último mapa recibido (puede ir por detrás de `window`).
    map_window: Window,
    map_error: Option<String>,
    preview_time: Duration,
    /// Mensaje de estado y si es un error.
    status: Option<(String, bool)>,
    auto_save: bool,
    changed_at: Option<Instant>,
    /// Último encargo de vista previa hecho y si hace falta otro.
    generation: u64,
    preview_due: bool,
    /// Falta elegir el bioma en el que está el jugador.
    find_here: bool,
    /// El terreno tal como está en el editor compila (o su error). Se comprueba al cambiar.
    check: Result<(), String>,
    /// Lo último que dijo el servidor del terreno (recarga, cambios esperando, perfiles).
    terrain: TerrainStatus,
    new_profile: String,
    /// Se ha pulsado una vez "Delete profile".
    confirm_delete: bool,
    requests: Vec<Request>,
}

impl Default for BiomeEditor {
    fn default() -> Self {
        Self {
            content: Arc::default(),
            player: [0.0; 3],
            hosting: false,
            content_dir: PathBuf::from("content"),
            seed: 0,
            workspace: None,
            doc: "world".into(),
            tab: Tab::Nodes,
            selected: None,
            views: HashMap::new(),
            texts: HashMap::new(),
            rename: String::new(),
            new_biome: String::new(),
            window: Window { center: [0.0, 0.0], span: DEFAULT_SPAN },
            previews: HashMap::new(),
            map: None,
            map_window: Window { center: [0.0, 0.0], span: DEFAULT_SPAN },
            map_error: None,
            preview_time: Duration::ZERO,
            status: None,
            auto_save: true,
            changed_at: None,
            generation: 0,
            preview_due: false,
            find_here: false,
            check: Ok(()),
            terrain: TerrainStatus { auto: true, ..TerrainStatus::default() },
            new_profile: String::new(),
            confirm_delete: false,
            requests: Vec::new(),
        }
    }
}

/// Un número corto para la interfaz.
fn show_number(value: f64) -> String {
    match number(value) {
        toml::Value::Integer(n) => n.to_string(),
        _ if value.abs() >= 100.0 => format!("{value:.0}"),
        _ if value.abs() >= 10.0 => format!("{value:.1}"),
        _ => {
            let text = format!("{value:.3}");
            text.trim_end_matches('0').trim_end_matches('.').to_string()
        }
    }
}

/// Un valor del archivo tal como se escribe en una caja.
fn show(value: &toml::Value) -> String {
    match value {
        toml::Value::Integer(n) => n.to_string(),
        toml::Value::Float(f) => show_number(*f),
        toml::Value::String(s) => s.clone(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Array(items) => items.iter().map(show).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

fn as_number(value: &toml::Value) -> Option<f64> {
    value.as_float().or(value.as_integer().map(|n| n as f64))
}

/// Lee lo escrito en una caja. `Err` = aún no vale (se deja escribir); `Ok(None)` = quitar.
fn parse(kind: Kind, written: &str) -> Result<Option<toml::Value>, ()> {
    let trimmed = written.trim();
    let numbers = || -> Result<Vec<f64>, ()> {
        trimmed.split(',').map(|part| part.trim().parse::<f64>().map_err(|_| ())).filter(|n| n.as_ref().map_or(true, |n| n.is_finite())).collect()
    };
    match kind {
        Kind::OptNumber | Kind::OptInteger | Kind::OptName if trimmed.is_empty() => Ok(None),
        Kind::Number | Kind::OptNumber | Kind::Link(_) => {
            trimmed.parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| Some(number(n))).ok_or(())
        }
        Kind::Integer | Kind::OptInteger => trimmed.parse::<i64>().map(|n| Some(toml::Value::Integer(n))).map_err(|_| ()),
        Kind::Text | Kind::OptName | Kind::Choice => Ok(Some(toml::Value::String(written.to_string()))),
        Kind::Numbers => {
            let values = numbers()?;
            Ok(Some(match values.as_slice() {
                [single] => number(*single),
                many => toml::Value::Array(many.iter().map(|n| number(*n)).collect()),
            }))
        }
    }
}

/// Imagen en grises de los valores de un nodo, de su mínimo (negro) a su máximo (blanco).
fn grid_image(grid: &Grid) -> image::Handle {
    let range = (grid.max - grid.min).max(1e-6);
    let mut rgba = Vec::with_capacity(grid.values.len() * 4);
    for value in &grid.values {
        let t = if grid.max > grid.min { ((value - grid.min) / range).clamp(0.0, 1.0) } else { 0.5 };
        let gray = (20.0 + t * 225.0) as u8;
        rgba.extend_from_slice(&[gray, gray, gray, 255]);
    }
    image::Handle::from_rgba(grid.size, grid.size, rgba)
}

/// Nodos del archivo de los que depende `node` (directa o indirectamente), incluido él.
fn dependencies(doc: &GenDoc, node: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![node.to_string()];
    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(table) = doc.node(&name) else { continue };
        for value in table.values() {
            let mut push = |v: &toml::Value| {
                if let Some(name) = v.as_str().filter(|n| doc.has_node(n)) {
                    stack.push(name.to_string());
                }
            };
            push(value);
            if let Some(items) = value.as_array() {
                items.iter().for_each(&mut push);
            }
        }
    }
    seen
}

impl BiomeEditor {
    /// Abre los archivos del terreno de una carpeta de contenido.
    pub fn open(&mut self, content_dir: PathBuf, seed: u32) {
        self.content_dir = content_dir;
        self.seed = seed;
        if self.workspace.as_ref().is_some_and(Workspace::dirty) {
            // Se vuelve con cambios sin guardar: se conservan tal cual.
            self.preview_due = true;
            return;
        }
        self.reload();
    }

    fn reload(&mut self) {
        match Workspace::open(&self.content_dir) {
            Ok(mut workspace) => {
                // La primera vez se abre el bioma en el que está el jugador (en cuanto se
                // sepa: hace falta el contenido).
                self.find_here |= self.workspace.is_none();
                workspace.active = self.terrain.profiles.clone();
                let doc = if workspace.doc(&self.doc).is_some() { self.doc.clone() } else { "world".into() };
                self.workspace = Some(workspace);
                self.select_doc(doc);
                self.status = None;
                self.validate();
            }
            Err(error) => {
                self.workspace = None;
                self.status = Some((format!("{error:#}"), true));
            }
        }
    }

    /// Bioma que hay bajo el jugador, según los archivos.
    fn biome_here(&self, workspace: &Workspace) -> Option<String> {
        let source = workspace.source().ok()?;
        let terrain = Terrain::compile(&source, &self.content, self.seed, None).ok()?;
        Some(terrain.biome_at(self.player[0].floor() as i32, self.player[2].floor() as i32).id.clone())
    }

    /// Copia el estado del juego que se va a dibujar.
    pub fn sync(&mut self, content: &Arc<ContentData>, player: [f64; 3], hosting: bool) {
        if !Arc::ptr_eq(&self.content, content) {
            self.content = content.clone();
            self.preview_due = true;
            self.validate();
        }
        self.player = player;
        self.hosting = hosting;
        if self.find_here && self.content.block_count() > 0 {
            self.find_here = false;
            if let Some(workspace) = self.workspace.as_ref() {
                if let Some(id) = self.biome_here(workspace) {
                    let key = workspace.active_key(&id);
                    self.select_doc(key);
                }
            }
        }
    }

    /// Lo que dice el servidor del terreno. Si el perfil que usa el mundo para el bioma
    /// que se edita ha cambiado (desde la consola, o porque no se aceptó), se pasa a él.
    pub fn set_terrain_status(&mut self, status: &TerrainStatus) {
        if self.terrain == *status {
            return;
        }
        self.terrain = status.clone();
        let Some(workspace) = self.workspace.as_mut() else { return };
        if workspace.active == status.profiles {
            return;
        }
        workspace.active = status.profiles.clone();
        let id = workspace.doc(&self.doc).filter(|doc| doc.kind == DocKind::Biome).map(|doc| doc.id.clone());
        if let Some(key) = id.map(|id| workspace.active_key(&id)).filter(|key| *key != self.doc) {
            self.select_doc(key);
        }
        self.preview_due = true;
        self.validate();
    }

    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    pub fn dirty(&self) -> bool {
        self.workspace.as_ref().is_some_and(Workspace::dirty)
    }

    fn current(&self) -> Option<&GenDoc> {
        self.workspace.as_ref()?.doc(&self.doc)
    }

    fn current_mut(&mut self) -> Option<&mut GenDoc> {
        let doc = self.doc.clone();
        self.workspace.as_mut()?.doc_mut(&doc)
    }

    fn select_doc(&mut self, doc: String) {
        self.doc = doc;
        self.selected = None;
        self.rename.clear();
        self.texts.clear();
        self.previews.clear();
        self.confirm_delete = false;
        // La vista del lienzo se fija al abrir el archivo: si se recalculase en cada frame,
        // mover el nodo de más arriba a la izquierda desplazaría todos los demás.
        if !self.views.contains_key(&self.doc) {
            let nodes = self.graph_nodes();
            let view = fit_view(&nodes);
            self.views.insert(self.doc.clone(), view);
        }
        // La vista previa se pone sobre la franja del bioma.
        if let Some(center) = self.band_center() {
            self.window = Window { center, span: DEFAULT_SPAN };
        } else {
            let length = self.workspace.as_ref().and_then(|w| w.world.get("length")).and_then(|v| as_number(&v));
            self.window = Window { center: [0.0, 0.0], span: length.unwrap_or(DEFAULT_SPAN) / 4.0 };
        }
        self.preview_due = true;
    }

    /// Centro de la franja del bioma que se edita, si está en la lista del mundo.
    fn band_center(&self) -> Option<[f64; 2]> {
        let workspace = self.workspace.as_ref()?;
        let doc = workspace.doc(&self.doc).filter(|doc| doc.kind == DocKind::Biome)?;
        let source = workspace.source().ok()?;
        let index = source.world.biomes.iter().position(|name| source.resolve(name).ok() == Some(doc.id.as_str()))?;
        let bands = source.world.biomes.len() as f64;
        let length = source.world.length as f64;
        Some([0.0, ((index as f64 + 0.5) / bands - 0.5) * length])
    }

    /// Nombres de nodo que se pueden usar desde el archivo actual: los suyos y, en un
    /// bioma, los comunes.
    fn usable_nodes(&self) -> Vec<String> {
        let Some(doc) = self.current() else { return Vec::new() };
        let mut names = doc.node_names();
        if doc.kind == DocKind::Biome {
            if let Some(world) = self.workspace.as_ref().map(|w| &w.world) {
                names.extend(world.node_names().into_iter().filter(|name| !doc.has_node(name)));
            }
        }
        names
    }

    fn changed(&mut self) {
        self.changed_at = Some(Instant::now());
        self.preview_due = true;
        self.validate();
    }

    /// Compila el terreno entero como lo haría el servidor: un nodo que no existe o un
    /// bloque mal escrito se ven aquí, antes de guardar.
    fn validate(&mut self) {
        let Some(workspace) = self.workspace.as_ref() else { return };
        self.check = workspace
            .source()
            .and_then(|source| Terrain::compile(&source, &self.content, self.seed, None).map(|_| ()))
            .map_err(|error| format!("{error:#}"));
    }

    fn error(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), true));
    }

    /// Guarda lo que haya cambiado: el servidor lo nota y regenera el mundo.
    fn save(&mut self) {
        let Some(workspace) = self.workspace.as_mut() else { return };
        self.changed_at = None;
        match workspace.save() {
            Ok(0) => {}
            Ok(files) => {
                let note = match self.check {
                    Ok(()) => "the world regenerates around you",
                    Err(_) => "it has errors: the world keeps the previous terrain",
                };
                self.status = Some((format!("Saved {files} file(s): {note}."), false));
            }
            Err(error) => self.error(format!("{error:#}")),
        }
    }

    /// Cada frame: guarda solo si toca.
    pub fn tick(&mut self, now: Instant) {
        let due = self.changed_at.is_some_and(|at| now.duration_since(at) >= AUTO_SAVE_DELAY);
        if self.auto_save && due && self.dirty() {
            // Con errores no se guarda solo: el mundo se quedaría igual y el archivo roto.
            if self.check.is_ok() {
                self.save();
            } else {
                self.changed_at = None;
            }
        }
    }

    /// El siguiente encargo de vista previa, si hace falta.
    pub fn take_job(&mut self) -> Option<Job> {
        if !self.preview_due || self.content.block_count() == 0 {
            return None;
        }
        self.preview_due = false;
        let workspace = self.workspace.as_ref()?;
        let doc = workspace.doc(&self.doc)?;
        let source = match workspace.source() {
            Ok(source) => source,
            Err(error) => {
                self.map_error = Some(format!("{error:#}"));
                return None;
            }
        };
        self.generation += 1;
        Some(Job {
            generation: self.generation,
            source: Arc::new(source),
            content: self.content.clone(),
            seed: self.seed,
            biome: (doc.kind == DocKind::Biome).then(|| doc.id.clone()),
            nodes: doc.node_names(),
            window: self.window,
            thumb: THUMB_SIZE,
            map: MAP_SIZE,
        })
    }

    /// Llega una vista previa calculada.
    pub fn preview_done(&mut self, done: Done) {
        let window = done.window;
        if done.generation != self.generation {
            return;
        }
        self.preview_time = done.elapsed;
        self.previews = done
            .nodes
            .into_iter()
            .map(|(name, grid)| {
                let preview = match grid {
                    Ok(grid) => Preview {
                        footer: format!("{} … {}", show_number(grid.min as f64), show_number(grid.max as f64)),
                        handle: Some(grid_image(&grid)),
                        error: false,
                    },
                    Err(error) => Preview { handle: None, footer: error, error: true },
                };
                (name, preview)
            })
            .collect();
        match done.map {
            Ok(map) => {
                self.map = Some(image::Handle::from_rgba(map.width, map.height, map.rgba));
                self.map_window = window;
                self.map_error = None;
            }
            Err(error) => self.map_error = Some(error),
        }
    }

    // --- Lectura y escritura de campos ---

    fn read(&self, field: &Field) -> Option<toml::Value> {
        let doc = self.current()?;
        let value = match &field.target {
            Target::Doc => doc.get(&field.key),
            Target::Node(node) => doc.node(node)?.get(&field.key).cloned(),
            Target::Entry(section, index) => doc.entries(section).get(*index)?.get(&field.key).cloned(),
        }?;
        match field.index {
            None => Some(value),
            Some(index) => value.as_array()?.get(index).cloned(),
        }
    }

    fn write(&mut self, field: &Field, value: Option<toml::Value>) {
        if matches!(field.target, Target::Node(_)) {
            self.pin_layout();
        }
        let whole = match field.index {
            None => value,
            Some(index) => {
                let current = Field { index: None, ..field.clone() };
                let mut items = self.read(&current).and_then(|v| v.as_array().cloned()).unwrap_or_default();
                match value {
                    Some(value) if index < items.len() => items[index] = value,
                    Some(value) => items.push(value),
                    None if index < items.len() => {
                        items.remove(index);
                    }
                    None => {}
                }
                Some(toml::Value::Array(items))
            }
        };
        let Some(doc) = self.current_mut() else { return };
        match &field.target {
            Target::Doc => doc.set(&field.key, whole),
            Target::Node(node) => doc.set_param(node, &field.key, whole),
            Target::Entry(section, index) => doc.set_entry(section, *index, &field.key, whole),
        }
        self.changed();
    }

    /// Texto de una caja: lo que se está escribiendo o el valor del archivo.
    fn shown(&self, field: &Field) -> String {
        self.texts.get(field).cloned().unwrap_or_else(|| self.read(field).map(|v| show(&v)).unwrap_or_default())
    }

    // --- Nodos ---

    fn connect(&mut self, from: &str, to: &str, param: &str, index: Option<usize>) {
        let Some(doc) = self.current() else { return };
        if from == to || doc.has_node(from) && dependencies(doc, from).contains(to) {
            self.error(format!("Connecting {from} to {to} would make a loop."));
            return;
        }
        let kind = doc.node(to).and_then(|n| n.get("type").and_then(|t| t.as_str()).and_then(graph::schema));
        let Some(param_kind) = kind.and_then(|schema| schema.param(param)).map(|p| p.kind) else { return };
        let field = Field { target: Target::Node(to.into()), key: param.into(), index: None, kind: Kind::Text };
        let link = toml::Value::String(from.into());
        match (param_kind, index) {
            (ParamKind::Inputs, Some(index)) => self.write(&field.at(index), Some(link)),
            _ => self.write(&field, Some(link)),
        }
        self.status = None;
    }

    fn disconnect(&mut self, node: &str, param: &str, index: Option<usize>) {
        let default = self
            .current()
            .and_then(|doc| doc.node(node))
            .and_then(|n| n.get("type").and_then(|t| t.as_str()).and_then(graph::schema))
            .and_then(|schema| schema.param(param))
            .map_or(0.0, |p| p.default as f64);
        let field = Field { target: Target::Node(node.into()), key: param.into(), index, kind: Kind::Text };
        match index {
            // De una lista se quita el elemento, salvo que sea el último.
            Some(_) if self.read(&Field { index: None, ..field.clone() }).and_then(|v| v.as_array().map(Vec::len)) > Some(1) => {
                self.write(&field, None)
            }
            _ => self.write(&field, Some(number(default))),
        }
    }

    fn delete_node(&mut self, name: &str) {
        self.pin_layout();
        if let Some(doc) = self.current_mut() {
            doc.remove_node(name);
        }
        if self.selected.as_deref() == Some(name) {
            self.selected = None;
        }
        self.changed();
    }

    /// Guarda en el archivo la posición de los nodos que aún no tienen una (los que se
    /// colocan solos en columnas). Se hace antes de cambiar algo del lienzo: si no, al
    /// mover un nodo (o al conectarlo) los demás de su columna se recolocarían.
    fn pin_layout(&mut self) {
        let loose: Vec<(String, [f32; 2])> = match self.current() {
            Some(doc) => {
                let nodes = self.graph_nodes();
                nodes.into_iter().filter(|node| doc.position(&node.name).is_none()).map(|node| (node.name, node.pos)).collect()
            }
            None => return,
        };
        if loose.is_empty() {
            return;
        }
        if let Some(doc) = self.current_mut() {
            for (name, pos) in loose {
                doc.set_position(&name, pos);
            }
        }
    }

    /// Posiciones de los nodos sin una guardada: en columnas según de cuántos dependen.
    fn auto_layout(&self, doc: &GenDoc, heights: &HashMap<String, f32>) -> HashMap<String, [f32; 2]> {
        let names = doc.node_names();
        let mut depth: HashMap<String, usize> = HashMap::new();
        fn depth_of(doc: &GenDoc, name: &str, memo: &mut HashMap<String, usize>, guard: usize) -> usize {
            if let Some(d) = memo.get(name) {
                return *d;
            }
            if guard > 64 {
                return 0;
            }
            let inputs = dependencies(doc, name);
            let direct: Vec<String> = doc
                .node(name)
                .map(|node| {
                    node.values()
                        .flat_map(|v| match v {
                            toml::Value::Array(items) => items.clone(),
                            other => vec![other.clone()],
                        })
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .filter(|n| n != name && inputs.contains(n))
                        .collect()
                })
                .unwrap_or_default();
            let d = direct.iter().map(|n| depth_of(doc, n, memo, guard + 1) + 1).max().unwrap_or(0);
            memo.insert(name.to_string(), d);
            d
        }
        // Debajo de los que ya tienen sitio, para no taparlos.
        let top = names
            .iter()
            .filter_map(|name| Some(doc.position(name)?[1] + heights.get(name).copied().unwrap_or(200.0)))
            .fold(None, |top: Option<f32>, bottom| Some(top.map_or(bottom, |top| top.max(bottom))))
            .map_or(0.0, |bottom| bottom + 80.0);
        let left = names.iter().filter_map(|name| doc.position(name)).map(|pos| pos[0]).fold(f32::INFINITY, f32::min);
        let left = if left.is_finite() { left } else { 0.0 };
        let mut columns: HashMap<usize, f32> = HashMap::new();
        let mut out = HashMap::new();
        for name in &names {
            let d = depth_of(doc, name, &mut depth, 0);
            if doc.position(name).is_some() {
                continue;
            }
            let y = columns.entry(d).or_insert(top);
            out.insert(name.clone(), [left + d as f32 * (NODE_WIDTH + 70.0), *y]);
            *y += heights.get(name).copied().unwrap_or(200.0) + 24.0;
        }
        out
    }

    /// Los nodos del archivo actual tal como los dibuja el lienzo.
    fn graph_nodes(&self) -> Vec<GraphNode> {
        let Some(doc) = self.current() else { return Vec::new() };
        let root = (doc.kind == DocKind::Biome).then(|| doc.get("height").map(|v| show(&v)).unwrap_or("height".into()));
        let mut nodes: Vec<GraphNode> = doc
            .node_names()
            .into_iter()
            .filter_map(|name| {
                let table = doc.node(&name)?;
                let kind = table.get("type").and_then(|t| t.as_str()).unwrap_or("?").to_string();
                let schema = graph::schema(&kind);
                let mut rows = Vec::new();
                let mut params = Vec::new();
                for param in schema.map_or(&[][..], |s| s.params) {
                    let value = table.get(param.name);
                    let link = |v: &toml::Value| v.as_str().map(str::to_string);
                    let label = |v: &toml::Value| match v.as_str() {
                        Some(node) if doc.has_node(node) => String::new(),
                        Some(node) => format!("← {node}"),
                        None => show(v),
                    };
                    match param.kind {
                        ParamKind::Input => {
                            let v = value.cloned().unwrap_or(number(param.default as f64));
                            let tail = label(&v);
                            rows.push(GraphRow {
                                label: if tail.is_empty() { param.name.into() } else { format!("{}: {tail}", param.name) },
                                param: param.name.into(),
                                index: None,
                                link: link(&v).filter(|n| doc.has_node(n)),
                            });
                        }
                        ParamKind::Inputs => {
                            let items = value.and_then(|v| v.as_array().cloned()).unwrap_or_default();
                            for (i, v) in items.iter().enumerate() {
                                let tail = label(v);
                                rows.push(GraphRow {
                                    label: if tail.is_empty() { format!("{} {}", param.name, i + 1) } else { tail },
                                    param: param.name.into(),
                                    index: Some(i),
                                    link: link(v).filter(|n| doc.has_node(n)),
                                });
                            }
                            rows.push(GraphRow { label: "+".into(), param: param.name.into(), index: Some(items.len()), link: None });
                        }
                        _ => {
                            if let Some(v) = value {
                                params.push(format!("{}: {}", param.name, show(v)));
                            }
                        }
                    }
                }
                let preview = self.previews.get(&name);
                let footer = preview.map_or(String::from("…"), |p| p.footer.clone());
                Some(GraphNode {
                    root: root.as_deref() == Some(name.as_str()),
                    error: preview.is_some_and(|p| p.error) || schema.is_none(),
                    thumbnail: preview.and_then(|p| p.handle.clone()),
                    pos: doc.position(&name).unwrap_or_default(),
                    size: doc.size(&name),
                    name,
                    kind,
                    rows,
                    params,
                    footer,
                })
            })
            .collect();
        let heights: HashMap<String, f32> = nodes.iter().map(|node| (node.name.clone(), node.height())).collect();
        let auto = self.auto_layout(doc, &heights);
        for node in &mut nodes {
            if let Some(pos) = auto.get(&node.name) {
                node.pos = *pos;
            }
        }
        nodes
    }

    fn view_of(&self, nodes: &[GraphNode]) -> ([f32; 2], f32) {
        self.views.get(&self.doc).copied().unwrap_or_else(|| fit_view(nodes))
    }

    /// El bioma que se edita (su identificador) y su perfil, si es un bioma.
    fn current_biome(&self) -> Option<(String, Option<String>)> {
        self.current().filter(|doc| doc.kind == DocKind::Biome).map(|doc| (doc.id.clone(), doc.profile.clone()))
    }

    /// Pasa a editar un perfil del bioma (o su archivo por defecto) y lo usa en el mundo.
    fn use_profile(&mut self, profile: Option<String>) {
        let Some((id, _)) = self.current_biome() else { return };
        let Some(workspace) = self.workspace.as_mut() else { return };
        match &profile {
            Some(name) => workspace.active.insert(id.clone(), name.clone()),
            None => workspace.active.remove(&id),
        };
        let key = workspace.active_key(&id);
        self.select_doc(key);
        self.requests.push(Request::Terrain(TerrainOp::Profile { biome: id, profile }));
        self.validate();
    }
}

/// Vista del lienzo con todos los nodos a la vista desde arriba a la izquierda.
fn fit_view(nodes: &[GraphNode]) -> ([f32; 2], f32) {
    let min_x = nodes.iter().map(|n| n.pos[0]).fold(f32::INFINITY, f32::min);
    let min_y = nodes.iter().map(|n| n.pos[1]).fold(f32::INFINITY, f32::min);
    if min_x.is_finite() { ([min_x - 30.0, min_y - 30.0], 0.75) } else { ([0.0, 0.0], 0.75) }
}

impl Screen for BiomeEditor {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {
            Message::Doc(doc) => self.select_doc(doc),
            Message::Tab(tab) => {
                self.tab = tab;
                self.texts.clear();
            }
            Message::Close => self.requests.push(Request::Close),
            Message::Save => self.save(),
            Message::Revert => {
                self.reload();
                self.status = Some(("Changes discarded: back to the files on disk.".into(), false));
            }
            Message::Auto(on) => self.auto_save = on,
            Message::Graph(event) => match event {
                GraphEvent::Select(name) => {
                    self.rename = name.clone().unwrap_or_default();
                    self.selected = name;
                    self.texts.clear();
                }
                GraphEvent::Move(name, pos) => {
                    self.pin_layout();
                    if let Some(doc) = self.current_mut() {
                        doc.set_position(&name, pos);
                    }
                    // Mover un nodo no cambia el terreno: no hace falta otra vista previa.
                    self.changed_at = Some(Instant::now());
                }
                GraphEvent::Resize(name, size) => {
                    self.pin_layout();
                    if let Some(doc) = self.current_mut() {
                        doc.set_size(&name, size);
                    }
                    self.changed_at = Some(Instant::now());
                }
                GraphEvent::Connect { from, to, param, index } => self.connect(&from, &to, &param, index),
                GraphEvent::Disconnect { node, param, index } => self.disconnect(&node, &param, index),
                GraphEvent::View(pan, zoom) => {
                    self.views.insert(self.doc.clone(), (pan, zoom.clamp(MIN_ZOOM, MAX_ZOOM)));
                }
                GraphEvent::Delete(name) => self.delete_node(&name),
            },
            Message::FitView => {
                let view = fit_view(&self.graph_nodes());
                self.views.insert(self.doc.clone(), view);
            }
            Message::AddNode(choice) => {
                self.pin_layout();
                let kind = choice.rsplit(' ').next().unwrap_or_default().to_string();
                let nodes = self.graph_nodes();
                let (pan, zoom) = self.view_of(&nodes);
                let pos = [pan[0] + 60.0 / zoom, pan[1] + 60.0 / zoom];
                let world: Vec<String> = match self.current() {
                    Some(doc) if doc.kind == DocKind::Biome => self.workspace.as_ref().map(|w| w.world.node_names()).unwrap_or_default(),
                    _ => Vec::new(),
                };
                let result = self.current_mut().map(|doc| doc.add_node(&kind, pos, |name| world.iter().any(|w| w == name)));
                match result {
                    Some(Ok(name)) => {
                        self.rename = name.clone();
                        self.selected = Some(name);
                        self.texts.clear();
                        self.changed();
                    }
                    Some(Err(error)) => self.error(format!("{error:#}")),
                    None => {}
                }
            }
            Message::DeleteNode => {
                if let Some(name) = self.selected.clone() {
                    self.delete_node(&name);
                }
            }
            Message::RenameText(text) => self.rename = text,
            Message::Rename => {
                self.pin_layout();
                let (Some(old), Some(workspace)) = (self.selected.clone(), self.workspace.as_mut()) else { return };
                let new = self.rename.trim().to_string();
                match workspace.rename_node(&self.doc, &old, &new) {
                    Ok(()) => {
                        self.selected = Some(new);
                        self.texts.clear();
                        self.status = None;
                        self.changed();
                    }
                    Err(error) => self.error(format!("{error:#}")),
                }
            }
            Message::ChangeType(kind) => {
                let Some(name) = self.selected.clone() else { return };
                let Some(schema) = graph::schema(&kind) else { return };
                let Some(doc) = self.current_mut() else { return };
                let Some(old) = doc.node(&name) else { return };
                // Se conservan las entradas que el tipo nuevo también tiene.
                let mut node = schema.new_node();
                for param in schema.params {
                    if let Some(value) = old.get(param.name) {
                        node.insert(param.name.into(), value.clone());
                    }
                }
                doc.replace_node(&name, &node);
                self.texts.clear();
                self.changed();
            }
            Message::Text(field, written) => {
                let parsed = parse(field.kind, &written);
                self.texts.insert(field.clone(), written);
                if let Ok(value) = parsed {
                    self.write(&field, value);
                }
            }
            Message::Pick(field, choice) => {
                self.texts.remove(&field);
                let value = match (choice.as_str(), field.kind) {
                    (NUMBER, Kind::Link(default)) => Some(number(default as f64 / 1000.0)),
                    (NONE | DEFAULT, _) => None,
                    _ => Some(toml::Value::String(choice)),
                };
                self.write(&field, value);
            }
            Message::Toggle(field, on) => self.write(&field, Some(toml::Value::Boolean(on))),
            Message::Set(field, value) => {
                self.texts.clear();
                self.write(&field, value);
            }
            Message::ListAdd(field) => {
                let element = match field.kind {
                    Kind::Link(default) => number(default as f64 / 1000.0),
                    _ => {
                        // Una lista de puntos: el siguiente, algo más allá del último.
                        let last = self.read(&Field { index: field.index.map(|i| i.saturating_sub(1)), ..field.clone() });
                        last.unwrap_or(toml::Value::Array(vec![number(0.0), number(0.0)]))
                    }
                };
                self.texts.clear();
                self.write(&field, Some(element));
            }
            Message::ListRemove(field) => {
                self.texts.clear();
                self.write(&field, None);
            }
            Message::EntryAdd(section) => {
                let mut entry = toml::Table::new();
                let first_block = || self.blocks().first().cloned().unwrap_or("base:stone".into());
                match section {
                    "surface" => {
                        entry.insert("top".into(), toml::Value::String(first_block()));
                    }
                    "trees" => {
                        entry.insert("density".into(), number(0.1));
                        entry.insert("wood".into(), toml::Value::String("base:wood".into()));
                        entry.insert("leaves".into(), toml::Value::Array(vec![toml::Value::String("base:leaves_green".into())]));
                    }
                    _ => {
                        entry.insert("density".into(), number(0.02));
                        entry.insert("block".into(), toml::Value::String("base:flower_red".into()));
                    }
                }
                if let Some(doc) = self.current_mut() {
                    doc.add_entry(section, &entry);
                }
                self.texts.clear();
                self.changed();
            }
            Message::EntryRemove(section, index) => {
                if let Some(doc) = self.current_mut() {
                    doc.remove_entry(section, index);
                }
                self.texts.clear();
                self.changed();
            }
            Message::EntryMove(section, index, delta) => {
                if let Some(doc) = self.current_mut() {
                    doc.move_entry(section, index, delta);
                }
                self.texts.clear();
                self.changed();
            }
            Message::Zoom(factor) => {
                self.window.span = (self.window.span * factor).clamp(SPANS.0, SPANS.1);
                self.preview_due = true;
            }
            Message::CenterOnPlayer => {
                self.window.center = [self.player[0], self.player[2]];
                self.preview_due = true;
            }
            Message::MapClick(point) => {
                self.window.center = point;
                self.preview_due = true;
            }
            Message::Teleport => {
                // Sobre el suelo de ese punto, según el terreno que se está editando.
                let [x, z] = self.window.center;
                let ground = self.workspace.as_ref().and_then(|w| w.source().ok()).and_then(|source| {
                    let terrain = Terrain::compile(&source, &self.content, self.seed, None).ok()?;
                    Some(terrain.height(x, z).max(SEA_LEVEL as f32) as f64)
                });
                let y = ground.map_or(300.0, |ground| ground + 1.0 + EYE_HEIGHT);
                self.requests.push(Request::Teleport([x, y, z]));
            }
            Message::Profile(choice) => {
                self.use_profile((choice != DEFAULT_PROFILE).then_some(choice));
            }
            Message::NewProfileText(text) => self.new_profile = text,
            Message::NewProfile => {
                let name = self.new_profile.trim().to_lowercase().replace([' ', '-'], "_");
                let from = self.doc.clone();
                let Some(workspace) = self.workspace.as_mut() else { return };
                match workspace.new_profile(&from, &name) {
                    Ok(_) => {
                        self.new_profile.clear();
                        self.use_profile(Some(name.clone()));
                        self.status = Some((format!("Profile \"{name}\" created as a copy: the world uses it now."), false));
                    }
                    Err(error) => self.error(format!("{error:#}")),
                }
            }
            Message::DeleteProfile => {
                let Some((id, Some(name))) = self.current_biome() else { return };
                if !self.confirm_delete {
                    self.confirm_delete = true;
                    return;
                }
                let deleted = self.workspace.as_mut().map(|workspace| workspace.delete_profile(&id, &name));
                match deleted {
                    Some(Ok(())) => {
                        self.views.remove(&format!("{id}@{name}"));
                        self.select_doc(id.clone());
                        self.requests.push(Request::Terrain(TerrainOp::Profile { biome: id, profile: None }));
                        self.validate();
                        self.status = Some((format!("Profile \"{name}\" deleted: the world uses the default file."), false));
                    }
                    Some(Err(error)) => self.error(format!("{error:#}")),
                    None => {}
                }
            }
            Message::LiveWorld(on) => {
                self.terrain.auto = on;
                self.requests.push(Request::Terrain(TerrainOp::Auto(on)));
            }
            Message::Apply => {
                if self.dirty() {
                    if self.check.is_err() {
                        self.error("Fix the errors first: the world would keep the previous terrain.");
                        return;
                    }
                    self.save();
                }
                self.requests.push(Request::Terrain(TerrainOp::Apply));
            }
            Message::NewBiomeText(text) => self.new_biome = text,
            Message::NewBiome => {
                let file = self.new_biome.trim().to_lowercase().replace([' ', '-'], "_");
                let Some(workspace) = self.workspace.as_mut() else { return };
                match workspace.new_biome(&file, self.new_biome.trim()) {
                    Ok(id) => {
                        self.new_biome.clear();
                        self.select_doc(id);
                        self.changed();
                    }
                    Err(error) => self.error(format!("{error:#}")),
                }
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let small = |label: &str| button(text(label.to_string()).size(13)).padding([4, 9]);
        let Some(workspace) = self.workspace.as_ref() else {
            let problem = self.status.as_ref().map_or("Opening the terrain files…".to_string(), |s| s.0.clone());
            return panel(column![
                text("Biome editor").size(22),
                text(problem).size(13).color(style::ERROR),
                small("Close").on_press(Message::Close)
            ]
            .spacing(12)
            .into());
        };

        // --- Barra de arriba: archivo, pestañas y guardar ---
        let mut docs: Vec<String> = vec!["world".into()];
        docs.extend(workspace.biomes.iter().map(|doc| doc.id.clone()));
        let doc_label = |id: &str| match workspace.doc(id) {
            Some(doc) if doc.kind == DocKind::World => format!("World (world.toml){}", if doc.dirty() { " *" } else { "" }),
            Some(doc) => {
                // Un bioma está "sin guardar" si lo está cualquiera de sus perfiles.
                let dirty = doc.dirty() || workspace.profiles.iter().any(|p| p.id == doc.id && p.dirty());
                format!("{}{} ({})", doc.title(), if dirty { " *" } else { "" }, doc.id)
            }
            None => id.to_string(),
        };
        let labels: Vec<String> = docs.iter().map(|id| doc_label(id)).collect();
        let current_id = self.current().map_or("world".to_string(), |doc| doc.id.clone());
        let selected_label = doc_label(&current_id);
        let doc_ids = docs.clone();
        let doc_picker = pick_list(labels.clone(), Some(selected_label), move |label: String| {
            let index = labels.iter().position(|l| *l == label).unwrap_or(0);
            Message::Doc(doc_ids[index].clone())
        })
        .text_size(13)
        .width(300);
        let is_biome = self.current().is_some_and(|doc| doc.kind == DocKind::Biome);
        let tabs: Vec<(Tab, &str)> = if is_biome {
            vec![(Tab::Nodes, "Nodes"), (Tab::Surface, "Surface"), (Tab::Vegetation, "Trees & plants"), (Tab::Settings, "Settings")]
        } else {
            vec![(Tab::Nodes, "Shared nodes"), (Tab::Settings, "World")]
        };
        let tab = if tabs.iter().any(|(t, _)| *t == self.tab) { self.tab } else { Tab::Nodes };
        let tab_buttons: Vec<Element<'_, Message>> = tabs
            .iter()
            .map(|(t, label)| {
                small(label).style(if *t == tab { button::primary } else { button::secondary }).on_press(Message::Tab(*t)).into()
            })
            .collect();
        let dirty = workspace.dirty();
        let top = row![
            text("Biomes").size(20),
            doc_picker,
            Row::with_children(tab_buttons).spacing(4),
            space::horizontal(),
            row![checkbox(self.auto_save).on_toggle(Message::Auto), text("Auto save").size(13)].spacing(4).align_y(Alignment::Center),
            small(if dirty { "Save *" } else { "Save" }).on_press_maybe(dirty.then_some(Message::Save)),
            small("Revert").style(button::secondary).on_press_maybe(dirty.then_some(Message::Revert)),
            small("Close").style(button::secondary).on_press(Message::Close),
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        // --- Segunda barra: perfil del bioma y cuándo cambia el mundo ---
        let mut second = Row::new().spacing(8).align_y(Alignment::Center);
        if let Some((id, profile)) = self.current_biome() {
            let mut options = vec![DEFAULT_PROFILE.to_string()];
            options.extend(workspace.profile_names(&id));
            let current = profile.clone().unwrap_or(DEFAULT_PROFILE.into());
            let delete_label = if self.confirm_delete { "Really delete?" } else { "Delete profile" };
            second = second
                .push(text("Profile").size(13))
                .push(pick_list(options, Some(current), Message::Profile).text_size(13).padding(4).width(180))
                .push(
                    text_input("new profile name", &self.new_profile)
                        .size(12)
                        .padding(4)
                        .width(160)
                        .on_input(Message::NewProfileText)
                        .on_submit(Message::NewProfile),
                )
                .push(
                    small("Save as new profile")
                        .style(button::secondary)
                        .on_press_maybe(valid_profile_name(&self.profile_input()).then_some(Message::NewProfile)),
                )
;
            if profile.is_some() {
                second = second.push(small(delete_label).style(button::danger).on_press(Message::DeleteProfile));
            }
        } else {
            second = second.push(text("world.toml is shared by every biome and profile.").size(12).color(style::MUTED));
        }
        let waiting = !self.terrain.auto && (self.terrain.pending || dirty);
        second = second
            .push(space::horizontal())
            .push(
                row![
                    checkbox(self.terrain.auto).on_toggle_maybe(self.hosting.then_some(Message::LiveWorld)),
                    text("Live world").size(13)
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .push(
                small(if waiting { "Apply to world *" } else { "Apply to world" })
                    .style(if waiting { button::primary } else { button::secondary })
                    .on_press_maybe((self.hosting && (waiting || dirty)).then_some(Message::Apply)),
            );

        let status: Element<'_, Message> = match (&self.status, &self.check) {
            (Some((text_, error)), _) => text(text_.clone()).size(12).color(if *error { style::ERROR } else { style::MUTED }).into(),
            (None, Err(error)) => text(error.clone()).size(12).color(style::ERROR).into(),
            (None, Ok(_)) if !self.hosting => {
                text("You are not the host: you can edit the files, but this world will not change.").size(12).color(style::ERROR).into()
            }
            (None, Ok(_)) if self.terrain.error.is_some() => {
                text(format!("Server: {}", self.terrain.error.clone().unwrap_or_default())).size(12).color(style::ERROR).into()
            }
            (None, Ok(_)) => text(match (self.terrain.auto, self.auto_save) {
                (false, _) if waiting => "Live world is off: the changes wait until you press Apply to world.",
                (false, _) => "Live world is off: saved changes wait until you press Apply to world.",
                (true, true) => "Changes are saved a moment after you stop, and the world regenerates around you.",
                (true, false) => "Save to see the changes in the world.",
            })
            .size(12)
            .color(style::MUTED)
            .into(),
        };

        let body: Element<'_, Message> = match tab {
            Tab::Nodes => self.nodes_tab(),
            Tab::Surface => self.surface_tab(),
            Tab::Vegetation => self.vegetation_tab(),
            Tab::Settings if is_biome => self.biome_settings(),
            Tab::Settings => self.world_settings(),
        };
        let side = column![self.map_panel(), self.side_panel(tab)].spacing(10).width(SIDE_WIDTH);
        let main = column![top, second, status, row![container(body).width(Length::Fill).height(Length::Fill), side].spacing(10)]
            .spacing(8)
            .height(Length::Fill);

        // La ventana entera es del editor: el mundo se ve en la del juego.
        container(main)
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::background(Color::from_rgb(0.09, 0.1, 0.12)))
            .into()
    }
}

/// Caja sencilla para los mensajes (sin archivos que editar).
fn panel(content: Element<'_, Message>) -> Element<'_, Message> {
    container(container(content).padding(20).style(container::rounded_box)).padding(20).into()
}

/// Una fila etiqueta + control.
fn labeled<'a>(label: impl Into<String>, control: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    row![text(label.into()).size(12).width(LABEL_WIDTH), control.into()].spacing(6).align_y(Alignment::Center).into()
}

fn card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .padding(10)
        .width(Length::Fill)
        .style(|_| container::background(Color::from_rgba(1.0, 1.0, 1.0, 0.05)).border(border::rounded(6)))
        .into()
}

impl BiomeEditor {
    /// Nombre de perfil tal como quedará escrito lo que hay en su caja.
    fn profile_input(&self) -> String {
        self.new_profile.trim().to_lowercase().replace([' ', '-'], "_")
    }

    /// Bloques del contenido, por identificador (los que valen para el terreno).
    fn blocks(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .content
            .numbered_states()
            .filter_map(|(block, _)| self.content.block(block))
            .filter(|def| def.model.is_none())
            .map(|def| def.id.clone())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    fn plants(&self) -> Vec<String> {
        let mut ids: Vec<String> =
            self.content.numbered_states().filter_map(|(block, _)| self.content.block(block)).map(|def| def.id.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    fn block_color(&self, id: &str) -> Option<Color> {
        let block = self.content.block_id(id)?;
        let [r, g, b] = self.content.block(block)?.faces.top;
        Some(Color::from_rgb8(r, g, b))
    }

    fn swatch(&self, id: &str) -> Element<'_, Message> {
        let color = self.block_color(id).unwrap_or(Color::TRANSPARENT);
        container(space()).width(14).height(14).style(move |_| container::background(color).border(border::rounded(3))).into()
    }

    /// Caja de texto de un campo.
    fn input(&self, field: Field, placeholder: &str, width: f32) -> Element<'_, Message> {
        let shown = self.shown(&field);
        let invalid = self.texts.contains_key(&field) && parse(field.kind, &shown).is_err();
        text_input(placeholder, &shown)
            .size(12)
            .padding(4)
            .width(width)
            .on_input(move |written| Message::Text(field.clone(), written))
            .style(move |theme, status| {
                let mut style = text_input::default(theme, status);
                if invalid {
                    style.border = style.border.color(style::ERROR);
                }
                style
            })
            .into()
    }

    /// Lista desplegable de un campo con nombres.
    fn picker(&self, field: Field, options: Vec<String>, width: f32) -> Element<'_, Message> {
        let selected = self.read(&field).and_then(|v| v.as_str().map(str::to_string)).or_else(|| match field.kind {
            Kind::Link(_) => Some(NUMBER.to_string()),
            Kind::OptName => Some(NONE.to_string()),
            Kind::Choice => Some(DEFAULT.to_string()),
            _ => None,
        });
        pick_list(options, selected, move |choice| Message::Pick(field.clone(), choice)).text_size(12).padding(4).width(width).into()
    }

    /// Una entrada que es un número o un nodo.
    fn link_input(&self, field: Field) -> Element<'_, Message> {
        let mut options = vec![NUMBER.to_string()];
        options.extend(self.usable_nodes().into_iter().filter(|name| Some(name) != self.selected.as_ref()));
        let is_node = self.read(&field).is_some_and(|v| v.is_str());
        let mut controls = Row::new().spacing(4).align_y(Alignment::Center);
        if !is_node {
            controls = controls.push(self.input(field.clone(), "0", 70.0));
        }
        controls.push(self.picker(field, options, if is_node { 170.0 } else { 120.0 })).into()
    }

    /// Una lista de nombres como fichas que se quitan, más un desplegable para añadir.
    fn chips(&self, field: Field, options: Vec<String>) -> Element<'_, Message> {
        let items = self.read(&field).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let mut chips = Row::new().spacing(4).align_y(Alignment::Center);
        for (index, item) in items.iter().enumerate() {
            let name = show(item);
            chips = chips.push(
                button(row![self.swatch(&name), text(name.clone()).size(11)].spacing(4).align_y(Alignment::Center))
                    .padding([2, 6])
                    .style(button::secondary)
                    .on_press(Message::ListRemove(field.clone().at(index))),
            );
        }
        let add = field.clone().at(items.len());
        chips = chips.push(pick_list(options, None::<String>, move |choice| Message::Pick(add.clone(), choice)).placeholder("add…").text_size(11).padding(3).width(120));
        scrollable(chips).direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new().width(3).scroller_width(3))).into()
    }

    fn map_panel(&self) -> Element<'_, Message> {
        let small = |label: &str| button(text(label.to_string()).size(12)).padding([3, 8]).style(button::secondary);
        let map = canvas_widget(MapView {
            handle: self.map.as_ref(),
            window: self.map_window,
            next: self.window,
            player: [self.player[0], self.player[2]],
        })
        .width(MAP_SIZE as f32)
        .height(MAP_SIZE as f32);
        let mut panel = column![
            map,
            row![
                small("−").on_press(Message::Zoom(2.0)),
                small("+").on_press(Message::Zoom(0.5)),
                small("Me").on_press(Message::CenterOnPlayer),
                small("Go there").on_press_maybe(self.hosting.then_some(Message::Teleport)),
                text(format!("{} m", show_number(self.window.span))).size(12).color(style::MUTED),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        ]
        .spacing(4);
        let note = match &self.map_error {
            Some(error) => text(error.clone()).size(11).color(style::ERROR),
            None => text(format!("Click to look elsewhere · {} ms", self.preview_time.as_millis())).size(11).color(style::MUTED),
        };
        panel = panel.push(note);
        panel.into()
    }

    fn side_panel(&self, tab: Tab) -> Element<'_, Message> {
        if tab != Tab::Nodes {
            let help = match tab {
                Tab::Surface => "The first rule that matches a column gives its blocks. Conditions left empty always match. Slope is meters of rise per meter (1 = 45°).",
                Tab::Vegetation => "Density is the fraction of 7 × 7 m cells with a tree (or of columns with a plant): a number, or a node to make groves. Several rules add up.",
                _ => "",
            };
            return text(help).size(12).color(style::MUTED).into();
        }
        let Some(name) = self.selected.clone() else {
            return text("Drag from an output (right) to an input (left) to connect two nodes. Drag a wire end away to unplug it. Drag the corner of a box to make it bigger (double-click it to undo). Right-drag or drag the background to move, wheel to zoom, Delete removes the selected node.")
                .size(12)
                .color(style::MUTED)
                .into();
        };
        let Some(node) = self.current().and_then(|doc| doc.node(&name)) else { return space().into() };
        let kind = node.get("type").and_then(|t| t.as_str()).unwrap_or("?").to_string();
        let target = Target::Node(name.clone());
        let mut fields = column![
            row![
                text_input("name", &self.rename).size(13).padding(4).on_input(Message::RenameText).on_submit(Message::Rename),
                button(text("Rename").size(12)).padding([4, 8]).on_press_maybe((self.rename.trim() != name).then_some(Message::Rename)),
            ]
            .spacing(4),
            labeled(
                "Type",
                pick_list(NODE_TYPES.iter().map(|s| s.kind.to_string()).collect::<Vec<_>>(), Some(kind.clone()), Message::ChangeType)
                    .text_size(12)
                    .padding(4)
                    .width(Length::Fill)
            ),
        ]
        .spacing(6);
        if let Some(schema) = graph::schema(&kind) {
            fields = fields.push(text(schema.help).size(11).color(style::MUTED));
            for param in schema.params {
                let field = |kind: Kind| Field::new(target.clone(), param.name, kind);
                let link_default = (param.default * 1000.0) as i32;
                let control: Element<'_, Message> = match param.kind {
                    ParamKind::Input => self.link_input(field(Kind::Link(link_default))),
                    ParamKind::Inputs => {
                        let items = self.read(&field(Kind::Text)).and_then(|v| v.as_array().map(Vec::len)).unwrap_or(0);
                        let mut list = Column::new().spacing(3);
                        for index in 0..items {
                            list = list.push(
                                row![
                                    self.link_input(field(Kind::Link(link_default)).at(index)),
                                    button(text("×").size(12)).padding([2, 6]).style(button::secondary).on_press_maybe(
                                        (items > 1).then_some(Message::ListRemove(field(Kind::Link(link_default)).at(index)))
                                    ),
                                ]
                                .spacing(3)
                                .align_y(Alignment::Center),
                            );
                        }
                        list.push(button(text("+ input").size(11)).padding([2, 8]).style(button::secondary).on_press(Message::ListAdd(field(Kind::Link(link_default)).at(items)))).into()
                    }
                    ParamKind::Number => self.input(field(if param.required { Kind::Number } else { Kind::OptNumber }), &show_number(param.default as f64), 110.0),
                    ParamKind::Integer | ParamKind::Seed => {
                        let hint = if param.kind == ParamKind::Seed { "by name".to_string() } else { show_number(param.default as f64) };
                        self.input(field(if param.required { Kind::Integer } else { Kind::OptInteger }), &hint, 110.0)
                    }
                    ParamKind::Scale => self.input(field(Kind::Numbers), "200 or 200, 600", 140.0),
                    ParamKind::Choice(options) => {
                        let mut list: Vec<String> = options.iter().map(|o| o.to_string()).collect();
                        if !param.required {
                            list.insert(0, DEFAULT.into());
                        }
                        self.picker(field(Kind::Choice), list, 140.0)
                    }
                    ParamKind::Bool => {
                        let on = self.read(&field(Kind::Text)).and_then(|v| v.as_bool()).unwrap_or(false);
                        let f = field(Kind::Text);
                        checkbox(on).on_toggle(move |on| Message::Toggle(f.clone(), on)).into()
                    }
                    ParamKind::Points => {
                        let count = self.read(&field(Kind::Text)).and_then(|v| v.as_array().map(Vec::len)).unwrap_or(0);
                        let mut list = Column::new().spacing(3);
                        for index in 0..count {
                            list = list.push(
                                row![
                                    self.input(field(Kind::Numbers).at(index), "in, out", 120.0),
                                    button(text("×").size(12)).padding([2, 6]).style(button::secondary).on_press_maybe(
                                        (count > 1).then_some(Message::ListRemove(field(Kind::Numbers).at(index)))
                                    ),
                                ]
                                .spacing(3),
                            );
                        }
                        list.push(button(text("+ point").size(11)).padding([2, 8]).style(button::secondary).on_press(Message::ListAdd(field(Kind::Numbers).at(count)))).into()
                    }
                };
                fields = fields.push(labeled(param.name, control));
            }
        }
        if let Some(preview) = self.previews.get(&name).filter(|p| p.error) {
            fields = fields.push(text(preview.footer.clone()).size(11).color(style::ERROR));
        }
        let users = self.current().map(|doc| doc.users(&name)).unwrap_or_default();
        fields = fields.push(
            text(if users.is_empty() { "Not used by any node.".to_string() } else { format!("Used by: {}", users.join(", ")) })
                .size(11)
                .color(style::MUTED),
        );
        let resized = self.current().is_some_and(|doc| doc.size(&name).is_some());
        fields = fields.push(
            row![
                button(text("Normal box size").size(12))
                    .padding([4, 10])
                    .style(button::secondary)
                    .on_press_maybe(resized.then(|| Message::Graph(GraphEvent::Resize(name.clone(), None)))),
                button(text("Delete node").size(12)).padding([4, 10]).style(button::danger).on_press(Message::DeleteNode),
            ]
            .spacing(6),
        );
        scrollable(fields.padding([0, 8])).height(Length::Fill).into()
    }

    fn nodes_tab(&self) -> Element<'_, Message> {
        let nodes = self.graph_nodes();
        let (pan, zoom) = self.view_of(&nodes);
        let kinds: Vec<String> = NODE_TYPES.iter().map(|s| format!("{} · {}", s.group, s.kind)).collect();
        let toolbar = row![
            pick_list(kinds, None::<String>, Message::AddNode).placeholder("Add node…").text_size(13).padding(4).width(190),
            button(text("Fit").size(12)).padding([4, 9]).style(button::secondary).on_press(Message::FitView),
            text(match self.current() {
                Some(doc) if doc.kind == DocKind::Biome => {
                    let root = doc.get("height").map(|v| show(&v)).unwrap_or("height".into());
                    format!("The terrain height comes out of \"{root}\" (gold). Shared nodes of world.toml can be used by name.")
                }
                _ => "Shared nodes: every biome can use them by name (the coast and the mountains, so biomes match).".to_string(),
            })
            .size(11)
            .color(style::MUTED),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        let graph = canvas_widget(NodeGraph { nodes, selected: self.selected.clone(), pan, zoom })
            .width(Length::Fill)
            .height(Length::Fill);
        column![toolbar, Element::from(graph).map(Message::Graph)].spacing(6).into()
    }

    fn entry_header(&self, section: &'static str, index: usize, count: usize, title: String) -> Element<'_, Message> {
        let small = |label: &str| button(text(label.to_string()).size(12)).padding([2, 7]).style(button::secondary);
        row![
            text(title).size(13),
            space::horizontal(),
            small("↑").on_press_maybe((index > 0).then_some(Message::EntryMove(section, index, -1))),
            small("↓").on_press_maybe((index + 1 < count).then_some(Message::EntryMove(section, index, 1))),
            small("×").on_press(Message::EntryRemove(section, index)),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
    }

    fn surface_tab(&self) -> Element<'_, Message> {
        let entries = self.current().map(|doc| doc.entries("surface")).unwrap_or_default();
        let blocks = self.blocks();
        let mut optional_blocks = vec![NONE.to_string()];
        optional_blocks.extend(blocks.iter().cloned());
        let mut nodes = vec![NONE.to_string()];
        nodes.extend(self.usable_nodes());
        let mut list = Column::new().spacing(8);
        for (index, _) in entries.iter().enumerate() {
            let target = Target::Entry("surface", index);
            let f = |key: &str, kind: Kind| Field::new(target.clone(), key, kind);
            let top = self.read(&f("top", Kind::Text)).map(|v| show(&v)).unwrap_or_default();
            let title = format!("{}. {}", index + 1, top);
            let conditions = row![
                text("height ≥").size(12),
                self.input(f("above", Kind::OptNumber), "", 60.0),
                text("<").size(12),
                self.input(f("below", Kind::OptNumber), "", 60.0),
                text("slope ≥").size(12),
                self.input(f("steeper", Kind::OptNumber), "", 50.0),
                text("<").size(12),
                self.input(f("flatter", Kind::OptNumber), "", 50.0),
                text("jitter").size(12),
                self.input(f("jitter", Kind::OptNumber), "2", 44.0),
            ]
            .spacing(5)
            .align_y(Alignment::Center);
            let node = row![
                text("node").size(12),
                self.picker(f("node", Kind::OptName), nodes.clone(), 150.0),
                text("≥").size(12),
                self.input(f("node_above", Kind::OptNumber), "", 60.0),
                text("<").size(12),
                self.input(f("node_below", Kind::OptNumber), "", 60.0),
            ]
            .spacing(5)
            .align_y(Alignment::Center);
            let under = self.read(&f("under", Kind::Text)).map(|v| show(&v)).unwrap_or(top.clone());
            let blocks_row = row![
                text("top").size(12),
                self.swatch(&top),
                self.picker(f("top", Kind::Text), blocks.clone(), 170.0),
                text("under").size(12),
                self.swatch(&under),
                self.picker(f("under", Kind::OptName), optional_blocks.clone(), 170.0),
                text("depth").size(12),
                self.input(f("depth", Kind::OptInteger), "3", 44.0),
            ]
            .spacing(5)
            .align_y(Alignment::Center);
            list = list.push(card(column![self.entry_header("surface", index, entries.len(), title), conditions, node, blocks_row].spacing(6)));
        }
        list = list.push(button(text("+ Surface rule").size(12)).padding([4, 10]).on_press(Message::EntryAdd("surface")));
        scrollable(list.padding([0, 10])).height(Length::Fill).into()
    }

    fn vegetation_tab(&self) -> Element<'_, Message> {
        let doc_entries = |section: &str| self.current().map(|doc| doc.entries(section)).unwrap_or_default();
        let blocks = self.blocks();
        let plants = self.plants();
        let mut list = Column::new().spacing(8).push(text("Trees").size(16));
        let trees = doc_entries("trees");
        for index in 0..trees.len() {
            let target = Target::Entry("trees", index);
            let f = |key: &str, kind: Kind| Field::new(target.clone(), key, kind);
            let shape = self.read(&f("shape", Kind::Text)).map(|v| show(&v)).unwrap_or("round".into());
            let body = column![
                self.entry_header("trees", index, trees.len(), format!("{}. {shape}", index + 1)),
                labeled("Density", self.link_input(f("density", Kind::Link(100)))),
                labeled(
                    "Shape",
                    self.picker(f("shape", Kind::Choice), [DEFAULT, "round", "pine", "bush", "pillar"].map(String::from).into(), 140.0)
                ),
                labeled("Trunk", row![self.input(f("trunk", Kind::Numbers), "4, 6", 90.0), text("min, max height").size(11).color(style::MUTED)].spacing(6).align_y(Alignment::Center)),
                labeled("Wood", row![self.swatch(&self.read(&f("wood", Kind::Text)).map(|v| show(&v)).unwrap_or_default()), self.picker(f("wood", Kind::Text), blocks.clone(), 170.0)].spacing(4).align_y(Alignment::Center)),
                labeled("Leaves", self.chips(f("leaves", Kind::Text), blocks.clone())),
                labeled("Grows on", self.chips(f("on", Kind::Text), blocks.clone())),
            ]
            .spacing(5);
            list = list.push(card(body));
        }
        list = list.push(button(text("+ Tree").size(12)).padding([4, 10]).on_press(Message::EntryAdd("trees")));
        list = list.push(text("Plants").size(16));
        let plant_entries = doc_entries("plants");
        for index in 0..plant_entries.len() {
            let target = Target::Entry("plants", index);
            let f = |key: &str, kind: Kind| Field::new(target.clone(), key, kind);
            let block = self.read(&f("block", Kind::Text)).map(|v| show(&v)).unwrap_or_default();
            let body = column![
                self.entry_header("plants", index, plant_entries.len(), format!("{}. {block}", index + 1)),
                labeled("Density", self.link_input(f("density", Kind::Link(20)))),
                labeled("Block", self.picker(f("block", Kind::Text), plants.clone(), 190.0)),
                labeled("Grows on", self.chips(f("on", Kind::Text), blocks.clone())),
            ]
            .spacing(5);
            list = list.push(card(body));
        }
        list = list.push(button(text("+ Plant").size(12)).padding([4, 10]).on_press(Message::EntryAdd("plants")));
        scrollable(list.padding([0, 10])).height(Length::Fill).into()
    }

    fn biome_settings(&self) -> Element<'_, Message> {
        let f = |key: &str, kind: Kind| Field::new(Target::Doc, key, kind);
        let color = self.read(&f("color", Kind::Text)).map(|v| show(&v)).unwrap_or_default();
        let swatch_color = crate::environment::parse_hex(&color).map(|c| Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)).unwrap_or(Color::TRANSPARENT);
        let nodes = self.usable_nodes();
        let body = column![
            text("Biome").size(16),
            labeled("Name", self.input(f("name", Kind::Text), "shown name", 240.0)),
            labeled(
                "Map color",
                row![
                    self.input(f("color", Kind::Text), "#RRGGBB", 100.0),
                    container(space()).width(18).height(18).style(move |_| container::background(swatch_color).border(border::rounded(3)))
                ]
                .spacing(6)
                .align_y(Alignment::Center)
            ),
            labeled("Height node", {
                // Sin `height` en el archivo, la altura sale del nodo que se llama así.
                let field = f("height", Kind::Text);
                let current = self.read(&field).map(|v| show(&v)).unwrap_or("height".into());
                pick_list(nodes, Some(current), move |choice| Message::Pick(field.clone(), choice))
                    .text_size(12)
                    .padding(4)
                    .width(200)
            }),
            text(format!("File: {}", self.current().map(|d| d.path.display().to_string()).unwrap_or_default())).size(11).color(style::MUTED),
        ]
        .spacing(8);
        scrollable(card(body)).into()
    }

    fn world_settings(&self) -> Element<'_, Message> {
        let f = |key: &str, kind: Kind| Field::new(Target::Doc, key, kind);
        let blocks = self.blocks();
        let order = self.read(&f("biomes", Kind::Text)).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let mut bands = Column::new().spacing(3);
        for (index, name) in order.iter().enumerate() {
            let name = show(name);
            let field = f("biomes", Kind::Text);
            let small = |label: &str| button(text(label.to_string()).size(11)).padding([2, 6]).style(button::secondary);
            let swap = |delta: i32| {
                let mut moved = order.clone();
                let target = index as i32 + delta;
                (target >= 0 && (target as usize) < moved.len()).then(|| {
                    moved.swap(index, target as usize);
                    Message::Set(field.clone(), Some(toml::Value::Array(moved)))
                })
            };
            bands = bands.push(
                row![
                    text(format!("{}. {name}", index + 1)).size(12).width(200),
                    small("↑").on_press_maybe(swap(-1)),
                    small("↓").on_press_maybe(swap(1)),
                    small("×").on_press(Message::ListRemove(field.clone().at(index))),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            );
        }
        let available: Vec<String> = self
            .workspace
            .as_ref()
            .map(|w| w.biomes.iter().filter_map(|doc| doc.id.split_once(':').map(|(_, n)| n.to_string())).collect())
            .unwrap_or_default();
        let add = f("biomes", Kind::Text).at(order.len());
        bands = bands.push(pick_list(available, None::<String>, move |name| Message::Pick(add.clone(), name)).placeholder("add a biome…").text_size(12).padding(4).width(200));
        let body = column![
            text("World").size(16),
            labeled("Width", self.input(f("width", Kind::Integer), "5120", 90.0)),
            labeled("Length", self.input(f("length", Kind::Integer), "32768", 90.0)),
            labeled("Blend", self.input(f("blend", Kind::OptNumber), "400", 90.0)),
            labeled("Border wobble", self.input(f("border_wobble", Kind::OptNumber), "300", 90.0)),
            labeled("Wobble size", self.input(f("border_scale", Kind::OptNumber), "1400", 90.0)),
            labeled("Stone", self.picker(f("stone", Kind::Text), blocks.clone(), 180.0)),
            labeled("Water", self.picker(f("water", Kind::Text), blocks, 180.0)),
            text("Biomes from north to south").size(14),
            bands,
            row![
                text_input("new biome name", &self.new_biome).size(12).padding(4).width(200).on_input(Message::NewBiomeText).on_submit(Message::NewBiome),
                button(text("Create").size(12)).padding([4, 10]).on_press_maybe((!self.new_biome.trim().is_empty()).then_some(Message::NewBiome)),
            ]
            .spacing(6),
            text("A new biome starts as grassland over the shared coast and mountains, at the end of the list.").size(11).color(style::MUTED),
        ]
        .spacing(8);
        scrollable(card(body)).into()
    }
}

/// El mapa de la vista previa: la imagen, la ventana que viene y el jugador. Un clic
/// centra la ventana en ese punto.
struct MapView<'a> {
    handle: Option<&'a image::Handle>,
    /// Ventana de la imagen que se tiene y de la que se ha pedido.
    window: Window,
    next: Window,
    player: [f64; 2],
}

impl canvas::Program<Message, Theme, Renderer> for MapView<'_> {
    type State = ();

    fn update(&self, _: &mut (), event: &Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<Message>> {
        let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event else { return None };
        let point = cursor.position_in(bounds)?;
        let w = &self.window;
        let x = w.center[0] - w.span / 2.0 + point.x as f64 / bounds.width as f64 * w.span;
        let z = w.center[1] - w.span / 2.0 + point.y as f64 / bounds.height as f64 * w.span;
        Some(canvas::Action::publish(Message::MapClick([x, z])).and_capture())
    }

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let area = Rectangle::with_size(bounds.size());
        match self.handle {
            Some(handle) => frame.draw_image(area, canvas::Image::new(handle.clone()).filter_method(image::FilterMethod::Nearest)),
            None => frame.fill_rectangle(Point::ORIGIN, area.size(), Color::from_rgb(0.1, 0.1, 0.12)),
        }
        let to_screen = |x: f64, z: f64| {
            let w = &self.window;
            Point::new(
                ((x - w.center[0]) / w.span + 0.5) as f32 * area.width,
                ((z - w.center[1]) / w.span + 0.5) as f32 * area.height,
            )
        };
        // La ventana pedida, si aún no ha llegado su mapa.
        if self.next != self.window {
            let a = to_screen(self.next.center[0] - self.next.span / 2.0, self.next.center[1] - self.next.span / 2.0);
            let b = to_screen(self.next.center[0] + self.next.span / 2.0, self.next.center[1] + self.next.span / 2.0);
            let rect = Path::rectangle(a, Size::new(b.x - a.x, b.y - a.y));
            frame.stroke(&rect, Stroke::default().with_color(Color::WHITE).with_width(1.0));
        }
        let player = to_screen(self.player[0], self.player[1]);
        if area.contains(player) {
            frame.fill(&Path::circle(player, 5.0), Color::from_rgb(1.0, 0.25, 0.2));
            frame.stroke(&Path::circle(player, 5.0), Stroke::default().with_color(Color::WHITE).with_width(1.5));
        }
        vec![frame.into_geometry()]
    }
}


#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::biome_preview;

    fn workspace_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let (source, target) = (entry.path(), to.join(entry.file_name()));
            if source.is_dir() {
                copy_dir(&source, &target);
            } else {
                std::fs::copy(&source, &target).unwrap();
            }
        }
    }

    /// El editor sobre una copia del contenido del juego, en el punto de aparición.
    struct Fixture {
        editor: BiomeEditor,
        dir: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            Self::prepared(name, |_| {})
        }

        /// Como `new`, pero deja retocar la copia del contenido antes de abrir el editor.
        fn prepared(name: &str, prepare: impl FnOnce(&std::path::Path)) -> Self {
            let dir = std::env::temp_dir().join(format!("tokikura-biome-editor-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            copy_dir(&workspace_dir().join("content"), &dir);
            prepare(&dir);
            let content = kuravoxel_common::content::load(&dir)
                .and_then(|loaded| ContentData::assign(loaded, &[], &[]))
                .expect("content loads");
            let mut editor = BiomeEditor::default();
            editor.open(dir.clone(), 1337);
            editor.sync(&Arc::new(content), [0.0, 30.0, 0.0], true);
            Self { editor, dir }
        }

        fn doc(&self) -> &GenDoc {
            self.editor.current().expect("a file is open")
        }

        /// Calcula la vista previa pendiente aquí mismo (sin hilo).
        fn preview(&mut self) {
            let job = self.editor.take_job().expect("a preview is due");
            self.editor.preview_done(biome_preview::run(&job));
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn node_field(node: &str, key: &str, kind: Kind) -> Field {
        Field::new(Target::Node(node.into()), key, kind)
    }

    #[test]
    fn it_opens_on_the_biome_under_the_player_with_previews() {
        let mut fixture = Fixture::new("open");
        assert_eq!(fixture.editor.doc, "base:plains");
        assert!(fixture.editor.window.center[1].abs() < 2000.0, "the window is over the plains band");
        fixture.preview();
        let preview = &fixture.editor.previews["flat_noise"];
        assert!(preview.handle.is_some() && !preview.error, "{}", preview.footer);
        assert!(fixture.editor.map.is_some());
        assert!(fixture.editor.take_job().is_none(), "nothing changed since");
        // Todas las pestañas y el mundo se dibujan.
        for tab in [Tab::Nodes, Tab::Surface, Tab::Vegetation, Tab::Settings] {
            fixture.editor.update(Message::Tab(tab));
            let _ = fixture.editor.view();
        }
        fixture.editor.update(Message::Graph(GraphEvent::Select(Some("flat_noise".into()))));
        let _ = fixture.editor.view();
        fixture.editor.update(Message::Doc("world".into()));
        for tab in [Tab::Nodes, Tab::Settings] {
            fixture.editor.update(Message::Tab(tab));
            let _ = fixture.editor.view();
        }
        fixture.preview();
        assert!(fixture.editor.previews.contains_key("cordillera"));
    }

    #[test]
    fn wires_connect_and_disconnect_nodes_without_loops() {
        let mut fixture = Fixture::new("wires");
        let editor = &mut fixture.editor;
        let inputs = |editor: &BiomeEditor| editor.read(&node_field("woods", "inputs", Kind::Text)).unwrap();
        let before = inputs(editor).as_array().unwrap().len();
        editor.update(Message::Graph(GraphEvent::Connect {
            from: "flat_noise".into(),
            to: "woods".into(),
            param: "inputs".into(),
            index: Some(before),
        }));
        let after = inputs(editor);
        assert_eq!(after.as_array().unwrap().last().unwrap().as_str(), Some("flat_noise"));
        editor.update(Message::Graph(GraphEvent::Disconnect {
            node: "woods".into(),
            param: "inputs".into(),
            index: Some(before),
        }));
        assert_eq!(inputs(editor).as_array().unwrap().len(), before);

        // `height` depende de `flat_part`: conectarlo dentro sería un bucle.
        let text = fixture.doc().text();
        fixture.editor.update(Message::Graph(GraphEvent::Connect {
            from: "height".into(),
            to: "flat_part".into(),
            param: "inputs".into(),
            index: Some(0),
        }));
        assert_eq!(fixture.doc().text(), text);
        assert!(fixture.editor.status.as_ref().is_some_and(|(s, error)| *error && s.contains("loop")));

        // Una entrada sencilla: conectar y soltar vuelve a su número inicial.
        fixture.editor.update(Message::Graph(GraphEvent::Connect {
            from: "flat_noise".into(),
            to: "river_line".into(),
            param: "input".into(),
            index: None,
        }));
        let input = node_field("river_line", "input", Kind::Text);
        assert_eq!(fixture.editor.read(&input).unwrap().as_str(), Some("flat_noise"));
        fixture.editor.update(Message::Graph(GraphEvent::Disconnect {
            node: "river_line".into(),
            param: "input".into(),
            index: None,
        }));
        assert_eq!(fixture.editor.read(&input), Some(toml::Value::Integer(0)));
    }

    #[test]
    fn fields_nodes_and_rules_edit_the_file_and_auto_save() {
        let mut fixture = Fixture::new("fields");
        let path = fixture.doc().path.clone();
        let editor = &mut fixture.editor;
        // Un número a medias no cambia nada; uno entero, sí.
        let scale = node_field("flat_noise", "scale", Kind::Numbers);
        let original = editor.read(&scale);
        editor.update(Message::Text(scale.clone(), "300,".into()));
        assert_eq!(editor.read(&scale), original);
        editor.update(Message::Text(scale.clone(), "300, 1200".into()));
        assert_eq!(editor.read(&scale).unwrap(), toml::Value::Array(vec![number(300.0), number(1200.0)]));
        let octaves = node_field("flat_noise", "octaves", Kind::OptInteger);
        editor.update(Message::Text(octaves.clone(), String::new()));
        assert!(editor.read(&octaves).is_none(), "an empty optional field is removed");

        // Nodo nuevo, renombrado, cambiado de tipo y borrado.
        editor.update(Message::AddNode("Shapes · ridge".into()));
        assert_eq!(editor.selected.as_deref(), Some("ridge"));
        editor.update(Message::RenameText("crests".into()));
        editor.update(Message::Rename);
        assert!(editor.current().unwrap().has_node("crests"));
        editor.update(Message::ChangeType("noise".into()));
        assert_eq!(editor.current().unwrap().node("crests").unwrap()["type"].as_str(), Some("noise"));
        editor.update(Message::RenameText("hills".into()));
        editor.update(Message::Rename);
        assert!(editor.status.as_ref().is_some_and(|(_, error)| *error), "hills is a shared node");
        editor.update(Message::DeleteNode);
        assert!(!editor.current().unwrap().has_node("crests"));

        // Reglas: se añade una al final y se sube.
        let rules = editor.current().unwrap().entries("surface").len();
        editor.update(Message::EntryAdd("surface"));
        let top = Field::new(Target::Entry("surface", rules), "top", Kind::Text);
        editor.update(Message::Pick(top, "base:salt".into()));
        editor.update(Message::EntryMove("surface", rules, -1));
        assert_eq!(editor.current().unwrap().entries("surface")[rules - 1]["top"].as_str(), Some("base:salt"));

        // Con "Auto" se guarda un momento después del último cambio.
        assert!(editor.dirty());
        editor.tick(Instant::now());
        assert!(editor.dirty(), "not yet");
        editor.tick(Instant::now() + AUTO_SAVE_DELAY);
        assert!(!editor.dirty());
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("scale = [300, 1200]") && saved.contains("base:salt"), "{saved}");
        assert!(saved.contains("# Planicies: el valle central"), "the comments stay");
    }

    #[test]
    fn broken_files_are_not_saved_alone_and_revert_restores_them() {
        let mut fixture = Fixture::new("broken");
        let editor = &mut fixture.editor;
        editor.update(Message::Pick(node_field("flat", "inputs", Kind::Link(0)).at(0), "nowhere".into()));
        assert!(editor.dirty());
        assert!(editor.check.as_ref().is_err_and(|error| error.contains("nowhere")), "{:?}", editor.check);
        assert!(editor.take_job().is_some(), "the other nodes still get previews");
        editor.tick(Instant::now() + AUTO_SAVE_DELAY);
        assert!(editor.dirty(), "a file with errors is not saved by itself");
        editor.update(Message::Revert);
        assert!(!editor.dirty());

        // Ir al centro de la vista previa: sobre el suelo de ese punto.
        editor.update(Message::Teleport);
        let requests = editor.take_requests();
        let [Request::Teleport([x, y, z])] = requests[..] else { panic!("{requests:?}") };
        assert_eq!([x, z], editor.window.center);
        assert!(y > 1.0 && y < 200.0, "{y}");
    }

    fn positions(editor: &BiomeEditor) -> HashMap<String, [f32; 2]> {
        editor.graph_nodes().into_iter().map(|node| (node.name, node.pos)).collect()
    }

    #[test]
    fn moving_a_node_leaves_the_others_where_they_were() {
        // Unos nodos con posición guardada y otros colocados solos: se quitan del archivo las
        // posiciones de algunos, porque el de verdad puede tenerlas ya todas.
        let mut fixture = Fixture::prepared("layout", |dir| {
            let path = dir.join("base/worldgen/biomes/plains.toml");
            let text = std::fs::read_to_string(&path).unwrap();
            let loose = ["grove", "grove_mask", "ground", "woods"];
            let kept: Vec<&str> = text
                .lines()
                .filter(|line| !loose.iter().any(|name| line.starts_with(&format!("{name} = ["))))
                .collect();
            std::fs::write(&path, kept.join("\n") + "\n").unwrap();
        });
        let editor = &mut fixture.editor;
        let doc = editor.current().unwrap();
        let loose: Vec<String> = doc.node_names().into_iter().filter(|name| doc.position(name).is_none()).collect();
        assert!(!loose.is_empty() && doc.position("flat_noise").is_some(), "{loose:?}");
        let before = positions(editor);
        let view = editor.views[&editor.doc];

        // Se mueve el nodo de más arriba a la izquierda, y uno de los colocados solos.
        editor.update(Message::Graph(GraphEvent::Move("flat_noise".into(), [2000.0, 2000.0])));
        editor.update(Message::Graph(GraphEvent::Move(loose[0].clone(), [2400.0, 2000.0])));
        let after = positions(editor);
        for (name, pos) in &before {
            if name != "flat_noise" && *name != loose[0] {
                assert_eq!(after[name], *pos, "{name} stays where it was");
            }
        }
        assert_eq!(after["flat_noise"], [2000.0, 2000.0]);
        assert_eq!(editor.views[&editor.doc], view, "the canvas does not jump");
        // Conectar dos nodos tampoco recoloca nada.
        editor.update(Message::Graph(GraphEvent::Connect {
            from: "flat_noise".into(),
            to: "river_line".into(),
            param: "input".into(),
            index: None,
        }));
        assert_eq!(positions(editor), after);
        assert!(loose.iter().all(|name| editor.current().unwrap().position(name).is_some()), "every node has its place now");
    }

    #[test]
    fn node_boxes_keep_the_size_they_are_given() {
        let mut fixture = Fixture::new("resize");
        let editor = &mut fixture.editor;
        editor.update(Message::Graph(GraphEvent::Resize("flat_noise".into(), Some([320.0, 400.0]))));
        let node = editor.graph_nodes().into_iter().find(|node| node.name == "flat_noise").unwrap();
        assert_eq!(node.size, Some([320.0, 400.0]));
        assert!(node.shape().thumb > 150.0, "a bigger box, a bigger thumbnail");
        assert!(node.params.iter().any(|param| param.starts_with("scale:")), "{:?}", node.params);
        // Moverla conserva el tamaño; "Normal box size" lo quita.
        editor.update(Message::Graph(GraphEvent::Move("flat_noise".into(), [0.0, 0.0])));
        assert_eq!(editor.current().unwrap().size("flat_noise"), Some([320.0, 400.0]));
        editor.update(Message::Graph(GraphEvent::Resize("flat_noise".into(), None)));
        assert_eq!(editor.current().unwrap().size("flat_noise"), None);
        assert_eq!(editor.current().unwrap().position("flat_noise"), Some([0.0, 0.0]));
        // Solo cambia el lienzo: no hace falta otra vista previa.
        fixture.preview();
        fixture.editor.update(Message::Graph(GraphEvent::Resize("flat_noise".into(), Some([200.0, 300.0]))));
        assert!(fixture.editor.take_job().is_none());
    }

    #[test]
    fn profiles_are_copies_and_the_world_uses_the_one_being_edited() {
        let mut fixture = Fixture::new("profiles");
        let default_path = fixture.doc().path.clone();
        let default_text = std::fs::read_to_string(&default_path).unwrap();
        let editor = &mut fixture.editor;
        editor.update(Message::NewProfileText("Wide Rivers".into()));
        editor.update(Message::NewProfile);
        assert_eq!(editor.doc, "base:plains@wide_rivers");
        let profile_path = fixture.dir.join("base/worldgen/profiles/plains/wide_rivers.toml");
        assert!(profile_path.is_file(), "a new profile is saved at once");
        let profile = |name: Option<&str>| {
            Request::Terrain(TerrainOp::Profile { biome: "base:plains".into(), profile: name.map(String::from) })
        };
        let editor = &mut fixture.editor;
        assert_eq!(editor.take_requests(), [profile(Some("wide_rivers"))]);

        // Editar el perfil no toca el archivo por defecto.
        let scale = node_field("flat_noise", "scale", Kind::Numbers);
        editor.update(Message::Text(scale, "321".into()));
        editor.update(Message::Save);
        assert!(std::fs::read_to_string(&profile_path).unwrap().contains("scale = 321"));
        assert_eq!(std::fs::read_to_string(&default_path).unwrap(), default_text);
        assert!(editor.workspace.as_ref().unwrap().source().unwrap().biomes["base:plains"].nodes.contains_key("flat_noise"));

        // Volver al archivo por defecto, y que el servidor diga que el mundo usa el perfil.
        editor.update(Message::Profile(DEFAULT_PROFILE.into()));
        assert_eq!(editor.doc, "base:plains");
        assert_eq!(editor.take_requests(), [profile(None)]);
        let status = TerrainStatus {
            auto: true,
            profiles: [("base:plains".to_string(), "wide_rivers".to_string())].into(),
            ..TerrainStatus::default()
        };
        editor.set_terrain_status(&status);
        assert_eq!(editor.doc, "base:plains@wide_rivers", "the editor follows the world");

        // Borrar pide dos clics.
        editor.update(Message::DeleteProfile);
        assert!(profile_path.is_file());
        editor.update(Message::DeleteProfile);
        assert!(!profile_path.exists());
        assert_eq!(editor.doc, "base:plains");
        assert_eq!(editor.take_requests(), [profile(None)]);
        let _ = editor.view();

        // Sin "Live world", "Apply" guarda lo pendiente y pide al servidor que lo use.
        editor.update(Message::LiveWorld(false));
        editor.update(Message::Text(node_field("flat_noise", "scale", Kind::Numbers), "222".into()));
        assert!(editor.dirty());
        editor.update(Message::Apply);
        assert!(!editor.dirty());
        assert_eq!(editor.take_requests(), [Request::Terrain(TerrainOp::Auto(false)), Request::Terrain(TerrainOp::Apply)]);
        let _ = editor.view();
    }

    #[test]
    fn a_new_biome_joins_the_world() {
        let mut fixture = Fixture::new("new");
        let editor = &mut fixture.editor;
        editor.update(Message::Doc("world".into()));
        editor.update(Message::NewBiomeText("Salt Flats".into()));
        editor.update(Message::NewBiome);
        assert_eq!(editor.doc, "base:salt_flats");
        let order = editor.workspace.as_ref().unwrap().world.get("biomes").unwrap();
        assert_eq!(order.as_array().unwrap().last().unwrap().as_str(), Some("salt_flats"));
        editor.update(Message::Save);
        assert!(fixture.dir.join("base/worldgen/biomes/salt_flats.toml").is_file());
        fixture.preview();
        assert!(fixture.editor.map_error.is_none(), "{:?}", fixture.editor.map_error);
    }
}
