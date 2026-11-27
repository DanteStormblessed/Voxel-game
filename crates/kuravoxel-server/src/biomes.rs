//! Definición del terreno por datos: el mundo (`worldgen/world.toml`) y sus biomas
//! (`worldgen/biomes/*.toml`), leídos de los paquetes de contenido.
//!
//! El mundo es una franja: los biomas se suceden de norte a sur en el orden de
//! `world.toml`, con fronteras onduladas y una zona de mezcla. Cada bioma es un grafo de
//! nodos (`graph.rs`) que da la altura, más unas reglas que eligen los bloques de la
//! superficie, los árboles y las plantas. Los nodos de `world.toml` son comunes: todos
//! los biomas pueden usarlos (la cordillera, la costa), y así casan entre sí.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use anyhow::{bail, Context};
use serde::Deserialize;

use kuravoxel_common::block::Block;
use kuravoxel_common::content::{self, ContentData};

use crate::graph::{At, Input, NodeDef, Program, Regs, MAX_OPS};
use crate::noise::{fbm2, hash2, smoothstep};

pub const SEA_LEVEL: i32 = 0;
/// Carpeta de cada paquete con la definición del terreno.
pub const FOLDER: &str = "worldgen";
/// Dentro de `worldgen/`: los perfiles de cada bioma, `profiles/<bioma>/<perfil>.toml`.
/// Un perfil es otra versión entera del archivo del bioma; cada mundo elige cuál usa.
pub const PROFILES: &str = "profiles";

/// Archivo de un perfil de un bioma (`biome_file` es el nombre del archivo del bioma, sin
/// paquete ni extensión), dentro de la carpeta `worldgen/` de su paquete.
pub fn profile_path(worldgen: &Path, biome_file: &str, profile: &str) -> std::path::PathBuf {
    worldgen.join(PROFILES).join(biome_file).join(format!("{profile}.toml"))
}

fn default_width() -> i32 {
    5120
}
fn default_length() -> i32 {
    32768
}
fn default_blend() -> f32 {
    400.0
}
fn default_wobble() -> f32 {
    300.0
}
fn default_wobble_scale() -> f32 {
    1400.0
}
fn default_stone() -> String {
    "base:stone".into()
}
fn default_water() -> String {
    "base:water".into()
}
fn default_height() -> String {
    "height".into()
}

/// `worldgen/world.toml`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorldFile {
    /// Zona jugable en metros: ancho (oeste-este) y largo (norte-sur).
    #[serde(default = "default_width")]
    pub width: i32,
    #[serde(default = "default_length")]
    pub length: i32,
    /// Biomas de norte a sur.
    pub biomes: Vec<String>,
    /// Ancho de la zona en la que dos biomas vecinos se mezclan, en metros.
    #[serde(default = "default_blend")]
    pub blend: f32,
    /// Cuánto serpentean las fronteras entre biomas, y el tamaño de esas ondas.
    #[serde(default = "default_wobble")]
    pub border_wobble: f32,
    #[serde(default = "default_wobble_scale")]
    pub border_scale: f32,
    #[serde(default = "default_stone")]
    pub stone: String,
    #[serde(default = "default_water")]
    pub water: String,
    /// Nodos comunes a todos los biomas.
    #[serde(default)]
    pub nodes: BTreeMap<String, NodeDef>,    /// Datos del editor de biomas (dónde está cada nodo en el lienzo). El generador no
    /// los usa.
    #[serde(default)]
    pub editor: Option<toml::Value>,
}

/// `worldgen/biomes/<nombre>.toml`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BiomeFile {
    pub name: Option<String>,
    /// Color del bioma en el mapa de biomas de la vista previa.
    pub color: Option<String>,
    /// Nodo que da la altura del terreno, en metros sobre el nivel del mar.
    #[serde(default = "default_height")]
    pub height: String,
    #[serde(default)]
    pub nodes: BTreeMap<String, NodeDef>,
    /// Reglas de superficie: vale la primera que se cumple.
    #[serde(default)]
    pub surface: Vec<SurfaceFile>,
    #[serde(default)]
    pub trees: Vec<TreeFile>,
    #[serde(default)]
    pub plants: Vec<PlantFile>,    /// Datos del editor de biomas (dónde está cada nodo en el lienzo). El generador no
    /// los usa.
    #[serde(default)]
    pub editor: Option<toml::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFile {
    /// Altura mínima / máxima del terreno, en metros.
    pub above: Option<f32>,
    pub below: Option<f32>,
    /// Pendiente mínima / máxima, en metros de desnivel por metro.
    pub steeper: Option<f32>,
    pub flatter: Option<f32>,
    /// Un nodo del grafo y el rango en el que debe estar.
    pub node: Option<String>,
    pub node_above: Option<f32>,
    pub node_below: Option<f32>,
    /// Tramado de los límites de altura, en metros, para que no sean líneas rectas.
    pub jitter: Option<f32>,
    pub top: String,
    pub under: Option<String>,
    /// Capas de `under` entre la superficie y la roca.
    pub depth: Option<u8>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TreeShape {
    /// Copa redondeada sobre el tronco.
    #[default]
    Round,
    /// Cono estrecho alrededor del tronco, como una conífera.
    Pine,
    /// Solo la copa, a ras de suelo.
    Bush,
    /// Solo el tronco: un cactus, un poste de roca.
    Pillar,
}

fn default_trunk() -> [i32; 2] {
    [4, 6]
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TreeFile {
    /// Fracción de las casillas de árbol (7 × 7 m) que tienen uno: un número o un nodo.
    pub density: Input,
    pub wood: String,
    pub leaves: Vec<String>,
    /// Altura del tronco, mínima y máxima.
    #[serde(default = "default_trunk")]
    pub trunk: [i32; 2],
    #[serde(default)]
    pub shape: TreeShape,
    /// Bloques de superficie sobre los que crece; vacío = cualquiera en tierra firme.
    #[serde(default)]
    pub on: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlantFile {
    /// Fracción de las columnas con esta planta: un número o un nodo.
    pub density: Input,
    pub block: String,
    #[serde(default)]
    pub on: Vec<String>,
}

/// El terreno tal como está escrito, sin resolver bloques ni semilla.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldSource {
    pub world: WorldFile,
    /// Por identificador (`paquete:archivo`).
    pub biomes: BTreeMap<String, BiomeFile>,
}

impl WorldSource {
    /// Lee `worldgen/` de todos los paquetes. Un bioma con el mismo identificador en un
    /// paquete posterior sustituye al anterior, y manda el último `world.toml`.
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        Self::load_with(dir, &BTreeMap::new())
    }

    /// Como `load`, pero los biomas de `profiles` (identificador → perfil) se leen de ese
    /// perfil. Un perfil que no existe deja el archivo por defecto (y lo avisa).
    pub fn load_with(dir: &Path, profiles: &BTreeMap<String, String>) -> anyhow::Result<Self> {
        let mut world = None;
        let mut biomes = BTreeMap::new();
        for pack in content::packs(dir)? {
            let folder = pack.join(FOLDER);
            let world_path = folder.join("world.toml");
            if world_path.is_file() {
                world = Some(read_toml::<WorldFile>(&world_path)?);
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
                let path = match profiles.get(&id) {
                    Some(profile) => {
                        let chosen = profile_path(&folder, &content::stem(&path), profile);
                        if chosen.is_file() {
                            chosen
                        } else {
                            log::warn!("biome {id}: profile \"{profile}\" not found, using the default file");
                            path
                        }
                    }
                    None => path,
                };
                biomes.insert(id, read_toml::<BiomeFile>(&path)?);
            }
        }
        let world = world
            .with_context(|| format!("no {FOLDER}/world.toml in any content pack of {}", dir.display()))?;
        Ok(Self { world, biomes })
    }

    /// El mismo terreno sin los datos del editor: dos fuentes que solo se diferencian en
    /// cómo se ven sus nodos en el lienzo generan el mismo mundo.
    pub fn without_editor(mut self) -> Self {
        self.world.editor = None;
        for biome in self.biomes.values_mut() {
            biome.editor = None;
        }
        self
    }

    /// Lo mismo desde texto: `biomes` son pares (identificador, TOML).
    pub fn parse(world: &str, biomes: &[(&str, &str)]) -> anyhow::Result<Self> {
        let world = toml::from_str(world).context("error in world.toml")?;
        let biomes = biomes
            .iter()
            .map(|(id, text)| {
                let file = toml::from_str(text).with_context(|| format!("error in biome {id}"))?;
                Ok((id.to_string(), file))
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(Self { world, biomes })
    }

    /// Identificador completo de un bioma escrito entero (`base:desert`) o sin paquete.
    pub fn resolve(&self, name: &str) -> anyhow::Result<&str> {
        if let Some((id, _)) = self.biomes.get_key_value(name) {
            return Ok(id);
        }
        let mut found = self.biomes.keys().filter(|id| id.split_once(':').is_some_and(|(_, n)| n == name));
        match (found.next(), found.next()) {
            (Some(id), None) => Ok(id),
            (Some(_), Some(_)) => bail!("biome name \"{name}\" is ambiguous: write its pack too"),
            _ => bail!("unknown biome \"{name}\""),
        }
    }
}

fn refs(roots: &[String]) -> Vec<&str> {
    roots.iter().map(String::as_str).collect()
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<T> {
    let text = std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("error in {}", path.display()))
}

/// Un nombre de perfil válido: minúsculas, números y `_` (es un nombre de archivo).
pub fn valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name != "default"
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Carpeta `worldgen/` del paquete de un bioma (`base:plains` → `content/base/worldgen`).
fn biome_folder(dir: &Path, id: &str) -> Option<std::path::PathBuf> {
    let (pack, _) = id.split_once(':')?;
    let pack = content::packs(dir).ok()?.into_iter().find(|path| content::stem(path) == pack)?;
    Some(pack.join(FOLDER))
}

/// El archivo de un perfil de un bioma, si existe.
pub fn find_profile(dir: &Path, id: &str, profile: &str) -> Option<std::path::PathBuf> {
    let (_, file) = id.split_once(':')?;
    let path = profile_path(&biome_folder(dir, id)?, file, profile);
    path.is_file().then_some(path)
}

/// Perfiles guardados de un bioma, por nombre.
pub fn list_profiles(dir: &Path, id: &str) -> Vec<String> {
    let (Some(folder), Some((_, file))) = (biome_folder(dir, id), id.split_once(':')) else { return Vec::new() };
    let Ok(entries) = content::sorted_entries(&folder.join(PROFILES).join(file)) else { return Vec::new() };
    entries
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .map(|path| content::stem(&path))
        .filter(|name| valid_profile_name(name))
        .collect()
}

/// Huella de los archivos de terreno: cambia cuando se añade, borra o modifica alguno.
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
    if let Ok(packs) = std::fs::read_dir(dir) {
        for pack in packs.flatten() {
            visit(&pack.path().join(FOLDER), &mut count, &mut newest);
        }
    }
    (count, newest)
}

/// Un número fijo o el registro de un nodo.
#[derive(Clone, Copy, Debug)]
enum Value {
    Fixed(f32),
    Reg(u16),
}

impl Value {
    fn get(self, regs: &Regs) -> f32 {
        match self {
            Value::Fixed(value) => value,
            Value::Reg(reg) => regs[reg as usize],
        }
    }
}

#[derive(Clone, Debug)]
struct Rule {
    above: Option<f32>,
    below: Option<f32>,
    steeper: Option<f32>,
    flatter: Option<f32>,
    node: Option<(u16, f32, f32)>,
    jitter: f32,
    top: Block,
    under: Block,
    depth: i32,
}

#[derive(Clone, Debug)]
pub struct TreeRule {
    density: Value,
    pub wood: Block,
    pub leaves: Vec<Block>,
    pub trunk: [i32; 2],
    pub shape: TreeShape,
    pub on: Vec<Block>,
}

impl TreeRule {
    /// ¿Se ve de lejos como una copa? Un tronco pelado no.
    pub fn has_crown(&self) -> bool {
        self.shape != TreeShape::Pillar
    }

    /// ¿Crece sobre una superficie de este bloque a esta altura?
    pub fn grows_on(&self, top: Block, height: i32) -> bool {
        height > SEA_LEVEL && (self.on.is_empty() || self.on.contains(&top))
    }
}

#[derive(Clone, Debug)]
struct PlantRule {
    density: Value,
    block: Block,
    on: Vec<Block>,
}

/// Un bioma compilado.
#[derive(Clone, Debug)]
pub struct Biome {
    pub id: String,
    pub name: String,
    pub color: [u8; 3],
    height: Program,
    height_reg: u16,
    /// Nodos que consultan las reglas de superficie y las plantas.
    detail: Program,
    rules: Vec<Rule>,
    plants: Vec<PlantRule>,
    /// Nodos de densidad de los árboles.
    forest: Program,
    trees: Vec<TreeRule>,
}

/// Bloques de una columna: superficie, subsuelo y la planta que lleva encima (o aire).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub top: Block,
    pub under: Block,
    pub depth: i32,
    pub plant: Block,
}

/// El terreno compilado para una semilla y un contenido: lo que consulta el generador.
#[derive(Clone, Debug)]
pub struct Terrain {
    pub width: i32,
    pub length: i32,
    pub stone: Block,
    pub water: Block,
    seed: u32,
    /// Biomas de norte a sur.
    bands: Vec<Biome>,
    /// Mitad de la zona de mezcla, en fracción de banda.
    blend: f32,
    wobble: f32,
    wobble_inv: f64,
}

fn parse_color(text: &str) -> anyhow::Result<[u8; 3]> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 || !hex.is_ascii() {
        bail!("invalid color \"{text}\" (expected #RRGGBB)");
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16);
    match (channel(0), channel(2), channel(4)) {
        (Ok(r), Ok(g), Ok(b)) => Ok([r, g, b]),
        _ => bail!("invalid color \"{text}\" (expected #RRGGBB)"),
    }
}

impl Biome {
    fn compile(
        id: &str,
        file: &BiomeFile,
        shared: &BTreeMap<String, NodeDef>,
        content: &ContentData,
        seed: u32,
    ) -> anyhow::Result<Self> {
        // Los nodos del bioma tapan a los comunes del mismo nombre.
        let mut nodes = shared.clone();
        nodes.extend(file.nodes.iter().map(|(name, node)| (name.clone(), node.clone())));
        let block = |name: &str| {
            content.block_id(name).with_context(|| format!("unknown block \"{name}\""))
        };
        let blocks = |names: &[String]| names.iter().map(|name| block(name)).collect::<anyhow::Result<Vec<_>>>();

        let (height, regs) = Program::compile(&nodes, &[file.height.as_str()], seed)
            .context("in the height graph")?;
        let height_reg = regs[0];

        // Un solo programa para todo lo que consulta la superficie.
        let mut detail_roots: Vec<String> = Vec::new();
        let root = |roots: &mut Vec<String>, name: &str| -> u16 {
            let position = roots.iter().position(|r| r == name).unwrap_or_else(|| {
                roots.push(name.to_string());
                roots.len() - 1
            });
            position as u16
        };
        let mut rule_nodes = Vec::new();
        for rule in &file.surface {
            rule_nodes.push(rule.node.as_deref().map(|name| root(&mut detail_roots, name)));
        }
        let mut plant_nodes = Vec::new();
        for plant in &file.plants {
            plant_nodes.push(match &plant.density {
                Input::Value(value) => Err(*value),
                Input::Node(name) => Ok(root(&mut detail_roots, name)),
            });
        }
        let (detail, detail_regs) =
            Program::compile(&nodes, &refs(&detail_roots), seed).context("in the surface rules")?;

        let mut rules = Vec::new();
        for (rule, node) in file.surface.iter().zip(rule_nodes) {
            let top = block(&rule.top)?;
            if node.is_none() && (rule.node_above.is_some() || rule.node_below.is_some()) {
                bail!("a surface rule has \"node_above\" / \"node_below\" without \"node\"");
            }
            rules.push(Rule {
                above: rule.above,
                below: rule.below,
                steeper: rule.steeper,
                flatter: rule.flatter,
                node: node.map(|index| {
                    (
                        detail_regs[index as usize],
                        rule.node_above.unwrap_or(f32::NEG_INFINITY),
                        rule.node_below.unwrap_or(f32::INFINITY),
                    )
                }),
                jitter: rule.jitter.unwrap_or(2.0),
                top,
                under: rule.under.as_deref().map(block).transpose()?.unwrap_or(top),
                depth: rule.depth.unwrap_or(3) as i32,
            });
        }
        let mut plants = Vec::new();
        for (plant, node) in file.plants.iter().zip(plant_nodes) {
            plants.push(PlantRule {
                density: match node {
                    Ok(index) => Value::Reg(detail_regs[index as usize]),
                    Err(value) => Value::Fixed(value),
                },
                block: block(&plant.block)?,
                on: blocks(&plant.on)?,
            });
        }

        let mut forest_roots: Vec<String> = Vec::new();
        let tree_nodes: Vec<Result<u16, f32>> = file
            .trees
            .iter()
            .map(|tree| match &tree.density {
                Input::Value(value) => Err(*value),
                Input::Node(name) => Ok(root(&mut forest_roots, name)),
            })
            .collect();
        let (forest, forest_regs) =
            Program::compile(&nodes, &refs(&forest_roots), seed).context("in the tree rules")?;
        let mut trees = Vec::new();
        for (tree, node) in file.trees.iter().zip(tree_nodes) {
            let [low, high] = tree.trunk;
            if low < 1 || high < low || high > 24 {
                bail!("a tree has an invalid \"trunk\" range (use 1 to 24, minimum first)");
            }
            if tree.leaves.is_empty() {
                bail!("a tree has no \"leaves\"");
            }
            trees.push(TreeRule {
                density: match node {
                    Ok(index) => Value::Reg(forest_regs[index as usize]),
                    Err(value) => Value::Fixed(value),
                },
                wood: block(&tree.wood)?,
                leaves: blocks(&tree.leaves)?,
                trunk: tree.trunk,
                shape: tree.shape,
                on: blocks(&tree.on)?,
            });
        }

        let name = file.name.clone().unwrap_or_else(|| id.to_string());
        let color = match &file.color {
            Some(color) => parse_color(color)?,
            // Sin color propio, uno cualquiera pero estable.
            None => {
                let hash = hash2(id.len() as i32, id.bytes().map(|b| b as i32).sum(), 0xC0105);
                [(hash & 0xFF) as u8 | 0x40, (hash >> 8 & 0xFF) as u8 | 0x40, (hash >> 16 & 0xFF) as u8 | 0x40]
            }
        };
        Ok(Self {
            id: id.to_string(),
            name,
            color,
            height,
            height_reg,
            detail,
            rules,
            plants,
            forest,
            trees,
        })
    }

    fn height(&self, at: At) -> f32 {
        let mut regs = [0.0; MAX_OPS];
        self.height.eval(at, &mut regs);
        regs[self.height_reg as usize]
    }
}

impl Terrain {
    /// Compila el terreno. `solo` deja un único bioma a lo largo de todo el mundo, para
    /// trabajarlo por separado.
    pub fn compile(
        source: &WorldSource,
        content: &ContentData,
        seed: u32,
        solo: Option<&str>,
    ) -> anyhow::Result<Self> {
        let world = &source.world;
        if world.width < 64 || world.length < 64 {
            bail!("the world is too small: \"width\" and \"length\" must be at least 64");
        }
        let order: Vec<&str> = match solo {
            Some(name) => vec![source.resolve(name)?],
            None => world.biomes.iter().map(|name| source.resolve(name)).collect::<anyhow::Result<_>>()?,
        };
        if order.is_empty() {
            bail!("world.toml lists no biomes");
        }
        let mut compiled: BTreeMap<&str, Biome> = BTreeMap::new();
        for id in &order {
            if !compiled.contains_key(id) {
                let biome = Biome::compile(id, &source.biomes[*id], &world.nodes, content, seed)
                    .with_context(|| format!("error in biome {id}"))?;
                compiled.insert(id, biome);
            }
        }
        let bands: Vec<Biome> = order.iter().map(|id| compiled[id].clone()).collect();
        let band_length = world.length as f32 / bands.len() as f32;
        let block = |name: &str| {
            content.block_id(name).with_context(|| format!("unknown block \"{name}\" in world.toml"))
        };
        Ok(Self {
            width: world.width,
            length: world.length,
            stone: block(&world.stone)?,
            water: block(&world.water)?,
            seed,
            blend: (world.blend.max(0.0) * 0.5 / band_length).min(0.5),
            wobble: world.border_wobble.max(0.0),
            wobble_inv: 1.0 / world.border_scale.max(1.0) as f64,
            bands,
        })
    }

    /// Biomas de norte a sur.
    pub fn bands(&self) -> &[Biome] {
        &self.bands
    }

    /// Tramo de z (sin contar las ondas de la frontera) que ocupa la banda `index`.
    pub fn band_range(&self, index: usize) -> (i32, i32) {
        let at = |i: usize| (i as f64 / self.bands.len() as f64 - 0.5) * self.length as f64;
        (at(index).round() as i32, at(index + 1).round() as i32)
    }

    fn at(&self, x: f64, z: f64) -> At {
        At {
            x,
            z,
            u: (x / self.width as f64 + 0.5) as f32,
            v: (z / self.length as f64 + 0.5) as f32,
        }
    }

    /// Posición a lo largo de las bandas: la parte entera es el índice del bioma.
    fn band_coord(&self, x: f64, z: f64) -> f32 {
        // Desfasado media celda: las fronteras caen cada pocos kilómetros exactos y, sin
        // esto, pueden coincidir con las líneas de la rejilla del ruido, donde casi no varía.
        let (nx, nz) = (x * self.wobble_inv + 0.37, z * self.wobble_inv + 0.53);
        let wobble = fbm2(nx, nz, 2, self.seed ^ 0xB10E) * self.wobble;
        ((z + wobble as f64) / self.length as f64 + 0.5) as f32 * self.bands.len() as f32
    }

    /// Los dos biomas que se mezclan en un punto y el peso del segundo (0 fuera de las
    /// zonas de mezcla; nunca más de 0,5).
    fn mix(&self, x: f64, z: f64) -> (usize, usize, f32) {
        let last = self.bands.len() - 1;
        let t = self.band_coord(x, z);
        let index = (t.floor().max(0.0) as usize).min(last);
        let frac = t - index as f32;
        if self.blend <= 0.0 {
            return (index, index, 0.0);
        }
        // Posición a través de la frontera: 0 justo en ella, 1 al salir de la mezcla.
        let (other, across) = if frac < self.blend && index > 0 {
            (index - 1, frac / self.blend)
        } else if frac > 1.0 - self.blend && index < last {
            (index + 1, (1.0 - frac) / self.blend)
        } else {
            return (index, index, 0.0);
        };
        (index, other, 1.0 - smoothstep(-1.0, 1.0, across))
    }

    /// Altura del terreno en un punto, en metros (sin redondear).
    pub fn height(&self, x: f64, z: f64) -> f32 {
        let (a, b, weight) = self.mix(x, z);
        let at = self.at(x, z);
        let height = self.bands[a].height(at);
        if weight > 0.0 {
            height + (self.bands[b].height(at) - height) * weight
        } else {
            height
        }
    }

    /// Índice del bioma que pone la superficie y la vegetación de una columna. En la
    /// zona de mezcla los dos biomas se entrelazan en manchas en vez de cortarse en línea.
    pub fn surface_band(&self, wx: i32, wz: i32) -> usize {
        let (x, z) = (wx as f64, wz as f64);
        let mut t = self.band_coord(x, z);
        if self.blend > 0.0 {
            // Lenguas largas de un bioma dentro del otro, manchas menores y algo de grano.
            let tongues = fbm2(x / 190.0 + 0.41, z / 190.0 + 0.17, 2, self.seed ^ 0x70B6);
            let patches = fbm2(x / 48.0, z / 48.0, 2, self.seed ^ 0x9A7C);
            let grain = (hash2(wx, wz, self.seed ^ 0x6A11) & 0xFF) as f32 / 255.0 - 0.5;
            t += (tongues * 1.5 + patches * 0.8 + grain * 0.25) * self.blend;
        }
        (t.floor().max(0.0) as usize).min(self.bands.len() - 1)
    }

    pub fn biome_at(&self, wx: i32, wz: i32) -> &Biome {
        &self.bands[self.surface_band(wx, wz)]
    }

    /// Bloques de la columna (wx, wz), de altura `height` y pendiente `slope`.
    pub fn surface(&self, wx: i32, wz: i32, height: i32, slope: f32) -> Surface {
        let biome = self.biome_at(wx, wz);
        let mut regs = [0.0; MAX_OPS];
        if !biome.detail.is_trivial() {
            biome.detail.eval(self.at(wx as f64, wz as f64), &mut regs);
        }
        let hash = hash2(wx, wz, self.seed ^ 0xA5A5);
        let grain = (hash & 0x3FF) as f32 / 1023.0 * 2.0 - 1.0;
        let h = height as f32;
        let rule = biome.rules.iter().find(|rule| {
            let h = h + grain * rule.jitter;
            rule.above.is_none_or(|above| h >= above)
                && rule.below.is_none_or(|below| h < below)
                && rule.steeper.is_none_or(|steeper| slope >= steeper)
                && rule.flatter.is_none_or(|flatter| slope < flatter)
                && rule.node.is_none_or(|(reg, min, max)| (min..max).contains(&regs[reg as usize]))
        });
        let Some(rule) = rule else {
            return Surface { top: self.stone, under: self.stone, depth: 0, plant: Block::AIR };
        };
        let mut plant = Block::AIR;
        if height >= SEA_LEVEL && !biome.plants.is_empty() {
            let mut roll = (hash >> 10 & 0xFFFF) as f32 / 65536.0;
            for rule_plant in &biome.plants {
                if !rule_plant.on.is_empty() && !rule_plant.on.contains(&rule.top) {
                    continue;
                }
                let density = rule_plant.density.get(&regs).clamp(0.0, 1.0);
                if roll < density {
                    plant = rule_plant.block;
                    break;
                }
                roll -= density;
            }
        }
        Surface { top: rule.top, under: rule.under, depth: rule.depth, plant }
    }

    /// El árbol que toca en un punto para una tirada `roll` (0-1), si toca alguno.
    pub fn tree(&self, wx: i32, wz: i32, mut roll: f32) -> Option<&TreeRule> {
        let biome = self.biome_at(wx, wz);
        if biome.trees.is_empty() {
            return None;
        }
        let mut regs = [0.0; MAX_OPS];
        if !biome.forest.is_trivial() {
            biome.forest.eval(self.at(wx as f64, wz as f64), &mut regs);
        }
        for tree in &biome.trees {
            let density = tree.density.get(&regs).clamp(0.0, 1.0);
            if roll < density {
                return Some(tree);
            }
            roll -= density;
        }
        None
    }

    /// Densidad total de árboles en un punto y el tipo más abundante.
    pub fn forest(&self, wx: i32, wz: i32) -> Option<(f32, &TreeRule)> {
        let biome = self.biome_at(wx, wz);
        let mut regs = [0.0; MAX_OPS];
        if !biome.forest.is_trivial() {
            biome.forest.eval(self.at(wx as f64, wz as f64), &mut regs);
        }
        let (mut total, mut best) = (0.0, None::<(f32, &TreeRule)>);
        for tree in biome.trees.iter().filter(|tree| tree.has_crown()) {
            let density = tree.density.get(&regs).clamp(0.0, 1.0);
            total += density;
            if best.is_none_or(|(most, _)| density > most) {
                best = Some((density, tree));
            }
        }
        best.map(|(_, tree)| (total, tree))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const TEST_BLOCKS: [&str; 8] = ["grass", "dirt", "stone", "sand", "water", "snow", "wood", "leaves"];

    pub const TEST_WORLD: &str = r#"
        width = 2048
        length = 4096
        biomes = ["plains", "dunes"]
        blend = 200

        [nodes.land]
        type = "smoothstep"
        input = "u"
        from = 0.1
        to = 0.3
    "#;

    pub const TEST_PLAINS: &str = r#"
        name = "Plains"
        [nodes.hills]
        type = "noise"
        scale = 120
        [nodes.height]
        type = "add"
        inputs = [20, "hills"]

        [[surface]]
        above = 40
        top = "base:snow"
        under = "base:stone"
        [[surface]]
        top = "base:grass"
        under = "base:dirt"

        [[trees]]
        density = 0.5
        wood = "base:wood"
        leaves = ["base:leaves"]
        on = ["base:grass"]
    "#;

    pub const TEST_DUNES: &str = r#"
        [nodes.height]
        type = "mix"
        a = -30
        b = 220
        t = "land"

        [[surface]]
        top = "base:sand"
        depth = 5
    "#;

    pub fn terrain() -> Terrain {
        let source = WorldSource::parse(TEST_WORLD, &[("base:plains", TEST_PLAINS), ("base:dunes", TEST_DUNES)])
            .expect("valid source");
        Terrain::compile(&source, &content::test_content(&TEST_BLOCKS), 1337, None).expect("compiles")
    }

    #[test]
    fn bands_follow_the_listed_order_and_blend_at_the_border() {
        let terrain = terrain();
        let block = |name: &str| content::test_content(&TEST_BLOCKS).block_id(name).unwrap();
        // Norte: llanura a ~20 m con hierba. Sur: dunas a 220 m de arena.
        assert_eq!(terrain.biome_at(0, -1500).id, "base:plains");
        assert_eq!(terrain.biome_at(0, 1500).id, "base:dunes");
        assert!((terrain.height(0.0, -1500.0) - 20.0).abs() < 1.5);
        assert!((terrain.height(0.0, 1500.0) - 220.0).abs() < 0.01);
        assert_eq!(terrain.surface(0, -1500, 20, 0.0).top, block("base:grass"));
        assert_eq!(terrain.surface(0, -1500, 60, 0.0).top, block("base:snow"));
        let dunes = terrain.surface(0, 1500, 220, 0.0);
        assert_eq!((dunes.top, dunes.under, dunes.depth), (block("base:sand"), block("base:sand"), 5));

        // La altura cruza la frontera sin saltos.
        let mut previous = terrain.height(0.0, -1000.0);
        let mut seen_middle = false;
        for z in -999..1000 {
            let height = terrain.height(0.0, z as f64);
            assert!((height - previous).abs() < 6.0, "jump of {} at z={z}", height - previous);
            seen_middle |= (80.0..160.0).contains(&height);
            previous = height;
        }
        assert!(seen_middle);
    }

    #[test]
    fn shared_nodes_and_solo_mode() {
        let terrain = terrain();
        // `land` es un nodo de world.toml: al oeste, las dunas quedan bajo el mar.
        assert!(terrain.height(-1000.0, 1500.0) < -29.0);
        let source = WorldSource::parse(TEST_WORLD, &[("base:plains", TEST_PLAINS), ("base:dunes", TEST_DUNES)])
            .unwrap();
        let content = content::test_content(&TEST_BLOCKS);
        let solo = Terrain::compile(&source, &content, 1337, Some("dunes")).unwrap();
        assert_eq!(solo.bands().len(), 1);
        assert_eq!(solo.biome_at(0, -1500).id, "base:dunes");
        assert!(Terrain::compile(&source, &content, 1337, Some("nope")).is_err());
    }

    #[test]
    fn trees_respect_density_and_ground() {
        let terrain = terrain();
        let content = content::test_content(&TEST_BLOCKS);
        let rule = terrain.tree(0, -1500, 0.2).expect("dense enough");
        assert!(terrain.tree(0, -1500, 0.7).is_none());
        assert!(terrain.tree(0, 1500, 0.0).is_none(), "no trees in the dunes");
        assert!(rule.grows_on(content.block_id("base:grass").unwrap(), 20));
        assert!(!rule.grows_on(content.block_id("base:snow").unwrap(), 20));
        assert!(!rule.grows_on(content.block_id("base:grass").unwrap(), -3));
        assert_eq!(terrain.forest(0, -1500).map(|(density, _)| density), Some(0.5));
    }

    #[test]
    fn mistakes_name_the_biome() {
        let content = content::test_content(&TEST_BLOCKS);
        let compile = |plains: &str| {
            let source = WorldSource::parse(TEST_WORLD, &[("base:plains", plains), ("base:dunes", TEST_DUNES)])?;
            Terrain::compile(&source, &content, 1, None)
        };
        let error = format!("{:#}", compile(&TEST_PLAINS.replace("base:snow", "base:nope")).unwrap_err());
        assert!(error.contains("base:plains") && error.contains("base:nope"), "{error}");
        let error = format!("{:#}", compile(&TEST_PLAINS.replace("\"hills\"]", "\"hils\"]")).unwrap_err());
        assert!(error.contains("hils"), "{error}");
        assert!(compile(&TEST_PLAINS.replace("above = 40", "abov = 40")).is_err());
    }
}
