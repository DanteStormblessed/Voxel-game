//! Guardado del mundo en disco.
//!
//! El terreno se regenera desde la semilla, así que solo se guardan los chunks que alguien
//! ha modificado. Se agrupan en regiones de 8×8×8 chunks, un archivo por región, que se
//! reescribe entero (a un temporal y con renombrado atómico) cuando cambia.
//!
//! ```text
//! worlds/<nombre>/
//!   world.toml          semilla y tabla número → identificador de bloque
//!   regions/r.X.Y.Z.bin chunks modificados de esa región
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use glam::IVec3;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use voxel_common::chunk::Chunk;

/// Chunks por lado de una región (como desplazamiento de bits).
const REGION_BITS: i32 = 3;
const REGION_FORMAT: u32 = 1;

/// Contenido de `world.toml`.
#[derive(Serialize, Deserialize)]
pub struct WorldMeta {
    pub seed: u32,
    /// Identificador de bloque de cada número (el índice). Los chunks guardados usan
    /// estos números, así que la tabla debe conservarse entre sesiones. El 0 es aire.
    pub blocks: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct RegionFile {
    format: u32,
    chunks: Vec<([i32; 3], Arc<Chunk>)>,
}

#[derive(Default)]
struct Region {
    chunks: FxHashMap<IVec3, Arc<Chunk>>,
    dirty: bool,
}

pub struct Storage {
    dir: PathBuf,
    /// Regiones ya leídas del disco (o creadas). Solo contienen chunks modificados.
    regions: FxHashMap<IVec3, Region>,
}

fn region_of(chunk: IVec3) -> IVec3 {
    IVec3::new(chunk.x >> REGION_BITS, chunk.y >> REGION_BITS, chunk.z >> REGION_BITS)
}

/// Escribe un archivo sin dejarlo nunca a medias: primero a un temporal y luego renombra.
fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes).with_context(|| format!("failed to write {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("failed to replace {}", path.display()))
}

impl Storage {
    /// Abre la carpeta de un mundo, creándola si no existe. Devuelve también sus
    /// metadatos si el mundo ya existía.
    pub fn open(dir: &Path) -> anyhow::Result<(Self, Option<WorldMeta>)> {
        std::fs::create_dir_all(dir.join("regions"))
            .with_context(|| format!("failed to create world folder {}", dir.display()))?;
        let meta_path = dir.join("world.toml");
        let meta = if meta_path.exists() {
            let text = std::fs::read_to_string(&meta_path)
                .with_context(|| format!("failed to read {}", meta_path.display()))?;
            Some(toml::from_str(&text).with_context(|| format!("error in {}", meta_path.display()))?)
        } else {
            None
        };
        Ok((Self { dir: dir.to_path_buf(), regions: FxHashMap::default() }, meta))
    }

    pub fn write_meta(&self, meta: &WorldMeta) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(meta)?;
        write_atomic(&self.dir.join("world.toml"), text.as_bytes())
    }

    fn region_path(&self, region: IVec3) -> PathBuf {
        self.dir.join("regions").join(format!("r.{}.{}.{}.bin", region.x, region.y, region.z))
    }

    fn region(&mut self, key: IVec3) -> &mut Region {
        if !self.regions.contains_key(&key) {
            let path = self.region_path(key);
            let mut region = Region::default();
            if path.exists() {
                let loaded = std::fs::read(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|bytes| Ok(postcard::from_bytes::<RegionFile>(&bytes)?));
                match loaded {
                    Ok(file) if file.format == REGION_FORMAT => {
                        region.chunks = file
                            .chunks
                            .into_iter()
                            .map(|(pos, chunk)| (IVec3::from_array(pos), chunk))
                            .collect();
                    }
                    Ok(file) => log::error!(
                        "{}: unknown region format {}, ignoring it",
                        path.display(),
                        file.format
                    ),
                    // No se pisa un archivo que no se ha podido leer: se aparta.
                    Err(error) => {
                        log::error!("{}: unreadable region, set aside: {error:#}", path.display());
                        let _ = std::fs::rename(&path, path.with_extension("corrupt"));
                    }
                }
            }
            self.regions.insert(key, region);
        }
        self.regions.get_mut(&key).expect("region inserted above")
    }

    /// Versión guardada de un chunk, si alguien lo modificó alguna vez.
    pub fn chunk(&mut self, pos: IVec3) -> Option<Arc<Chunk>> {
        self.region(region_of(pos)).chunks.get(&pos).cloned()
    }

    /// Registra la versión modificada de un chunk; se escribirá en el próximo guardado.
    pub fn put(&mut self, pos: IVec3, chunk: Arc<Chunk>) {
        let region = self.region(region_of(pos));
        region.chunks.insert(pos, chunk);
        region.dirty = true;
    }

    /// Escribe las regiones con cambios. Devuelve cuántas se han escrito.
    pub fn save(&mut self) -> anyhow::Result<usize> {
        let mut written = 0;
        let dirty: Vec<IVec3> =
            self.regions.iter().filter(|(_, region)| region.dirty).map(|(key, _)| *key).collect();
        for key in dirty {
            let path = self.region_path(key);
            let region = self.regions.get_mut(&key).expect("key taken from the map");
            let file = RegionFile {
                format: REGION_FORMAT,
                chunks: region
                    .chunks
                    .iter()
                    .map(|(pos, chunk)| (pos.to_array(), chunk.clone()))
                    .collect(),
            };
            write_atomic(&path, &postcard::to_allocvec(&file)?)?;
            region.dirty = false;
            written += 1;
        }
        Ok(written)
    }
}
