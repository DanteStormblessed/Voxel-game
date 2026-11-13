//! Contenido del juego definido en archivos (paquetes de contenido).
//!
//! El servidor carga los paquetes, asigna a cada **estado de bloque** un número y a cada
//! ítem un índice, y envía el resultado (`ContentData`) a los clientes. Nada del
//! contenido está fijado en el código.
//!
//! * Un **bloque** (`BlockDef`) es un tipo: piedra, tronco. Sus campos pueden heredarse
//!   de otro bloque o de una plantilla (`parent`).
//! * Un **estado** es una variante concreta que va en los voxeles: el tronco tumbado en
//!   X es un estado, el tronco de pie otro. Cada estado tiene su número (`Block`) y su
//!   nombre (`base:wood[axis=x]`); un bloque sin orientación tiene un solo estado, que
//!   se llama como el bloque.
//! * Un **ítem** (`ItemDef`) es lo que se tiene en la mano. Cada bloque genera el suyo
//!   (con su mismo identificador) y los paquetes pueden definir más en `items/`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{bail, Context};
use glam::{IVec3, Vec3};
use serde::{Deserialize, Serialize};

use crate::block::Block;
use crate::icon::{self, IconImage, IconSpec};
use crate::model::{self, CellBox, Model, Rotation};

/// Mini-voxeles por metro de los objetos pequeños (12,5 cm cada uno). Reservado: los
/// modelos de mini-voxeles aún no se cargan ni se dibujan.
pub const MICRO_VOXELS_PER_METER: u32 = 8;
/// Números de bloque de 16 bits, con el 0 reservado para el aire.
pub const MAX_BLOCKS: usize = u16::MAX as usize;
/// Nivel máximo de luz que emite un bloque.
pub const MAX_LIGHT: u8 = 15;
/// Celdas como mucho de un bloque de varias celdas.
pub const MAX_PARTS: usize = 64;
/// Categoría de los bloques y de los ítems que no dicen la suya.
pub const DEFAULT_BLOCK_CATEGORY: &str = "Blocks";
pub const DEFAULT_ITEM_CATEGORY: &str = "Items";

/// Cómo se orienta un bloque al colocarlo. Cada orientación es un estado distinto.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    /// Un solo estado.
    #[default]
    None,
    /// Según el eje de la cara sobre la que se coloca (troncos): `axis=x|y|z`.
    Axis,
    /// Mirando hacia quien lo coloca (hornos, cofres): `facing=north|east|south|west`.
    Horizontal,
}

impl Orientation {
    /// Nombre de la propiedad y de cada uno de sus valores, en orden de estado.
    pub fn property(self) -> Option<(&'static str, &'static [&'static str])> {
        match self {
            Orientation::None => None,
            Orientation::Axis => Some(("axis", &["x", "y", "z"])),
            Orientation::Horizontal => Some(("facing", &["north", "east", "south", "west"])),
        }
    }

    pub fn state_count(self) -> u8 {
        self.property().map_or(1, |(_, values)| values.len() as u8)
    }

    /// Estado que se usa si nadie dice otro (el que genera el terreno, el del ítem).
    pub fn default_state(self) -> u8 {
        match self {
            Orientation::Axis => 1, // de pie
            _ => 0,
        }
    }

    /// Giro del modelo y de las caras en un estado.
    pub fn rotation(self, state: u8) -> Rotation {
        match (self, state) {
            (Orientation::Axis, 0) => Rotation::UpToX,
            (Orientation::Axis, 2) => Rotation::UpToZ,
            (Orientation::Horizontal, 1) => Rotation::FrontToEast,
            (Orientation::Horizontal, 2) => Rotation::FrontToSouth,
            (Orientation::Horizontal, 3) => Rotation::FrontToWest,
            _ => Rotation::Identity,
        }
    }

    /// Estado al colocar el bloque sobre la cara con normal `normal` (apunta hacia
    /// quien coloca), mirando en la dirección `look`.
    pub fn place(self, normal: IVec3, look: Vec3) -> u8 {
        match self {
            Orientation::None => 0,
            Orientation::Axis => {
                if normal.x != 0 {
                    0
                } else if normal.z != 0 {
                    2
                } else {
                    1
                }
            }
            Orientation::Horizontal => {
                // El frente mira hacia quien lo coloca: contrario a la mirada. Norte = -Z.
                if look.x.abs() > look.z.abs() {
                    if look.x > 0.0 { 3 } else { 1 } // mira al este → frente al oeste
                } else if look.z > 0.0 {
                    0 // mira al sur → frente al norte
                } else {
                    2
                }
            }
        }
    }
}

/// Color de cada cara de un bloque, ya resuelto (sRGB).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaceColors {
    pub top: [u8; 3],
    pub side: [u8; 3],
    pub bottom: [u8; 3],
    /// Lado que mira hacia `facing` en los bloques con orientación horizontal.
    pub front: [u8; 3],
}

/// Definición de un tipo de bloque, tal como viaja a los clientes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockDef {
    /// Identificador global, `paquete:nombre`.
    pub id: String,
    /// Nombre visible.
    pub name: String,
    /// Color principal (sRGB): el de la interfaz y el de las caras sin color propio.
    pub color: [u8; 3],
    pub faces: FaceColors,
    pub orientation: Orientation,
    /// Bloquea el paso (colisión). Sin efecto todavía: llega en la fase 3c.
    pub solid: bool,
    /// Deja ver a través (agua, cristal). Sin efecto todavía: llega en la fase 3c.
    pub transparent: bool,
    /// Luz que emite, de 0 a 15. Por ahora solo hará que brille (fase 3c).
    pub light: u8,
    /// Grupo en el que aparece en el inventario creativo.
    pub category: String,
    /// Etiquetas libres para buscar y para que el generador o los mods elijan bloques.
    pub tags: Vec<String>,
    /// Modelo de mini-voxeles que dibuja en vez de un cubo (`paquete:nombre`).
    pub model: Option<String>,
    /// Celda del modelo que dibuja siempre este bloque, si el archivo la fija (`cell`).
    pub cell: Option<[u8; 3]>,
    /// Celdas del modelo que ocupa el objeto, la primera la de anclaje (la que se coloca
    /// donde apunta el jugador). Una por bloque salvo los modelos de varias celdas sin
    /// `cell` (una puerta: dos), que se colocan y se rompen enteros. Vacío sin modelo.
    pub parts: Vec<[u8; 3]>,
    /// Forma con la que choca, si es `solid`.
    pub collision: CollisionShape,
    /// Opacidad de los bloques `transparent` (0 = invisible, 255 = opaco).
    pub opacity: u8,
    /// Genera un ítem con su mismo identificador (`item = false` en el archivo para no).
    pub has_item: bool,
    /// Icono de su ítem.
    pub icon: IconSpec,
}

/// Con qué forma choca un bloque sólido.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollisionShape {
    /// El cubo entero.
    Full,
    /// Las cajas que ocupa su modelo.
    Model,
    /// Cajas en mini-voxeles (`[x0, y0, z0, x1, y1, z1]`, de 0 a 8), antes de orientar.
    Boxes(Vec<CellBox>),
}

/// Un modelo de mini-voxeles con su identificador.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelDef {
    pub id: String,
    pub model: Model,
}

/// Lo que hay en un número de bloque: qué tipo y qué variante.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockState {
    /// Índice en `ContentData::blocks`.
    pub block: u16,
    /// Variante, según la orientación del bloque.
    pub state: u8,
    /// Celda que ocupa en un bloque de varias celdas (índice en `BlockDef::parts`).
    pub part: u16,
}

/// Índice de un ítem en `ContentData::items`. Como los números de bloque, se conserva al
/// recargar el contenido.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemId(pub u16);

/// Definición de un ítem, tal como viaja a los clientes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
    /// Color con el que se dibuja en la interfaz (sRGB).
    pub color: [u8; 3],
    pub category: String,
    pub tags: Vec<String>,
    /// Bloque (estado por defecto) que coloca el clic derecho, si coloca alguno.
    pub block: Option<Block>,
    /// Herramienta del motor que activa (reservado: llega con las herramientas creativas).
    pub tool: Option<String>,
    /// Modelo propio (la espada): se ve en el icono y al soltarlo. Si no tiene, se usa el
    /// de su bloque.
    pub model: Option<String>,
    /// Cuántos caben en una ranura (reservado para la supervivencia).
    pub max_stack: u16,
    pub icon: IconSpec,
}

impl ItemDef {
    /// El ítem coincide con un texto de búsqueda (nombre, identificador, categoría o
    /// etiqueta).
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || self.name.to_lowercase().contains(&query)
            || self.id.to_lowercase().contains(&query)
            || self.category.to_lowercase().contains(&query)
            || self.tags.iter().any(|tag| tag.to_lowercase().contains(&query))
    }
}

/// Todo el contenido activo en un servidor.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContentData {
    /// Tipos de bloque, ordenados por identificador. Su índice no es estable: lo que va
    /// en los chunks es el número de estado.
    pub blocks: Vec<BlockDef>,
    /// Estado de cada número de bloque; el 0 (aire) y los retirados son `None`.
    pub states: Vec<Option<BlockState>>,
    /// Ítems por índice; los retirados son `None`.
    pub items: Vec<Option<ItemDef>>,
    /// Modelos de mini-voxeles, ordenados por identificador.
    pub models: Vec<ModelDef>,
}

/// Separa `base:wood[axis=x]` en (`base:wood`, `Some(("axis", "x"))`).
/// Separa `base:door[facing=east,part=0-1-0]` en el id y sus propiedades.
fn split_state_name(name: &str) -> Option<(&str, Vec<(&str, &str)>)> {
    let Some((id, rest)) = name.split_once('[') else { return Some((name, Vec::new())) };
    let properties = rest
        .strip_suffix(']')?
        .split(',')
        .map(|pair| pair.split_once('=').map(|(key, value)| (key.trim(), value.trim())))
        .collect::<Option<Vec<_>>>()?;
    Some((id, properties))
}

/// Nombre de una celda en los nombres de estado: `0-1-0`.
fn part_name(cell: [u8; 3]) -> String {
    format!("{}-{}-{}", cell[0], cell[1], cell[2])
}

impl BlockDef {
    /// Partes del objeto (1 salvo los bloques de varias celdas).
    pub fn part_count(&self) -> usize {
        self.parts.len().max(1)
    }

    /// Se puede colocar algo encima sin romperlo antes (aire, agua).
    pub fn replaceable(&self) -> bool {
        !self.solid && self.model.is_none()
    }

    /// Nombre de uno de sus estados.
    fn state_name(&self, state: u8, part: u16) -> String {
        let mut properties = Vec::new();
        if let Some((key, values)) = self.orientation.property() {
            properties.push(format!("{key}={}", values[state as usize]));
        }
        if self.parts.len() > 1 {
            properties.push(format!("part={}", part_name(self.parts[part as usize])));
        }
        if properties.is_empty() {
            self.id.clone()
        } else {
            format!("{}[{}]", self.id, properties.join(","))
        }
    }
}

impl ContentData {
    /// Tipos de bloque.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn item_count(&self) -> usize {
        self.items.iter().flatten().count()
    }

    pub fn state(&self, block: Block) -> Option<BlockState> {
        self.states.get(block.0 as usize).copied().flatten()
    }

    /// Tipo de bloque de un número de bloque (el aire no tiene).
    pub fn block(&self, block: Block) -> Option<&BlockDef> {
        self.state(block).map(|state| &self.blocks[state.block as usize])
    }

    /// El número existe en el contenido (el aire cuenta como existente).
    pub fn is_known(&self, block: Block) -> bool {
        block == Block::AIR || self.state(block).is_some()
    }

    /// Números de bloque de todos los estados, con su estado.
    pub fn numbered_states(&self) -> impl Iterator<Item = (Block, BlockState)> + '_ {
        self.states
            .iter()
            .enumerate()
            .filter_map(|(index, state)| state.map(|state| (Block(index as u16), state)))
    }

    fn find_state(&self, block: u16, state: u8, part: u16) -> Option<Block> {
        self.numbered_states()
            .find(|(_, s)| s.block == block && s.state == state && s.part == part)
            .map(|(number, _)| number)
    }

    /// Nombre de un estado: `base:stone`, `base:wood[axis=x]` si el bloque se orienta o
    /// `base:door[facing=east,part=0-1-0]` si además ocupa varias celdas.
    pub fn state_name(&self, block: Block) -> Option<String> {
        let state = self.state(block)?;
        Some(self.blocks[state.block as usize].state_name(state.state, state.part))
    }

    /// Número de un estado por nombre. Las propiedades que no se escriban toman su valor
    /// por defecto (`base:door` es la parte de abajo mirando al norte).
    pub fn block_id(&self, name: &str) -> Option<Block> {
        let (id, properties) = split_state_name(name.trim())?;
        let index = self.blocks.iter().position(|def| def.id == id)?;
        let def = &self.blocks[index];
        let mut state = def.orientation.default_state();
        let mut part = 0u16;
        for (key, value) in properties {
            match def.orientation.property() {
                Some((expected, values)) if key == expected => {
                    state = values.iter().position(|&v| v == value)? as u8;
                }
                _ if key == "part" && def.parts.len() > 1 => {
                    part = def.parts.iter().position(|&cell| part_name(cell) == value)? as u16;
                }
                _ => return None,
            }
        }
        self.find_state(index as u16, state, part)
    }

    /// Estado por defecto del mismo bloque (el que da su ítem): orientación por defecto
    /// y parte de anclaje.
    pub fn default_state(&self, block: Block) -> Block {
        let Some(state) = self.state(block) else { return block };
        let default = self.blocks[state.block as usize].orientation.default_state();
        self.find_state(state.block, default, 0).unwrap_or(block)
    }

    /// Estado de `block` orientado según cómo se coloca: sobre la cara con normal
    /// `normal` (hacia quien coloca) y mirando hacia `look`.
    pub fn orient(&self, block: Block, normal: IVec3, look: Vec3) -> Block {
        let Some(state) = self.state(block) else { return block };
        let orientation = self.blocks[state.block as usize].orientation;
        self.find_state(state.block, orientation.place(normal, look), state.part).unwrap_or(block)
    }

    /// Todas las celdas del objeto al que pertenece el estado `block` puesto en `pos`:
    /// (posición, estado). Un bloque normal es solo él; una puerta, sus dos mitades,
    /// giradas según su orientación.
    pub fn structure(&self, block: Block, pos: IVec3) -> Vec<(IVec3, Block)> {
        let Some(state) = self.state(block) else { return vec![(pos, block)] };
        let def = &self.blocks[state.block as usize];
        if def.parts.len() <= 1 {
            return vec![(pos, block)];
        }
        let rotation = def.orientation.rotation(state.state);
        let anchor = IVec3::from_array(def.parts[0].map(i32::from));
        let offset = |cell: [u8; 3]| {
            let delta = IVec3::from_array(cell.map(i32::from)) - anchor;
            IVec3::from_array(rotation.apply_dir(delta.to_array()))
        };
        let origin = pos - offset(def.parts[state.part as usize]);
        (0..def.parts.len())
            .filter_map(|part| {
                let number = self.find_state(state.block, state.state, part as u16)?;
                Some((origin + offset(def.parts[part]), number))
            })
            .collect()
    }

    /// Se puede colocar algo en una celda con este bloque sin romperlo antes.
    pub fn replaceable(&self, block: Block) -> bool {
        block == Block::AIR || self.block(block).is_some_and(BlockDef::replaceable)
    }

    pub fn model(&self, id: &str) -> Option<&ModelDef> {
        self.models.iter().find(|model| model.id == id)
    }

    pub fn item(&self, item: ItemId) -> Option<&ItemDef> {
        self.items.get(item.0 as usize).and_then(|item| item.as_ref())
    }

    /// Ítems que existen, con su índice, en orden.
    pub fn numbered_items(&self) -> impl Iterator<Item = (ItemId, &ItemDef)> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| item.as_ref().map(|item| (ItemId(index as u16), item)))
    }

    pub fn item_id(&self, id: &str) -> Option<ItemId> {
        self.numbered_items().find(|(_, item)| item.id == id).map(|(index, _)| index)
    }

    /// El ítem que coloca este bloque (cualquier estado suyo), si hay alguno.
    pub fn item_for_block(&self, block: Block) -> Option<ItemId> {
        let block = self.default_state(block);
        self.numbered_items().find(|(_, item)| item.block == Some(block)).map(|(index, _)| index)
    }

    /// Categorías de los ítems, sin repetir, por orden alfabético.
    pub fn categories(&self) -> Vec<&str> {
        let mut categories: Vec<&str> =
            self.numbered_items().map(|(_, item)| item.category.as_str()).collect();
        categories.sort_unstable();
        categories.dedup();
        categories
    }

    /// Nombre de cada número de bloque (vacío en los huecos y el aire). Es lo que guarda
    /// `world.toml` para que cada estado conserve su número entre sesiones.
    pub fn state_names(&self) -> Vec<String> {
        (0..self.states.len())
            .map(|index| self.state_name(Block(index as u16)).unwrap_or_default())
            .collect()
    }

    pub fn item_names(&self) -> Vec<String> {
        self.items.iter().map(|item| item.as_ref().map_or(String::new(), |i| i.id.clone())).collect()
    }

    /// Construye el contenido a partir de lo leído del disco.
    ///
    /// `previous_states` y `previous_items` son los nombres que ya tenían número (de
    /// `world.toml` o del contenido anterior): esos lo conservan, así que los chunks en
    /// memoria, en disco y en los clientes siguen siendo válidos. Lo nuevo va al final y
    /// lo retirado deja su hueco.
    pub fn assign(
        loaded: Loaded,
        previous_states: &[String],
        previous_items: &[String],
    ) -> anyhow::Result<Self> {
        let Loaded { mut blocks, items, models } = loaded;
        for block in &mut blocks {
            let Some(id) = &block.model else { continue };
            let model = &models
                .iter()
                .find(|model| model.id == *id)
                .with_context(|| format!("block {}: unknown model \"{id}\"", block.id))?
                .model;
            block.parts = match block.cell {
                Some(cell) if model.cell(cell).is_none() => bail!(
                    "block {}: model {id} has no cell {cell:?} (it is {:?} cells)",
                    block.id,
                    model.cells()
                ),
                Some(cell) => vec![cell],
                // Sin `cell`, el bloque es el objeto entero: una parte por celda con algo,
                // de abajo arriba; la primera es el anclaje.
                None => {
                    let [cx, cy, cz] = model.cells();
                    let mut parts = Vec::new();
                    for y in 0..cy {
                        for z in 0..cz {
                            for x in 0..cx {
                                let cell = [x as u8, y as u8, z as u8];
                                if model.cell(cell).is_some_and(|c| c.iter().any(|&v| v != 0)) {
                                    parts.push(cell);
                                }
                            }
                        }
                    }
                    if parts.len() > MAX_PARTS {
                        bail!("block {}: model {id} spans {} cells (maximum {MAX_PARTS})", block.id, parts.len());
                    }
                    parts
                }
            };
        }
        for item in &items {
            if let Some(id) = item.model.as_ref().filter(|id| !models.iter().any(|m| m.id == **id)) {
                bail!("item {}: unknown model \"{id}\"", item.id);
            }
        }

        // Estados: nombre de cada uno y a qué tipo pertenecen.
        let mut wanted: Vec<(String, BlockState, bool)> = Vec::new();
        for (index, def) in blocks.iter().enumerate() {
            let default = def.orientation.default_state();
            for state in 0..def.orientation.state_count() {
                for part in 0..def.part_count() as u16 {
                    let name = def.state_name(state, part);
                    let is_default = state == default && part == 0;
                    wanted.push((name, BlockState { block: index as u16, state, part }, is_default));
                }
            }
        }
        let names: Vec<(String, Option<String>)> = wanted
            .iter()
            .map(|(name, state, default)| {
                // Un bloque que antes no se orientaba se llamaba solo por su id: ese número
                // pasa a su estado por defecto.
                let alias = default.then(|| blocks[state.block as usize].id.clone());
                (name.clone(), alias.filter(|alias| alias != name))
            })
            .collect();
        let numbers = assign_numbers(&names, previous_states, true);
        let mut states: Vec<Option<BlockState>> = vec![None; numbers.len];
        for ((_, state, _), &number) in wanted.iter().zip(&numbers.assigned) {
            states[number] = Some(*state);
        }
        if states.len() > MAX_BLOCKS + 1 {
            bail!("too many block states: {} (maximum {MAX_BLOCKS})", states.len() - 1);
        }
        let mut content = Self { blocks, states, items: Vec::new(), models };

        // Ítems: los de cada bloque, sustituidos por los de `items/` con el mismo id.
        let mut defs: BTreeMap<String, ItemDef> = BTreeMap::new();
        for def in &content.blocks {
            if !def.has_item {
                continue;
            }
            let block = content.block_id(&def.id);
            defs.insert(
                def.id.clone(),
                ItemDef {
                    id: def.id.clone(),
                    name: def.name.clone(),
                    color: def.color,
                    category: def.category.clone(),
                    tags: def.tags.clone(),
                    block,
                    tool: None,
                    model: None,
                    max_stack: 64,
                    icon: def.icon.clone(),
                },
            );
        }
        for item in items {
            let block = match &item.block {
                Some(name) => Some(
                    content
                        .block_id(name)
                        .with_context(|| format!("item {}: unknown block \"{name}\"", item.id))?,
                ),
                None => None,
            };
            let block_def = block.and_then(|block| content.block(block));
            let name = item
                .name
                .or_else(|| block_def.map(|def| def.name.clone()))
                .with_context(|| format!("item {}: missing name", item.id))?;
            let color = item
                .color
                .or_else(|| block_def.map(|def| def.color))
                .with_context(|| format!("item {}: missing color", item.id))?;
            let category = item.category.unwrap_or_else(|| {
                block_def.map_or(DEFAULT_ITEM_CATEGORY.to_string(), |def| def.category.clone())
            });
            let def = ItemDef {
                id: item.id.clone(),
                name,
                color,
                category,
                tags: item.tags.unwrap_or_default(),
                block: block.map(|block| content.default_state(block)),
                tool: item.tool,
                model: item.model,
                max_stack: item.max_stack.unwrap_or(64).max(1),
                icon: item.icon.or_else(|| block_def.map(|def| def.icon.clone())).unwrap_or_default(),
            };
            defs.insert(item.id, def);
        }
        let names: Vec<(String, Option<String>)> =
            defs.keys().map(|id| (id.clone(), None)).collect();
        let numbers = assign_numbers(&names, previous_items, false);
        let mut items: Vec<Option<ItemDef>> = vec![None; numbers.len];
        for (def, &number) in defs.into_values().zip(&numbers.assigned) {
            items[number] = Some(def);
        }
        if items.len() > u16::MAX as usize {
            bail!("too many items: {}", items.len());
        }
        content.items = items;
        Ok(content)
    }
}

struct Numbers {
    /// Número de cada nombre pedido, en el mismo orden.
    assigned: Vec<usize>,
    /// Tamaño de la tabla resultante (huecos incluidos).
    len: usize,
}

/// Reparte números a `names` (nombre, alias aceptado) respetando los de `previous` (nombre
/// por número). Con `reserve_zero`, el 0 nunca se asigna (es el aire).
fn assign_numbers(names: &[(String, Option<String>)], previous: &[String], reserve_zero: bool) -> Numbers {
    let start = reserve_zero as usize;
    let lookup: BTreeMap<&str, usize> = previous
        .iter()
        .enumerate()
        .skip(start)
        .filter(|(_, name)| !name.is_empty())
        .map(|(number, name)| (name.as_str(), number))
        .collect();
    let mut taken = vec![false; previous.len().max(start)];
    let mut assigned = vec![usize::MAX; names.len()];
    // Primero los nombres exactos; después los alias, si su número sigue libre.
    for pass in 0..2 {
        for (slot, (name, alias)) in names.iter().enumerate() {
            if assigned[slot] != usize::MAX {
                continue;
            }
            let key = if pass == 0 { Some(name.as_str()) } else { alias.as_deref() };
            if let Some(&number) = key.and_then(|key| lookup.get(key)) {
                if !taken[number] {
                    taken[number] = true;
                    assigned[slot] = number;
                }
            }
        }
    }
    let mut next = taken.len();
    for slot in &mut assigned {
        if *slot == usize::MAX {
            *slot = next;
            next += 1;
        }
    }
    Numbers { assigned, len: next }
}

// --- Lectura de archivos ---

/// Lo leído de los paquetes, antes de numerar.
#[derive(Default)]
pub struct Loaded {
    pub blocks: Vec<BlockDef>,
    pub items: Vec<ItemSource>,
    pub models: Vec<ModelDef>,
}

/// Un ítem de `items/`, con la herencia ya resuelta pero sin números.
#[derive(Clone, Debug)]
pub struct ItemSource {
    pub id: String,
    pub name: Option<String>,
    pub color: Option<[u8; 3]>,
    pub category: Option<String>,
    pub tags: Option<Vec<String>>,
    /// Nombre del bloque (o estado) que coloca.
    pub block: Option<String>,
    pub tool: Option<String>,
    pub model: Option<String>,
    pub max_stack: Option<u16>,
    /// `None` si el archivo no dice nada: entonces sirve el del bloque que coloca.
    pub icon: Option<IconSpec>,
}

/// `[icon]` en un archivo de bloque o de ítem. Todo opcional para poder heredarlo.
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct IconFile {
    image: Option<String>,
    rotation: Option<[f32; 3]>,
    scale: Option<f32>,
    offset: Option<[f32; 2]>,
}

impl IconFile {
    fn inherit(self, parent: &Option<IconFile>) -> IconFile {
        let Some(parent) = parent else { return self };
        IconFile {
            image: self.image.or_else(|| parent.image.clone()),
            rotation: self.rotation.or(parent.rotation),
            scale: self.scale.or(parent.scale),
            offset: self.offset.or(parent.offset),
        }
    }

    fn resolve(&self, icons: &BTreeMap<String, IconImage>) -> anyhow::Result<IconSpec> {
        let image = match &self.image {
            Some(id) => Some(icons.get(id).cloned().with_context(|| {
                format!("unknown icon image \"{id}\" (expected icons/<name>.png in its pack)")
            })?),
            None => None,
        };
        let spec = IconSpec {
            image,
            rotation: self.rotation.unwrap_or(icon::DEFAULT_ROTATION),
            scale: self.scale.unwrap_or(1.0),
            offset: self.offset.unwrap_or([0.0, 0.0]),
        };
        if !(spec.scale > 0.0 && spec.scale <= 10.0) || !spec.rotation.iter().chain(&spec.offset).all(|v| v.is_finite()) {
            bail!("icon: scale must be between 0 and 10, and rotation and offset must be numbers");
        }
        Ok(spec)
    }
}

fn inherit_icon(child: Option<IconFile>, parent: &Option<IconFile>) -> Option<IconFile> {
    match child {
        Some(child) => Some(child.inherit(parent)),
        None => parent.clone(),
    }
}

/// `collision` en un archivo de bloque: un nombre o una lista de cajas.
#[derive(Clone, Deserialize)]
#[serde(untagged)]
enum CollisionFile {
    Named(String),
    Boxes(Vec<CellBox>),
}

/// Un archivo de bloque en disco. Todo es opcional para poder heredarlo de `parent`.
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockFile {
    id: Option<String>,
    parent: Option<String>,
    /// Solo sirve para heredar: no es un bloque (ni se hereda esta marca).
    #[serde(default)]
    template: bool,
    name: Option<String>,
    color: Option<String>,
    faces: Option<FacesFile>,
    orientation: Option<Orientation>,
    solid: Option<bool>,
    transparent: Option<bool>,
    light: Option<u8>,
    category: Option<String>,
    tags: Option<Vec<String>>,
    /// Genera su ítem (por defecto sí).
    item: Option<bool>,
    model: Option<String>,
    cell: Option<[u8; 3]>,
    collision: Option<CollisionFile>,
    opacity: Option<f32>,
    icon: Option<IconFile>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FacesFile {
    top: Option<String>,
    side: Option<String>,
    bottom: Option<String>,
    front: Option<String>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemFile {
    id: Option<String>,
    parent: Option<String>,
    #[serde(default)]
    template: bool,
    name: Option<String>,
    color: Option<String>,
    category: Option<String>,
    tags: Option<Vec<String>>,
    block: Option<String>,
    tool: Option<String>,
    model: Option<String>,
    max_stack: Option<u16>,
    icon: Option<IconFile>,
}

/// Campos que se heredan: lo del hijo gana, lo que no tenga lo pone el padre.
trait Inherit {
    fn parent(&self) -> Option<&str>;
    fn inherit(self, parent: &Self) -> Self;
}

impl Inherit for BlockFile {
    fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    fn inherit(self, parent: &Self) -> Self {
        let faces = match (self.faces, &parent.faces) {
            (Some(child), Some(parent)) => Some(FacesFile {
                top: child.top.or_else(|| parent.top.clone()),
                side: child.side.or_else(|| parent.side.clone()),
                bottom: child.bottom.or_else(|| parent.bottom.clone()),
                front: child.front.or_else(|| parent.front.clone()),
            }),
            (child, parent) => child.or_else(|| parent.clone()),
        };
        Self {
            id: self.id,
            parent: self.parent,
            template: self.template,
            name: self.name.or_else(|| parent.name.clone()),
            color: self.color.or_else(|| parent.color.clone()),
            faces,
            orientation: self.orientation.or(parent.orientation),
            solid: self.solid.or(parent.solid),
            transparent: self.transparent.or(parent.transparent),
            light: self.light.or(parent.light),
            category: self.category.or_else(|| parent.category.clone()),
            tags: self.tags.or_else(|| parent.tags.clone()),
            item: self.item.or(parent.item),
            model: self.model.or_else(|| parent.model.clone()),
            cell: self.cell.or(parent.cell),
            collision: self.collision.or_else(|| parent.collision.clone()),
            opacity: self.opacity.or(parent.opacity),
            icon: inherit_icon(self.icon, &parent.icon),
        }
    }
}

impl Inherit for ItemFile {
    fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    fn inherit(self, parent: &Self) -> Self {
        Self {
            id: self.id,
            parent: self.parent,
            template: self.template,
            name: self.name.or_else(|| parent.name.clone()),
            color: self.color.or_else(|| parent.color.clone()),
            category: self.category.or_else(|| parent.category.clone()),
            tags: self.tags.or_else(|| parent.tags.clone()),
            block: self.block.or_else(|| parent.block.clone()),
            tool: self.tool.or_else(|| parent.tool.clone()),
            model: self.model.or_else(|| parent.model.clone()),
            max_stack: self.max_stack.or(parent.max_stack),
            icon: inherit_icon(self.icon, &parent.icon),
        }
    }
}

/// Resuelve la herencia de cada entrada (por identificador) siguiendo la cadena de
/// `parent`. Un padre que no existe o un ciclo es un error.
fn resolve_inheritance<T: Inherit + Clone>(raw: &BTreeMap<String, T>) -> anyhow::Result<BTreeMap<String, T>> {
    let mut resolved = BTreeMap::new();
    for id in raw.keys() {
        let mut chain = vec![id.as_str()];
        let mut current = &raw[id];
        while let Some(parent) = current.parent() {
            if chain.contains(&parent) {
                bail!("{id}: inheritance cycle through \"{parent}\"");
            }
            current = raw.get(parent).with_context(|| format!("{id}: unknown parent \"{parent}\""))?;
            chain.push(parent);
        }
        // Del ancestro más lejano al propio archivo.
        let mut merged = raw[*chain.last().expect("chain starts with the id")].clone();
        for link in chain.iter().rev().skip(1) {
            merged = raw[*link].clone().inherit(&merged);
        }
        resolved.insert(id.clone(), merged);
    }
    Ok(resolved)
}

fn parse_color(text: &str) -> anyhow::Result<[u8; 3]> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 || !hex.is_ascii() {
        bail!("invalid color \"{text}\": expected #RRGGBB");
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16);
    match (channel(0), channel(2), channel(4)) {
        (Ok(r), Ok(g), Ok(b)) => Ok([r, g, b]),
        _ => bail!("invalid color \"{text}\": expected #RRGGBB"),
    }
}

fn valid_id(id: &str) -> bool {
    let Some((pack, name)) = id.split_once(':') else { return false };
    let ok = |part: &str| {
        !part.is_empty()
            && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/'))
    };
    ok(pack) && ok(name)
}

fn block_def(id: &str, file: &BlockFile, icons: &BTreeMap<String, IconImage>) -> anyhow::Result<BlockDef> {
    let name = file.name.clone().context("missing name")?;
    let color = file.color.as_deref().map(parse_color).transpose()?;
    let faces = file.faces.clone().unwrap_or_default();
    let face = |value: &Option<String>| value.as_deref().map(parse_color).transpose();
    let (top, side, bottom) = (face(&faces.top)?, face(&faces.side)?, face(&faces.bottom)?);
    let front = face(&faces.front)?;
    // El color principal es `color`; si no hay, el de los lados.
    let color = color.or(side).context("missing color (or faces.side)")?;
    let light = file.light.unwrap_or(0);
    if light > MAX_LIGHT {
        bail!("light must be between 0 and {MAX_LIGHT}");
    }
    let collision = match &file.collision {
        None if file.model.is_some() => CollisionShape::Model,
        None => CollisionShape::Full,
        Some(CollisionFile::Named(name)) => match name.as_str() {
            "full" => CollisionShape::Full,
            "model" if file.model.is_some() => CollisionShape::Model,
            "model" => bail!("collision = \"model\" needs a model"),
            other => bail!("unknown collision \"{other}\": use full, model or a list of boxes"),
        },
        Some(CollisionFile::Boxes(boxes)) => {
            let valid = |b: &CellBox| (0..3).all(|a| b[a] < b[a + 3] && b[a + 3] <= model::CELL as u8);
            if boxes.is_empty() || !boxes.iter().all(valid) {
                bail!("collision boxes must be [x0, y0, z0, x1, y1, z1] with 0 <= x0 < x1 <= 8");
            }
            CollisionShape::Boxes(boxes.clone())
        }
    };
    let opacity = file.opacity.unwrap_or(0.6);
    if !(0.0..=1.0).contains(&opacity) {
        bail!("opacity must be between 0 and 1");
    }
    Ok(BlockDef {
        id: id.to_string(),
        name,
        color,
        faces: FaceColors {
            top: top.unwrap_or(color),
            side: side.unwrap_or(color),
            bottom: bottom.unwrap_or(color),
            front: front.or(side).unwrap_or(color),
        },
        orientation: file.orientation.unwrap_or_default(),
        solid: file.solid.unwrap_or(true),
        transparent: file.transparent.unwrap_or(false),
        light,
        category: file.category.clone().unwrap_or_else(|| DEFAULT_BLOCK_CATEGORY.to_string()),
        tags: file.tags.clone().unwrap_or_default(),
        model: file.model.clone(),
        cell: file.cell,
        parts: Vec::new(),
        collision,
        opacity: (opacity * 255.0).round() as u8,
        has_item: file.item.unwrap_or(true),
        icon: file.icon.clone().unwrap_or_default().resolve(icons)?,
    })
}

fn sorted_entries(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    Ok(entries)
}

/// Carpetas de paquete dentro de `dir`: `base` primero, el resto por orden alfabético.
fn packs(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut packs: Vec<PathBuf> =
        sorted_entries(dir)?.into_iter().filter(|path| path.is_dir()).collect();
    packs.sort_by_key(|path| path.file_name().map_or(true, |name| name != "base"));
    Ok(packs)
}

fn stem(path: &Path) -> String {
    path.file_stem().unwrap_or_default().to_string_lossy().into_owned()
}

/// Lee los `.toml` de `<paquete>/<folder>/` de todos los paquetes, por identificador. Si
/// dos archivos definen el mismo, gana el del paquete cargado más tarde.
fn read_files<T: for<'de> Deserialize<'de>>(
    dir: &Path,
    folder: &str,
    id_of: impl Fn(&T) -> Option<String>,
) -> anyhow::Result<BTreeMap<String, (PathBuf, T)>> {
    let mut files = BTreeMap::new();
    for pack in packs(dir)? {
        let pack_name = stem(&pack);
        let folder = pack.join(folder);
        if !folder.is_dir() {
            continue;
        }
        for path in sorted_entries(&folder)? {
            if path.extension().is_none_or(|ext| ext != "toml") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let file: T =
                toml::from_str(&text).with_context(|| format!("error in {}", path.display()))?;
            let id = id_of(&file).unwrap_or_else(|| format!("{pack_name}:{}", stem(&path)));
            if !valid_id(&id) {
                bail!("error in {}: invalid id \"{id}\" (expected pack:name in lowercase)", path.display());
            }
            files.insert(id, (path, file));
        }
    }
    Ok(files)
}

/// Lee todos los bloques e ítems de todos los paquetes de `dir`, con la herencia resuelta.
pub fn load(dir: &Path) -> anyhow::Result<Loaded> {
    // BTreeMap: orden estable por identificador, para que los números asignados no
    // dependan del orden en que el sistema de archivos liste las carpetas.
    let icons = load_assets(dir, "icons", "png", |bytes| icon::decode_png(bytes))?;
    let block_files = read_files::<BlockFile>(dir, "blocks", |file| file.id.clone())?;
    let raw: BTreeMap<String, BlockFile> =
        block_files.iter().map(|(id, (_, file))| (id.clone(), file.clone())).collect();
    let mut blocks = Vec::new();
    for (id, file) in resolve_inheritance(&raw)? {
        if file.template {
            continue;
        }
        let path = &block_files[&id].0;
        blocks.push(block_def(&id, &file, &icons).with_context(|| format!("error in {}", path.display()))?);
    }
    if blocks.is_empty() {
        bail!("no blocks defined in {}", dir.display());
    }

    let item_files = read_files::<ItemFile>(dir, "items", |file| file.id.clone())?;
    let raw: BTreeMap<String, ItemFile> =
        item_files.iter().map(|(id, (_, file))| (id.clone(), file.clone())).collect();
    let mut items = Vec::new();
    for (id, file) in resolve_inheritance(&raw)? {
        if file.template {
            continue;
        }
        let path = &item_files[&id].0;
        let color = file
            .color
            .as_deref()
            .map(parse_color)
            .transpose()
            .with_context(|| format!("error in {}", path.display()))?;
        let icon = file
            .icon
            .as_ref()
            .map(|icon| icon.resolve(&icons))
            .transpose()
            .with_context(|| format!("error in {}", path.display()))?;
        items.push(ItemSource {
            id,
            name: file.name,
            color,
            category: file.category,
            tags: file.tags,
            block: file.block,
            tool: file.tool,
            model: file.model,
            max_stack: file.max_stack,
            icon,
        });
    }
    let models = load_assets(dir, "models", "vox", |bytes| model::parse_vox(bytes))?
        .into_iter()
        .map(|(id, model)| ModelDef { id, model })
        .collect();
    Ok(Loaded { blocks, items, models })
}

/// Lee los archivos `.<ext>` de `<paquete>/<folder>/` de todos los paquetes, por
/// identificador (`paquete:archivo`). Si dos paquetes tienen el mismo, gana el último.
fn load_assets<T>(
    dir: &Path,
    folder: &str,
    ext: &str,
    parse: impl Fn(&[u8]) -> anyhow::Result<T>,
) -> anyhow::Result<BTreeMap<String, T>> {
    let mut assets = BTreeMap::new();
    for pack in packs(dir)? {
        let folder = pack.join(folder);
        if !folder.is_dir() {
            continue;
        }
        for path in sorted_entries(&folder)? {
            if path.extension().is_none_or(|e| e != ext) {
                continue;
            }
            let id = format!("{}:{}", stem(&pack), stem(&path));
            if !valid_id(&id) {
                bail!("{}: invalid file name \"{id}\"", path.display());
            }
            let bytes = std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
            assets.insert(id, parse(&bytes).with_context(|| format!("error in {}", path.display()))?);
        }
    }
    Ok(assets)
}

/// Huella del contenido en disco: cambia cuando se añade, borra o modifica un archivo.
/// Sirve para detectar cuándo hay que recargar sin depender de avisos del sistema.
pub fn fingerprint(dir: &Path) -> (usize, Option<SystemTime>) {
    fn visit(dir: &Path, count: &mut usize, newest: &mut Option<SystemTime>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, count, newest);
            } else if let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) {
                *count += 1;
                *newest = Some(newest.map_or(modified, |newest| newest.max(modified)));
            }
        }
    }
    let (mut count, mut newest) = (0, None);
    visit(dir, &mut count, &mut newest);
    (count, newest)
}

/// Contenido mínimo para pruebas: un bloque sin orientación por nombre (`base:<nombre>`),
/// numerados desde 1, cada uno con su ítem.
pub fn test_content(names: &[&str]) -> ContentData {
    let blocks = names
        .iter()
        .enumerate()
        .map(|(index, name)| BlockDef::simple(&format!("base:{name}"), name, [index as u8 * 20, 100, 200]))
        .collect();
    // Sin orden alfabético: el número de cada bloque es su posición + 1.
    let previous: Vec<String> =
        std::iter::once(String::new()).chain(names.iter().map(|name| format!("base:{name}"))).collect();
    let items: Vec<String> = names.iter().map(|name| format!("base:{name}")).collect();
    ContentData::assign(Loaded { blocks, ..Loaded::default() }, &previous, &items)
        .expect("test content is valid")
}

impl BlockDef {
    /// Bloque sólido de un solo color, sin orientación (para pruebas y herramientas).
    pub fn simple(id: &str, name: &str, color: [u8; 3]) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            color,
            faces: FaceColors { top: color, side: color, bottom: color, front: color },
            orientation: Orientation::None,
            solid: true,
            transparent: false,
            light: 0,
            category: DEFAULT_BLOCK_CATEGORY.to_string(),
            tags: Vec::new(),
            model: None,
            cell: None,
            parts: Vec::new(),
            collision: CollisionShape::Full,
            opacity: 255,
            has_item: true,
            icon: IconSpec::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, path: &str, text: &str) {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kuravoxel-content-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn oriented(id: &str) -> BlockDef {
        BlockDef { orientation: Orientation::Axis, ..BlockDef::simple(id, id, [1, 2, 3]) }
    }

    #[test]
    fn colors_parse_strictly() {
        assert_eq!(parse_color("#3ACB3C").unwrap(), [0x3A, 0xCB, 0x3C]);
        assert_eq!(parse_color("ffffff").unwrap(), [255, 255, 255]);
        assert!(parse_color("#fff").is_err());
        assert!(parse_color("#GGGGGG").is_err());
    }

    #[test]
    fn air_is_never_assigned_to_a_block() {
        let loaded = Loaded { blocks: vec![BlockDef::simple("a:x", "x", [0; 3])], ..Loaded::default() };
        let content = ContentData::assign(loaded, &[], &[]).unwrap();
        assert_eq!(content.block_id("a:x"), Some(Block(1)));
        assert!(content.states[0].is_none());
    }

    #[test]
    fn reload_keeps_existing_numbers() {
        let simple = |ids: &[&str]| Loaded {
            blocks: ids.iter().map(|id| BlockDef::simple(id, id, [0; 3])).collect(),
            ..Loaded::default()
        };
        let first = ContentData::assign(simple(&["a:x", "a:y", "a:z"]), &[], &[]).unwrap();
        assert_eq!(first.block_id("a:y"), Some(Block(2)));

        // Se retira "a:x" y aparece "a:b", que ordenado iría antes que los demás.
        let second = ContentData::assign(
            simple(&["a:b", "a:y", "a:z"]),
            &first.state_names(),
            &first.item_names(),
        )
        .unwrap();
        assert_eq!(second.block_id("a:y"), Some(Block(2)));
        assert_eq!(second.block_id("a:z"), Some(Block(3)));
        assert_eq!(second.block_id("a:b"), Some(Block(4)));
        assert_eq!(second.block_id("a:x"), None);
        assert!(second.states[1].is_none());
        // Los ítems también conservan su índice.
        assert_eq!(second.item_id("a:y"), first.item_id("a:y"));
    }

    #[test]
    fn oriented_blocks_have_one_number_per_state() {
        let loaded = Loaded { blocks: vec![oriented("a:log"), BlockDef::simple("a:stone", "s", [0; 3])], ..Loaded::default() };
        let content = ContentData::assign(loaded, &[], &[]).unwrap();
        assert_eq!(content.states.len(), 5); // aire + 3 + 1
        let up = content.block_id("a:log").unwrap();
        assert_eq!(content.state_name(up).as_deref(), Some("a:log[axis=y]"));
        let x = content.block_id("a:log[axis=x]").unwrap();
        assert_ne!(x, up);
        assert_eq!(content.default_state(x), up);
        assert_eq!(content.block_id("a:log[axis=w]"), None);
        assert_eq!(content.block_id("a:stone[axis=x]"), None);

        // Colocar sobre una pared en X tumba el tronco en X.
        assert_eq!(content.orient(up, IVec3::X, Vec3::NEG_X), x);
        assert_eq!(content.orient(up, IVec3::Y, Vec3::NEG_Y), up);
        // El ítem coloca el estado por defecto y se encuentra desde cualquier estado.
        let item = content.item_for_block(x).unwrap();
        assert_eq!(content.item(item).unwrap().block, Some(up));
    }

    #[test]
    fn a_block_that_starts_orienting_keeps_its_number() {
        let plain = Loaded { blocks: vec![BlockDef::simple("a:log", "l", [0; 3])], ..Loaded::default() };
        let first = ContentData::assign(plain, &[], &[]).unwrap();
        let now = Loaded { blocks: vec![oriented("a:log")], ..Loaded::default() };
        let second = ContentData::assign(now, &first.state_names(), &[]).unwrap();
        assert_eq!(second.block_id("a:log"), first.block_id("a:log"));
    }

    #[test]
    fn files_inherit_from_parents_and_templates() {
        let dir = temp_dir("inherit");
        write(&dir, "base/blocks/_leaves.toml", "template = true\ncategory = \"Nature\"\ntags = [\"leaves\"]\nsolid = false\n");
        write(&dir, "base/blocks/leaves.toml", "parent = \"base:_leaves\"\nname = \"Leaves\"\ncolor = \"#00FF00\"\n");
        write(&dir, "base/blocks/red_leaves.toml", "parent = \"base:leaves\"\nname = \"Red\"\ncolor = \"#FF0000\"\n");
        write(&dir, "base/blocks/log.toml", "name = \"Log\"\norientation = \"axis\"\n[faces]\nside = \"#804000\"\ntop = \"#C08040\"\n");
        write(&dir, "mod/items/wand.toml", "name = \"Wand\"\ncolor = \"#FFFFFF\"\ntool = \"selection\"\ncategory = \"Tools\"\n");
        write(&dir, "mod/items/stack.toml", "block = \"base:log[axis=x]\"\n");
        let content = ContentData::assign(load(&dir).unwrap(), &[], &[]).unwrap();

        // La plantilla no es un bloque; sus campos llegan a toda la descendencia.
        assert!(content.block_id("base:_leaves").is_none());
        let red = content.block(content.block_id("base:red_leaves").unwrap()).unwrap();
        assert_eq!((red.color, red.solid, red.category.as_str()), ([255, 0, 0], false, "Nature"));
        assert_eq!(red.tags, ["leaves"]);

        let log = content.block(content.block_id("base:log").unwrap()).unwrap();
        assert_eq!((log.color, log.faces.top, log.faces.bottom), ([0x80, 0x40, 0], [0xC0, 0x80, 0x40], [0x80, 0x40, 0]));

        let wand = content.item(content.item_id("mod:wand").unwrap()).unwrap();
        assert_eq!((wand.block, wand.tool.as_deref()), (None, Some("selection")));
        // Un ítem con bloque toma de él lo que no diga, y coloca su estado por defecto.
        let stack = content.item(content.item_id("mod:stack").unwrap()).unwrap();
        assert_eq!((stack.name.as_str(), stack.block), ("Log", content.block_id("base:log")));
        assert_eq!(content.categories(), ["Blocks", "Nature", "Tools"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn icons_come_from_the_files_and_reach_the_items() {
        let dir = temp_dir("icons");
        let mut image = IconImage::new(2, 2);
        image.rgba.fill(200);
        std::fs::create_dir_all(dir.join("base/icons")).unwrap();
        std::fs::write(dir.join("base/icons/star.png"), icon::encode_png(&image).unwrap()).unwrap();
        write(&dir, "base/blocks/crate.toml", "name = \"Crate\"\ncolor = \"#806040\"\n[icon]\nrotation = [20, 30, 0]\n");
        write(&dir, "base/items/star.toml", "name = \"Star\"\ncolor = \"#FFFF00\"\n[icon]\nimage = \"base:star\"\nscale = 0.8\n");
        write(&dir, "base/items/plain.toml", "name = \"Plain\"\ncolor = \"#FFFFFF\"\n");
        let content = ContentData::assign(load(&dir).unwrap(), &[], &[]).unwrap();
        let item = |id: &str| content.item(content.item_id(id).unwrap()).unwrap().icon.clone();

        // El ítem de un bloque toma el [icon] del bloque.
        assert_eq!((item("base:crate").rotation, item("base:crate").scale), ([20.0, 30.0, 0.0], 1.0));
        assert_eq!((item("base:star").image, item("base:star").scale), (Some(image), 0.8));
        assert_eq!(item("base:plain"), IconSpec::default());

        write(&dir, "base/items/plain.toml", "name = \"Plain\"\ncolor = \"#FFFFFF\"\n[icon]\nimage = \"base:none\"\n");
        let error = format!("{:#}", load(&dir).err().unwrap());
        assert!(error.contains("base:none"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_files_are_explained() {
        let dir = temp_dir("broken");
        write(&dir, "base/blocks/a.toml", "parent = \"base:b\"\nname = \"A\"\ncolor = \"#000000\"\n");
        write(&dir, "base/blocks/b.toml", "parent = \"base:a\"\n");
        let error = format!("{:#}", load(&dir).err().unwrap());
        assert!(error.contains("cycle"), "{error}");

        write(&dir, "base/blocks/b.toml", "name = \"B\"\ncolor = \"#000000\"\nlight = 20\n");
        let error = format!("{:#}", load(&dir).err().unwrap());
        assert!(error.contains("light"), "{error}");

        write(&dir, "base/blocks/b.toml", "name = \"B\"\ncolor = \"#000000\"\n");
        write(&dir, "base/items/x.toml", "block = \"base:nope\"\n");
        let error = format!("{:#}", ContentData::assign(load(&dir).unwrap(), &[], &[]).err().unwrap());
        assert!(error.contains("base:nope"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_cell_blocks_are_one_object_with_a_state_per_cell() {
        let door = Model { size: [8, 16, 2], voxels: vec![1; 256], palette: vec![[1; 3]] };
        let bed = Model { size: [16, 4, 8], voxels: vec![1; 512], palette: vec![[1; 3]] };
        let models = vec![
            ModelDef { id: "a:bed".into(), model: bed },
            ModelDef { id: "a:door".into(), model: door },
        ];
        let horizontal = |id: &str| BlockDef {
            model: Some(id.into()),
            orientation: Orientation::Horizontal,
            ..BlockDef::simple(id, id, [1; 3])
        };
        // Una mitad suelta, con `cell`, sigue siendo un bloque de una celda.
        let half = BlockDef { cell: Some([0, 1, 0]), ..horizontal("a:half") };
        let half = BlockDef { model: Some("a:door".into()), ..half };
        let loaded = Loaded { blocks: vec![horizontal("a:bed"), horizontal("a:door"), half], items: vec![], models };
        let content = ContentData::assign(loaded, &[], &[]).unwrap();

        // 4 orientaciones × 2 celdas, con nombre; sin nada, la de abajo mirando al norte.
        let door = content.block_id("a:door").unwrap();
        assert_eq!(content.state_name(door).as_deref(), Some("a:door[facing=north,part=0-0-0]"));
        let top_east = content.block_id("a:door[facing=east,part=0-1-0]").unwrap();
        assert_eq!(content.default_state(top_east), door);
        assert_eq!(content.item_for_block(top_east), content.item_id("a:door"));
        assert_eq!(content.state_name(content.block_id("a:half").unwrap()).as_deref(), Some("a:half[facing=north]"));

        // Desde cualquiera de sus celdas se encuentra el objeto entero.
        let pos = IVec3::new(5, 10, 5);
        let east = content.orient(door, IVec3::Y, Vec3::NEG_X);
        let whole = content.structure(east, pos);
        assert_eq!(whole, vec![(pos, east), (pos + IVec3::Y, top_east)]);
        assert_eq!(content.structure(top_east, pos + IVec3::Y), whole);

        // La cama (2 celdas en X) gira entera: mirando al este se extiende hacia +Z.
        let bed = content.block_id("a:bed[facing=east]").unwrap();
        let cells: Vec<IVec3> = content.structure(bed, pos).into_iter().map(|(p, _)| p).collect();
        assert_eq!(cells, [pos, pos + IVec3::Z]);
        assert_eq!(content.structure(content.block_id("a:half").unwrap(), pos).len(), 1);
    }

    #[test]
    fn horizontal_blocks_face_whoever_places_them() {
        let o = Orientation::Horizontal;
        assert_eq!(o.place(IVec3::Y, Vec3::new(0.0, -0.5, 1.0)), 0); // mira al sur
        assert_eq!(o.place(IVec3::Y, Vec3::new(0.0, -0.5, -1.0)), 2);
        assert_eq!(o.place(IVec3::Y, Vec3::new(1.0, 0.0, 0.2)), 3);
        assert_eq!(o.place(IVec3::Y, Vec3::new(-1.0, 0.0, 0.2)), 1);
    }

    #[test]
    fn items_match_searches() {
        let content = test_content(&["stone", "dirt"]);
        let stone = content.item(content.item_id("base:stone").unwrap()).unwrap();
        assert!(stone.matches("STO") && stone.matches("") && !stone.matches("dirt"));
        assert!(stone.matches("block")); // categoría por defecto
    }
}
