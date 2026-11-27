//! Mundo del servidor: la única copia con autoridad de los chunks.

use std::collections::hash_map::Entry;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::IVec3;
use rustc_hash::FxHashMap;
use kuravoxel_common::chunk::Chunk;
use kuravoxel_common::lod::{LodTile, TileKey};

use kuravoxel_common::block::Block;
use kuravoxel_common::physics::{Collider, Shape};
use kuravoxel_common::shape::BlockTable;
use kuravoxel_common::chunk::{CHUNK_BITS, CHUNK_SIZE_I};

use crate::builds::{self, BuiltColumn, Builds};
use crate::storage::Storage;
use crate::biomes::Terrain;
use crate::worldgen::WorldGen;
use crate::Event;

/// Resultado de cambiar una caja de voxeles.
#[derive(Default)]
pub struct Edited {
    /// Chunks que han cambiado.
    pub changed: Vec<IVec3>,
    /// Chunks de la caja que no estaban cargados (y se han saltado).
    pub missing: usize,
    /// Cómo estaban los chunks cambiados: basta con devolverlos para deshacer.
    pub before: Vec<(IVec3, Arc<Chunk>)>,
}

/// Tras una edición, el terreno lejano de esa columna se rehace cuando lleva este tiempo
/// sin cambios, o como muy tarde pasado `BUILD_MAX_DELAY` desde la primera.
const BUILD_DELAY: Duration = Duration::from_millis(1200);
const BUILD_MAX_DELAY: Duration = Duration::from_secs(5);

/// Una tesela lejana: tal como sale del generador y con las construcciones encima (la
/// misma si no hay ninguna). La primera se guarda para no regenerarla tras cada edición.
struct LodEntry {
    base: Arc<LodTile>,
    shown: Arc<LodTile>,
}

pub struct World {
    pub generator: Arc<WorldGen>,
    /// Sube cada vez que cambia el generador: lo encargado al anterior se descarta.
    epoch: u32,
    pool: rayon::ThreadPool,
    events: Sender<Event>,
    /// `None` mientras el chunk se está generando.
    chunks: FxHashMap<IVec3, Option<Arc<Chunk>>>,
    lod: FxHashMap<TileKey, Option<LodEntry>>,
    pub storage: Storage,
    /// Dónde la superficie ya no es la del generador, por columna de chunks.
    builds: Builds,
    /// Columnas editadas pendientes de revisar: primera y última edición.
    builds_dirty: FxHashMap<(i32, i32), (Instant, Instant)>,
    /// Número de la última revisión encargada de cada columna: las anteriores se ignoran.
    builds_seq: FxHashMap<(i32, i32), u64>,
    next_seq: u64,
    /// Por número de bloque: es un cubo visible (cuenta como superficie a lo lejos).
    cubes: Arc<Vec<bool>>,
}

impl World {
    pub fn new(
        seed: u32,
        terrain: Arc<Terrain>,
        mut storage: Storage,
        events: Sender<Event>,
    ) -> Self {
        // Todo lo modificado en el mundo debe verse a lo lejos: se revisa al arrancar.
        let start = Instant::now();
        let long_ago = start.checked_sub(BUILD_MAX_DELAY).unwrap_or(start);
        let builds_dirty: FxHashMap<(i32, i32), (Instant, Instant)> =
            storage.load_all().into_iter().map(|column| (column, (long_ago, long_ago))).collect();
        if !builds_dirty.is_empty() {
            log::info!("world: {} modified chunk columns ({:.0?})", builds_dirty.len(), start.elapsed());
        }
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
            generator: Arc::new(WorldGen::new(seed, terrain)),
            epoch: 0,
            pool,
            events,
            chunks: FxHashMap::default(),
            lod: FxHashMap::default(),
            storage,
            builds: Builds::default(),
            builds_dirty,
            builds_seq: FxHashMap::default(),
            next_seq: 0,
            cubes: Arc::new(Vec::new()),
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
                let epoch = self.epoch;
                self.pool.spawn(move || {
                    let chunk = generator.generate(pos);
                    let _ = events.send(Event::Generated(epoch, pos, chunk));
                });
                None
            }
        }
    }

    /// Igual que `chunk`, para una tesela de terreno lejano.
    pub fn lod_tile(&mut self, key: TileKey) -> Option<Arc<LodTile>> {
        match self.lod.entry(key) {
            Entry::Occupied(entry) => entry.get().as_ref().map(|tile| tile.shown.clone()),
            Entry::Vacant(entry) => {
                entry.insert(None);
                let generator = self.generator.clone();
                let events = self.events.clone();
                let epoch = self.epoch;
                self.pool.spawn(move || {
                    let tile = generator.lod_tile(key);
                    let _ = events.send(Event::LodGenerated(epoch, tile));
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
            self.mark_built(chunk_pos);
        }
        Some(chunk_pos)
    }

    /// Chunk con autoridad, si está cargado.
    pub fn loaded_chunk(&self, pos: IVec3) -> Option<Arc<Chunk>> {
        self.chunks.get(&pos)?.clone()
    }

    /// Rellena la caja `min..=max` con un bloque. Los chunks no cargados se saltan.
    pub fn fill(&mut self, min: IVec3, max: IVec3, block: Block) -> Edited {
        self.edit_region(min, max, |_, _| block)
    }

    /// Todos los chunks que toca la caja `min..=max` están cargados.
    pub fn region_loaded(&self, min: IVec3, max: IVec3) -> bool {
        let (first, _) = Self::locate(min);
        let (last, _) = Self::locate(max);
        (first.z..=last.z).all(|cz| {
            (first.y..=last.y).all(|cy| {
                (first.x..=last.x).all(|cx| matches!(self.chunks.get(&IVec3::new(cx, cy, cz)), Some(Some(_))))
            })
        })
    }

    /// Cambia los voxeles de la caja `min..=max`: `edit` recibe la posición y el bloque
    /// que hay, y devuelve el que debe quedar. Va chunk a chunk (cada uno se reconstruye
    /// una sola vez); los chunks no cargados se saltan.
    pub fn edit_region(&mut self, min: IVec3, max: IVec3, mut edit: impl FnMut(IVec3, Block) -> Block) -> Edited {
        let mut edited = Edited::default();
        let (first, _) = Self::locate(min);
        let (last, _) = Self::locate(max);
        for cz in first.z..=last.z {
            for cy in first.y..=last.y {
                for cx in first.x..=last.x {
                    let chunk_pos = IVec3::new(cx, cy, cz);
                    let Some(Some(chunk)) = self.chunks.get(&chunk_pos) else {
                        edited.missing += 1;
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
                                let block = edit(origin + IVec3::new(x, y, z), *voxel);
                                changed |= *voxel != block;
                                *voxel = block;
                            }
                        }
                    }
                    if changed {
                        edited.before.push((chunk_pos, chunk.clone()));
                        let chunk = Arc::new(Chunk::from_dense(data));
                        self.storage.put(chunk_pos, chunk.clone());
                        self.chunks.insert(chunk_pos, Some(chunk));
                        edited.changed.push(chunk_pos);
                        self.mark_built(chunk_pos);
                    }
                }
            }
        }
        edited
    }

    /// Devuelve unos chunks a un estado anterior (deshacer). Los que ya no están en
    /// memoria se corrigen igualmente en el guardado. Devuelve los que hay que reenviar.
    pub fn restore(&mut self, chunks: Vec<(IVec3, Arc<Chunk>)>) -> Vec<IVec3> {
        let mut loaded = Vec::new();
        for (pos, chunk) in chunks {
            self.storage.put(pos, chunk.clone());
            self.mark_built(pos);
            if let Some(slot @ Some(_)) = self.chunks.get_mut(&pos) {
                *slot = Some(chunk);
                loaded.push(pos);
            }
        }
        loaded
    }

    /// Cambia el generador: todo lo que no esté editado se olvida y se volverá a generar
    /// cuando alguien lo pida. Lo que ya estaba encargado al anterior se descarta al llegar.
    pub fn set_generator(&mut self, generator: WorldGen) {
        self.generator = Arc::new(generator);
        self.epoch += 1;
        let storage = &mut self.storage;
        self.chunks.retain(|&pos, slot| slot.is_some() && storage.chunk(pos).is_some());
        self.lod.clear();
        // Lo construido se compara con el terreno natural, que ha cambiado: se revisa todo.
        self.builds.clear();
        self.builds_seq.clear();
        let now = Instant::now();
        let long_ago = now.checked_sub(BUILD_MAX_DELAY).unwrap_or(now);
        for column in self.storage.load_all() {
            self.builds_dirty.insert(column, (long_ago, long_ago));
        }
    }

    pub fn on_generated(&mut self, epoch: u32, pos: IVec3, chunk: Chunk) {
        if epoch != self.epoch {
            return;
        }
        // Si ya no está en el mapa es que nadie lo necesita: se descarta.
        if let Some(slot @ None) = self.chunks.get_mut(&pos) {
            *slot = Some(Arc::new(chunk));
        }
    }

    pub fn on_lod_generated(&mut self, epoch: u32, tile: LodTile) {
        if epoch != self.epoch {
            return;
        }
        if let Some(slot @ None) = self.lod.get_mut(&tile.key) {
            let base = Arc::new(tile);
            let shown = builds::apply(&base, &self.builds).map_or_else(|| base.clone(), Arc::new);
            *slot = Some(LodEntry { base, shown });
        }
    }

    /// Qué bloques cuentan como superficie a lo lejos: los cubos visibles.
    pub fn set_block_table(&mut self, blocks: &BlockTable) {
        let cubes = (0..blocks.len())
            .map(|i| {
                let props = blocks.get(Block(i as u16));
                props.visible && props.model.is_none()
            })
            .collect();
        self.cubes = Arc::new(cubes);
    }

    /// Anota que un chunk ha cambiado: su columna se revisará para el terreno lejano.
    fn mark_built(&mut self, chunk: IVec3) {
        let now = Instant::now();
        self.builds_dirty.entry((chunk.x, chunk.z)).or_insert((now, now)).1 = now;
    }

    /// Encarga la revisión de las columnas editadas que ya llevan un rato quietas. El
    /// resultado llega como `Event::BuildsScanned`.
    pub fn update_builds(&mut self) {
        if self.builds_dirty.is_empty() {
            return;
        }
        let now = Instant::now();
        let due: Vec<(i32, i32)> = self
            .builds_dirty
            .iter()
            .filter(|(_, times)| now - times.1 >= BUILD_DELAY || now - times.0 >= BUILD_MAX_DELAY)
            .map(|(&column, _)| column)
            .collect();
        for column in due {
            self.builds_dirty.remove(&column);
            self.next_seq += 1;
            let seq = self.next_seq;
            self.builds_seq.insert(column, seq);
            let layers = self.storage.column_chunks(column);
            let generator = self.generator.clone();
            let cubes = self.cubes.clone();
            let events = self.events.clone();
            self.pool.spawn(move || {
                let built = builds::scan_column(&generator, &cubes, column, &layers);
                let _ = events.send(Event::BuildsScanned { column, seq, built });
            });
        }
    }

    /// Llega la revisión de una columna. Devuelve las teselas cargadas que cambian con
    /// ella, ya rehechas: hay que reenviarlas a quien las tenga.
    pub fn on_builds_scanned(&mut self, column: (i32, i32), seq: u64, built: Vec<BuiltColumn>) -> Vec<Arc<LodTile>> {
        if self.builds_seq.get(&column) != Some(&seq) {
            return Vec::new();
        }
        self.builds_seq.remove(&column);
        if self.builds.get(&column).map_or(built.is_empty(), |old| *old == built) {
            return Vec::new();
        }
        if built.is_empty() {
            self.builds.remove(&column);
        } else {
            self.builds.insert(column, built);
        }
        let mut resend = Vec::new();
        for key in builds::tiles_touching(column) {
            if let Some(Some(entry)) = self.lod.get_mut(&key) {
                entry.shown = builds::apply(&entry.base, &self.builds).map_or_else(|| entry.base.clone(), Arc::new);
                resend.push(entry.shown.clone());
            }
        }
        resend
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
