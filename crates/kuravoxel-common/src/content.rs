//! Contenido del juego definido en archivos (paquetes de contenido).
//!
//! El servidor carga los paquetes, asigna a cada bloque un número y envía el resultado
//! (`ContentData`) a los clientes. Nada del contenido está fijado en el código.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use crate::block::Block;

/// Mini-voxeles por metro de los objetos pequeños (12,5 cm cada uno). Reservado: los
/// modelos de mini-voxeles aún no se cargan ni se dibujan.
pub const MICRO_VOXELS_PER_METER: u32 = 8;
/// Un byte por voxel, con el 0 reservado para el aire.
pub const MAX_BLOCKS: usize = 255;

/// Definición de un bloque, tal como viaja a los clientes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockDef {
    /// Identificador global, `paquete:nombre`.
    pub id: String,
    /// Nombre visible.
    pub name: String,
    /// Color plano sRGB.
    pub color: [u8; 3],
    /// Modelo de mini-voxeles (reservado, aún sin efecto).
    pub model: Option<String>,
}

/// Todo el contenido activo en un servidor. El índice en `blocks` es el número de bloque
/// que aparece en los chunks; el 0 (aire) y los bloques retirados son `None`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContentData {
    pub blocks: Vec<Option<BlockDef>>,
}

impl ContentData {
    pub fn block_id(&self, id: &str) -> Option<Block> {
        self.blocks
            .iter()
            .position(|block| block.as_ref().is_some_and(|block| block.id == id))
            .map(|index| Block(index as u8))
    }

    pub fn block_count(&self) -> usize {
        self.blocks.iter().flatten().count()
    }

    /// Construye el contenido a partir de lo leído del disco.
    ///
    /// Con `previous`, los bloques que ya existían conservan su número: los chunks que ya
    /// están en memoria (y en los clientes) siguen siendo válidos tras una recarga. Los
    /// bloques nuevos se añaden al final y los retirados dejan su hueco vacío.
    pub fn assign(defs: Vec<BlockDef>, previous: Option<&ContentData>) -> anyhow::Result<Self> {
        let mut blocks: Vec<Option<BlockDef>> = match previous {
            Some(previous) => vec![None; previous.blocks.len()],
            None => vec![None],
        };
        let mut fresh = Vec::new();
        for def in defs {
            match previous.and_then(|previous| previous.block_id(&def.id)) {
                Some(block) => blocks[block.0 as usize] = Some(def),
                None => fresh.push(def),
            }
        }
        blocks.extend(fresh.into_iter().map(Some));
        if blocks.len() > MAX_BLOCKS + 1 {
            bail!("too many blocks: {} (maximum {MAX_BLOCKS})", blocks.len() - 1);
        }
        Ok(Self { blocks })
    }
}

/// Un archivo de bloque en disco.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockFile {
    name: String,
    color: String,
    id: Option<String>,
    model: Option<String>,
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

/// Lee todos los bloques de todos los paquetes de `dir`.
///
/// Si dos archivos definen el mismo identificador gana el del paquete cargado más tarde.
pub fn load_blocks(dir: &Path) -> anyhow::Result<Vec<BlockDef>> {
    // BTreeMap: orden estable por identificador, para que los números asignados no
    // dependan del orden en que el sistema de archivos liste las carpetas.
    let mut blocks: BTreeMap<String, BlockDef> = BTreeMap::new();
    for pack in packs(dir)? {
        let pack_name = stem(&pack);
        let folder = pack.join("blocks");
        if !folder.is_dir() {
            continue;
        }
        for path in sorted_entries(&folder)? {
            if path.extension().is_none_or(|ext| ext != "toml") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let file: BlockFile =
                toml::from_str(&text).with_context(|| format!("error in {}", path.display()))?;
            let color =
                parse_color(&file.color).with_context(|| format!("error in {}", path.display()))?;
            let id = file.id.unwrap_or_else(|| format!("{pack_name}:{}", stem(&path)));
            blocks.insert(id.clone(), BlockDef { id, name: file.name, color, model: file.model });
        }
    }
    if blocks.is_empty() {
        bail!("no blocks defined in {}", dir.display());
    }
    Ok(blocks.into_values().collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn def(id: &str) -> BlockDef {
        BlockDef { id: id.into(), name: id.into(), color: [1, 2, 3], model: None }
    }

    #[test]
    fn colors_parse_strictly() {
        assert_eq!(parse_color("#3ACB3C").unwrap(), [0x3A, 0xCB, 0x3C]);
        assert_eq!(parse_color("ffffff").unwrap(), [255, 255, 255]);
        assert!(parse_color("#fff").is_err());
        assert!(parse_color("#GGGGGG").is_err());
    }

    #[test]
    fn reload_keeps_existing_numbers() {
        let first = ContentData::assign(vec![def("a:x"), def("a:y"), def("a:z")], None).unwrap();
        assert_eq!(first.block_id("a:y"), Some(Block(2)));

        // Se retira "a:x" y aparece "a:b", que ordenado iría antes que los demás.
        let second =
            ContentData::assign(vec![def("a:b"), def("a:y"), def("a:z")], Some(&first)).unwrap();
        assert_eq!(second.block_id("a:y"), Some(Block(2)));
        assert_eq!(second.block_id("a:z"), Some(Block(3)));
        assert_eq!(second.block_id("a:b"), Some(Block(4)));
        assert_eq!(second.block_id("a:x"), None);
        assert!(second.blocks[1].is_none());
    }
}
