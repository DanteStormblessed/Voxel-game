//! Mundo del servidor: la única copia con autoridad de los chunks.

use std::collections::hash_map::Entry;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use glam::IVec3;
use rustc_hash::FxHashMap;
use kuravoxel_common::chunk::Chunk;
use kuravoxel_common::lod::{LodTile, TileKey};

use kuravoxel_common::block::Block;
use kuravoxel_common::physics::{Collider, Shape};
use kuravoxel_common::shape::BlockTable;
use kuravoxel_common::chunk::{CHUNK_BITS, CHUNK_SIZE_I};

use crate::storage::Storage;
use crate::worldgen::{TerrainBlocks, WorldGen};
use crate::Event;

/// Resultado de `World::fill`.
#[derive(Default)]
pub struct Filled {
    /// Chunks que han cambiado.
    pub changed: Vec<IVec3>,
    /// Chunks de la caja que no estaban cargados.
    pub missing: usize,
}

pub struct World {
    pub generator: Arc<WorldGen>,
    pool: rayon::ThreadPool,
    events: Sender<Event>,
    /// `None` mientras el chunk se está generando.
    chunks: FxHashMap<IVec3, Option<Arc<Chunk>>>,
    lod: FxHashMap<TileKey, Option<Arc<LodTile>>>,
    pub storage: Storage,
}

impl World {
    pub fn new(
        seed: u32,
        blocks: TerrainBlocks,
        storage: Storage,
        events: Sender<Event>,
    ) -> Self {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        // Se dejan libres el hilo del servidor y, en el servidor integrado, los del cliente.
        let workers = (cores / 2).max(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .thread_name(|i| format!("worldgen-{i}"))
            .build()
            .expect("failed to create the world generation thread pool");
        log::info!("world: seed {seed}, {workers} generation threads");
        Self {
            generator: Arc::new(WorldGen::new(seed, blocks)),
            pool,
            events,
            chunks: FxHashMap::default(),
            lod: FxHashMap::default(),
            storage,
        }
    }

    /// Devuelve el chunk si ya existe; si no, encarga su generación.
    pub fn chunk(&mut self, pos: IVec3) -> Option<Arc<Chunk>> {
        match self.chunks.entry(pos) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                // Un chunk modificado alguna vez se lee del guardado, no se genera.
                if let Some(saved) = self.storage.chunk(pos) {
                    entry.insert(Some(saved.clone()));
                    return Some(saved);
                }
                entry.insert(None);
                let generator = self.generator.clone();
                let events = self.events.clone();
                self.pool.spawn(move || {
                    let chunk = generator.generate(pos);
                    let _ = events.send(Event::Generated(pos, chunk));
                });
                None
            }
        }
    }

    /// Igual que `chunk`, para una tesela de terreno lejano.
    pub fn lod_tile(&mut self, key: TileKey) -> Option<Arc<LodTile>> {
        match self.lod.entry(key) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                entry.insert(None);
                let generator = self.generator.clone();
                let events = self.events.clone();
                self.pool.spawn(move || {
                    let tile = generator.lod_tile(key);
                    let _ = events.send(Event::LodGenerated(tile));
                });
                None
            }
        }
    }

    /// Chunk y coordenadas locales de un voxel del mundo.
    fn locate(pos: IVec3) -> (IVec3, [usize; 3]) {
        let chunk = IVec3::new(pos.x >> CHUNK_BITS, pos.y >> CHUNK_BITS, pos.z >> CHUNK_BITS);
        let local = pos - chunk * CHUNK_SIZE_I;
        (chunk, [local.x as usize, local.y as usize, local.z as usize])
    }

    /// Bloque en una posición, si su chunk está cargado.
    pub fn block_at(&self, pos: IVec3) -> Option<Block> {
        let (chunk_pos, [x, y, z]) = Self::locate(pos);
        let chunk = self.chunks.get(&chunk_pos)?.as_ref()?;
        Some(chunk.get(x, y, z))
    }

    /// Cambia un voxel. Devuelve el chunk afectado, o `None` si no está cargado.
    pub fn set_block(&mut self, pos: IVec3, block: Block) -> Option<IVec3> {
        let (chunk_pos, [x, y, z]) = Self::locate(pos);
        let slot = self.chunks.get_mut(&chunk_pos)?;
        let chunk = slot.as_ref()?;
        if chunk.get(x, y, z) != block {
            let changed = Arc::new(chunk.with_block(x, y, z, block));
            self.storage.put(chunk_pos, changed.clone());
            *slot = Some(changed);
        }
        Some(chunk_pos)
    }

    /// Chunk con autoridad, si está cargado.
    pub fn loaded_chunk(&self, pos: IVec3) -> Option<Arc<Chunk>> {
        self.chunks.get(&pos)?.clone()
    }

    /// Rellena la caja `min..=max` con un bloque, chunk a chunk (cada chunk se reconstruye
    /// una sola vez). Los chunks no cargados se saltan.
    pub fn fill(&mut self, min: IVec3, max: IVec3, block: Block) -> Filled {
        let mut filled = Filled::default();
        let (first, _) = Self::locate(min);
        let (last, _) = Self::locate(max);
        for cz in first.z..=last.z {
            for cy in first.y..=last.y {
                for cx in first.x..=last.x {
                    let chunk_pos = IVec3::new(cx, cy, cz);
                    let Some(Some(chunk)) = self.chunks.get(&chunk_pos) else {
                        filled.missing += 1;
                        continue;
                    };
                    let origin = chunk_pos * CHUNK_SIZE_I;
                    let lo = (min - origin).max(IVec3::ZERO);
                    let hi = (max - origin).min(IVec3::splat(CHUNK_SIZE_I - 1));
                    let mut data = chunk.to_dense();
                    let mut changed = false;
                    for z in lo.z..=hi.z {
                        for y in lo.y..=hi.y {
                            for x in lo.x..=hi.x {
                                let voxel = &mut data[Chunk::index(x as usize, y as usize, z as usize)];
                                changed |= *voxel != block;
                                *voxel = block;
                            }
                        }
                    }
                    if changed {
                        let chunk = Arc::new(Chunk::from_dense(data));
                        self.storage.put(chunk_pos, chunk.clone());
                        self.chunks.insert(chunk_pos, Some(chunk));
                        filled.changed.push(chunk_pos);
                    }
                }
            }
        }
        filled
    }

    pub fn on_generated(&mut self, pos: IVec3, chunk: Chunk) {
        // Si ya no está en el mapa es que nadie lo necesita: se descarta.
        if let Some(slot @ None) = self.chunks.get_mut(&pos) {
            *slot = Some(Arc::new(chunk));
        }
    }

    pub fn on_lod_generated(&mut self, tile: LodTile) {
        if let Some(slot @ None) = self.lod.get_mut(&tile.key) {
            *slot = Some(Arc::new(tile));
        }
    }

    /// Libera lo que ningún jugador necesita ya.
    pub fn retain(
        &mut self,
        mut chunk_needed: impl FnMut(IVec3) -> bool,
        mut lod_needed: impl FnMut(TileKey) -> bool,
        cache_center: Option<(i32, i32, i32)>,
    ) {
        self.chunks.retain(|&pos, _| chunk_needed(pos));
        self.lod.retain(|&key, _| lod_needed(key));
        match cache_center {
            Some((x, z, radius)) => self.generator.trim_cache(x, z, radius),
            // Con varios jugadores (o ninguno) la caché de columnas se vacía entera de
            // vez en cuando; se recalcula bajo demanda.
            None => self.generator.trim_cache(0, 0, -1),
        }
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
}
/// El mundo del servidor tal como lo ve la física (ítems sueltos). Lo que no está
/// cargado cuenta como sólido: un ítem no cae por un hueco que aún no existe.
pub struct WorldCollider<'a> {
    pub world: &'a World,
    pub blocks: &'a BlockTable,
}

impl Collider for WorldCollider<'_> {
    fn shape(&self, voxel: IVec3) -> Shape<'_> {
        match self.world.block_at(voxel) {
            None => Shape::Full,
            Some(block) => self.blocks.get(block).collision.shape(),
        }
    }
}
