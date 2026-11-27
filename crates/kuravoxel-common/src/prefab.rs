//! Prefabs: trozos de mundo guardados para volver a colocarlos (un árbol, una casa).
//!
//! Un prefab es una caja de voxeles con su propia paleta de **nombres de estado**
//! (`base:wood[axis=x]`), así que no depende de los números de bloque de ningún mundo.
//! En disco son dos archivos dentro de un paquete de contenido:
//!
//! ```text
//! content/<paquete>/prefabs/<nombre>.toml   tamaño, anclaje y paleta (se lee y se retoca)
//! content/<paquete>/prefabs/<nombre>.bin    los voxeles, por tramos
//! ```
//!
//! Nadie los escribe a mano: se crean con la herramienta de selección del juego. El
//! servidor los lee, los guarda y los coloca; el cliente solo recibe una copia del que
//! tiene en el portapapeles para enseñar dónde va a caer.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use glam::IVec3;
use serde::{Deserialize, Serialize};

use crate::block::Block;
use crate::content::ContentData;

/// Lado máximo de un prefab, en voxeles.
pub const MAX_PREFAB_SIDE: i32 = 128;
/// Voxeles que puede tener un prefab (128 × 128 × 64).
pub const MAX_PREFAB_VOLUME: u64 = 1 << 20;
/// Paquete en el que se guarda un prefab cuyo nombre no dice otro.
pub const DEFAULT_PACK: &str = "base";
/// Nombre de la entrada 0 de toda paleta.
const AIR: &str = "air";
const BIN_MAGIC: &[u8; 4] = b"KVPF";
const BIN_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct Prefab {
    /// Tamaño (x, y, z) en voxeles.
    pub size: [u16; 3],
    /// Voxel del prefab que se pone donde apunta quien lo coloca. Por defecto, el centro
    /// de la base.
    pub anchor: [i32; 3],
    /// Nombres de estado; la entrada 0 es siempre el aire.
    pub palette: Vec<String>,
    /// Índice de paleta de cada voxel (`x + z·sx + y·sx·sz`).
    pub voxels: Vec<u16>,
}

/// Lo que se enseña de un prefab guardado en una lista.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefabInfo {
    /// Identificador, `paquete:nombre` (el nombre puede llevar carpetas: `trees/oak`).
    pub id: String,
    pub size: [u16; 3],
}

/// Forma en la que viaja un prefab: los voxeles por tramos.
#[derive(Serialize, Deserialize)]
struct PrefabWire {
    size: [u16; 3],
    anchor: [i32; 3],
    palette: Vec<String>,
    runs: Vec<(u32, u16)>,
}

impl Serialize for Prefab {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        PrefabWire {
            size: self.size,
            anchor: self.anchor,
            palette: self.palette.clone(),
            runs: encode_runs(&self.voxels),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Prefab {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let wire = PrefabWire::deserialize(deserializer)?;
        Prefab::from_runs(wire.size, wire.anchor, wire.palette, &wire.runs).map_err(D::Error::custom)
    }
}

fn encode_runs(voxels: &[u16]) -> Vec<(u32, u16)> {
    let mut runs: Vec<(u32, u16)> = Vec::new();
    for &voxel in voxels {
        match runs.last_mut() {
            Some((count, value)) if *value == voxel => *count += 1,
            _ => runs.push((1, voxel)),
        }
    }
    runs
}

/// Comprueba que una caja cabe en un prefab. El error es un mensaje para el jugador.
pub fn check_size(size: IVec3) -> Result<(), String> {
    let volume = size.x as i64 * size.y as i64 * size.z as i64;
    if size.min_element() < 1 || size.max_element() > MAX_PREFAB_SIDE || volume as u64 > MAX_PREFAB_VOLUME {
        return Err(format!(
            "That box is {}x{}x{}; the limit is {MAX_PREFAB_SIDE} per side and {MAX_PREFAB_VOLUME} blocks.",
            size.x, size.y, size.z
        ));
    }
    Ok(())
}

impl Prefab {
    /// Copia una caja de `size` voxeles; `block_at` da el bloque de cada posición local.
    /// Lo que el contenido no conoce se guarda como aire.
    pub fn capture(size: IVec3, content: &ContentData, mut block_at: impl FnMut(IVec3) -> Block) -> Prefab {
        let mut palette = vec![AIR.to_string()];
        // Índice de paleta de cada número de bloque visto.
        let mut index_of: Vec<Option<u16>> = vec![None; content.states.len().max(1)];
        index_of[0] = Some(0);
        let mut voxels = Vec::with_capacity((size.x * size.y * size.z).max(0) as usize);
        for y in 0..size.y {
            for z in 0..size.z {
                for x in 0..size.x {
                    let block = block_at(IVec3::new(x, y, z));
                    let slot = index_of.get_mut(block.0 as usize);
                    let index = match slot {
                        Some(Some(index)) => *index,
                        Some(slot @ None) => match content.state_name(block) {
                            Some(name) => {
                                palette.push(name);
                                *slot.insert((palette.len() - 1) as u16)
                            }
                            None => *slot.insert(0),
                        },
                        None => 0,
                    };
                    voxels.push(index);
                }
            }
        }
        let size = [size.x as u16, size.y as u16, size.z as u16];
        Prefab { size, anchor: default_anchor(size), palette, voxels }
    }

    fn from_runs(size: [u16; 3], anchor: [i32; 3], palette: Vec<String>, runs: &[(u32, u16)]) -> Result<Prefab, String> {
        check_size(IVec3::from_array(size.map(i32::from)))?;
        if palette.first().map(String::as_str) != Some(AIR) || palette.len() > u16::MAX as usize {
            return Err("prefab palette must start with \"air\"".into());
        }
        let volume = size.iter().map(|&s| s as usize).product::<usize>();
        let mut voxels = Vec::with_capacity(volume);
        for &(count, value) in runs {
            if value as usize >= palette.len() || voxels.len() + count as usize > volume {
                return Err("prefab data is corrupt".into());
            }
            voxels.extend(std::iter::repeat_n(value, count as usize));
        }
        if voxels.len() != volume {
            return Err("prefab data is corrupt".into());
        }
        Ok(Prefab { size, anchor, palette, voxels })
    }

    pub fn size(&self) -> IVec3 {
        IVec3::from_array(self.size.map(i32::from))
    }

    pub fn anchor(&self) -> IVec3 {
        IVec3::from_array(self.anchor)
    }

    #[inline]
    pub fn index(&self, pos: IVec3) -> usize {
        let [sx, _, sz] = self.size.map(|s| s as usize);
        pos.x as usize + pos.z as usize * sx + pos.y as usize * sx * sz
    }

    /// Índice de paleta de un voxel (0 = aire, también fuera del prefab).
    #[inline]
    pub fn get(&self, pos: IVec3) -> u16 {
        let size = self.size();
        if pos.cmplt(IVec3::ZERO).any() || pos.cmpge(size).any() {
            return 0;
        }
        self.voxels[self.index(pos)]
    }

    /// Voxeles que no son aire.
    pub fn block_count(&self) -> usize {
        self.voxels.iter().filter(|&&voxel| voxel != 0).count()
    }

    /// Número de bloque de cada entrada de la paleta en `content`; `None` para los
    /// nombres que ese contenido no tiene (al colocar el prefab se saltan).
    pub fn resolve(&self, content: &ContentData) -> Vec<Option<Block>> {
        self.palette
            .iter()
            .enumerate()
            .map(|(index, name)| if index == 0 { Some(Block::AIR) } else { content.block_id(name) })
            .collect()
    }

    /// Nombres de la paleta que `content` no conoce.
    pub fn unknown_blocks(&self, content: &ContentData) -> Vec<&str> {
        self.palette
            .iter()
            .skip(1)
            .filter(|name| content.block_id(name).is_none())
            .map(String::as_str)
            .collect()
    }

    /// El prefab girado `turns` cuartos de vuelta alrededor de la vertical (norte → este).
    /// Giran las posiciones, el anclaje y también los bloques orientables: un tronco
    /// tumbado en X pasa a estarlo en Z y un horno que miraba al norte mira al este.
    pub fn rotated(&self, turns: u8, content: &ContentData) -> Prefab {
        let turns = turns % 4;
        if turns == 0 {
            return self.clone();
        }
        let palette = self
            .palette
            .iter()
            .map(|name| {
                content
                    .block_id(name)
                    .and_then(|block| content.state_name(content.rotate_y(block, turns)))
                    .unwrap_or_else(|| name.clone())
            })
            .collect();
        let mut prefab = Prefab { palette, ..self.clone() };
        for _ in 0..turns {
            let [sx, sy, sz] = prefab.size.map(i32::from);
            let turned = Prefab {
                size: [sz as u16, sy as u16, sx as u16],
                // (x, z) → (sz − 1 − z, x), igual que `Model::rotated_y`.
                anchor: [sz - 1 - prefab.anchor[2], prefab.anchor[1], prefab.anchor[0]],
                palette: Vec::new(),
                voxels: vec![0; prefab.voxels.len()],
            };
            let mut voxels = turned.voxels.clone();
            for y in 0..sy {
                for z in 0..sz {
                    for x in 0..sx {
                        let to = turned.index(IVec3::new(sz - 1 - z, y, x));
                        voxels[to] = prefab.voxels[prefab.index(IVec3::new(x, y, z))];
                    }
                }
            }
            prefab = Prefab { palette: std::mem::take(&mut prefab.palette), voxels, ..turned };
        }
        prefab
    }

    // --- Disco ---

    /// Guarda el prefab como `<base>.toml` y `<base>.bin` (crea las carpetas que falten).
    pub fn save(&self, base: &Path) -> anyhow::Result<()> {
        if let Some(dir) = base.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
        }
        let meta = PrefabFile { size: self.size, anchor: self.anchor, palette: self.palette.clone() };
        let text = toml::to_string(&meta).context("failed to encode the prefab")?;
        let mut bytes = Vec::from(*BIN_MAGIC);
        bytes.push(BIN_VERSION);
        for (count, value) in encode_runs(&self.voxels) {
            bytes.extend_from_slice(&count.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        // Primero los voxeles: si algo falla a medias, el .toml antiguo sigue casando o
        // el prefab simplemente no aparece.
        write_atomic(&base.with_extension("bin"), &bytes)?;
        write_atomic(&base.with_extension("toml"), text.as_bytes())
    }

    pub fn load(base: &Path) -> anyhow::Result<Prefab> {
        let meta = read_meta(&base.with_extension("toml"))?;
        let path = base.with_extension("bin");
        let bytes = std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        let Some(body) = bytes.strip_prefix(BIN_MAGIC).and_then(|rest| rest.strip_prefix(&[BIN_VERSION])) else {
            bail!("{} is not a prefab (or is from a newer version)", path.display());
        };
        if body.len() % 6 != 0 {
            bail!("{} is truncated", path.display());
        }
        let runs: Vec<(u32, u16)> = body
            .chunks_exact(6)
            .map(|run| {
                (u32::from_le_bytes([run[0], run[1], run[2], run[3]]), u16::from_le_bytes([run[4], run[5]]))
            })
            .collect();
        Prefab::from_runs(meta.size, meta.anchor, meta.palette, &runs)
            .map_err(|problem| anyhow::anyhow!("{}: {problem}", path.display()))
    }
}

/// Anclaje por defecto: el centro de la base.
pub fn default_anchor(size: [u16; 3]) -> [i32; 3] {
    [size[0] as i32 / 2, 0, size[2] as i32 / 2]
}

/// El `.toml` de un prefab.
#[derive(Serialize, Deserialize)]
struct PrefabFile {
    size: [u16; 3],
    anchor: [i32; 3],
    palette: Vec<String>,
}

fn read_meta(path: &Path) -> anyhow::Result<PrefabFile> {
    let text = std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("error in {}", path.display()))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let temp = path.with_extension(format!(
        "{}.tmp",
        path.extension().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&temp, bytes).with_context(|| format!("failed to write {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("failed to write {}", path.display()))
}

/// Separa lo que escribe un jugador (`oak`, `trees/oak`, `mymod:trees/oak`) en paquete y
/// nombre. Solo minúsculas, números, `_` y `-`, con `/` para las carpetas: así un nombre
/// nunca sale de la carpeta de prefabs de su paquete.
pub fn parse_id(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let (pack, name) = text.split_once(':').unwrap_or((DEFAULT_PACK, text));
    let word = |part: &str| {
        !part.is_empty()
            && part.len() <= 48
            && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'))
    };
    (word(pack) && name.len() <= 96 && name.split('/').all(word)).then(|| (pack.to_string(), name.to_string()))
}

/// Ruta, sin extensión, de los archivos de un prefab.
pub fn path_of(content_dir: &Path, pack: &str, name: &str) -> PathBuf {
    let mut path = content_dir.join(pack).join("prefabs");
    path.extend(name.split('/'));
    path
}

/// Borra los dos archivos de un prefab. `false` si no existía.
pub fn delete(base: &Path) -> anyhow::Result<bool> {
    let toml = base.with_extension("toml");
    if !toml.is_file() {
        return Ok(false);
    }
    std::fs::remove_file(&toml).with_context(|| format!("failed to delete {}", toml.display()))?;
    let _ = std::fs::remove_file(base.with_extension("bin"));
    Ok(true)
}

/// Prefabs guardados en todos los paquetes de `content_dir`, por identificador.
pub fn list(content_dir: &Path) -> Vec<PrefabInfo> {
    fn visit(dir: &Path, pack: &str, prefix: &str, out: &mut Vec<PrefabInfo>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            if path.is_dir() {
                visit(&path, pack, &format!("{prefix}{stem}/"), out);
            } else if path.extension().is_some_and(|ext| ext == "toml") {
                let id = format!("{pack}:{prefix}{stem}");
                // Un archivo que no se entiende no aparece en la lista.
                if let (Some(_), Ok(meta)) = (parse_id(&id), read_meta(&path)) {
                    out.push(PrefabInfo { id, size: meta.size });
                }
            }
        }
    }
    let mut out = Vec::new();
    let Ok(packs) = std::fs::read_dir(content_dir) else { return out };
    for pack in packs.flatten() {
        let name = pack.file_name().to_string_lossy().into_owned();
        visit(&pack.path().join("prefabs"), &name, "", &mut out);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{BlockDef, Loaded, Orientation};

    /// Piedra (1), madera con eje (x = 2, y = 3, z = 4) y horno con frente (5-8).
    fn content() -> ContentData {
        let mut wood = BlockDef::simple("base:wood", "wood", [1, 2, 3]);
        wood.orientation = Orientation::Axis;
        let mut kiln = BlockDef::simple("base:kiln", "kiln", [4, 5, 6]);
        kiln.orientation = Orientation::Horizontal;
        // `assign` espera los bloques ordenados por identificador.
        let blocks = vec![kiln, BlockDef::simple("base:stone", "stone", [9, 9, 9]), wood];
        let names = [
            "",
            "base:stone",
            "base:wood[axis=x]",
            "base:wood[axis=y]",
            "base:wood[axis=z]",
            "base:kiln[facing=north]",
            "base:kiln[facing=east]",
            "base:kiln[facing=south]",
            "base:kiln[facing=west]",
        ]
        .map(String::from);
        ContentData::assign(Loaded { blocks, ..Loaded::default() }, &names, &[]).unwrap()
    }

    /// Caja de 3×2×2 con un bloque distinto en cada esquina reconocible.
    fn sample(content: &ContentData) -> Prefab {
        Prefab::capture(IVec3::new(3, 2, 2), content, |pos| match pos.to_array() {
            [0, 0, 0] => Block(1),
            [2, 0, 0] => Block(2),
            [0, 1, 1] => Block(5),
            [1, 0, 1] => Block(99), // no existe: aire
            _ => Block::AIR,
        })
    }

    #[test]
    fn capture_builds_a_palette_of_state_names() {
        let content = content();
        let prefab = sample(&content);
        assert_eq!(prefab.size, [3, 2, 2]);
        assert_eq!(prefab.anchor, [1, 0, 1]);
        assert_eq!(prefab.palette, ["air", "base:stone", "base:wood[axis=x]", "base:kiln[facing=north]"]);
        assert_eq!(prefab.block_count(), 3);
        assert_eq!(prefab.get(IVec3::new(2, 0, 0)), 2);
        assert_eq!(prefab.get(IVec3::new(5, 0, 0)), 0);
        assert_eq!(prefab.resolve(&content), [Some(Block::AIR), Some(Block(1)), Some(Block(2)), Some(Block(5))]);
    }

    #[test]
    fn rotating_turns_positions_states_and_the_anchor() {
        let content = content();
        let prefab = sample(&content);
        let turned = prefab.rotated(1, &content);
        assert_eq!(turned.size, [2, 2, 3]);
        // (x, z) → (sz − 1 − z, x) con sz = 2.
        assert_eq!(turned.palette[turned.get(IVec3::new(1, 0, 0)) as usize], "base:stone");
        assert_eq!(turned.palette[turned.get(IVec3::new(1, 0, 2)) as usize], "base:wood[axis=z]");
        assert_eq!(turned.palette[turned.get(IVec3::new(0, 1, 0)) as usize], "base:kiln[facing=east]");
        assert_eq!(turned.anchor, [0, 0, 1]);
        assert_eq!(turned.block_count(), 3);

        // Cuatro cuartos de vuelta lo dejan como estaba; dos de una vez o de uno en uno, igual.
        assert_eq!(prefab.rotated(4, &content), prefab);
        assert_eq!(turned.rotated(3, &content), prefab);
        assert_eq!(turned.rotated(1, &content), prefab.rotated(2, &content));
    }

    #[test]
    fn a_prefab_survives_the_disk_and_the_wire() {
        let content = content();
        let prefab = sample(&content);
        let dir = std::env::temp_dir().join(format!("kuravoxel-prefab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = path_of(&dir, "base", "trees/oak");
        prefab.save(&base).unwrap();
        assert_eq!(Prefab::load(&base).unwrap(), prefab);
        assert_eq!(list(&dir), [PrefabInfo { id: "base:trees/oak".into(), size: [3, 2, 2] }]);

        let bytes = postcard::to_allocvec(&prefab).unwrap();
        assert_eq!(postcard::from_bytes::<Prefab>(&bytes).unwrap(), prefab);

        assert!(delete(&base).unwrap());
        assert!(!delete(&base).unwrap());
        assert!(list(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_cannot_leave_the_prefabs_folder() {
        assert_eq!(parse_id("oak"), Some(("base".into(), "oak".into())));
        assert_eq!(parse_id(" mymod:trees/oak_2 "), Some(("mymod".into(), "trees/oak_2".into())));
        for bad in ["", "../oak", "trees//oak", "Oak", "a:b:c", "trees/.hidden", "c:/x", "oak.toml", "/oak"] {
            assert_eq!(parse_id(bad), None, "{bad}");
        }
        assert!(check_size(IVec3::new(128, 64, 128)).is_ok());
        assert!(check_size(IVec3::new(129, 1, 1)).is_err());
        assert!(check_size(IVec3::new(128, 128, 128)).is_err());
        assert!(check_size(IVec3::new(0, 1, 1)).is_err());
    }
}
