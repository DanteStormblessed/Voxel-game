//! Mundo del cliente: una copia de lo que el servidor ha decidido enviarle.
//!
//! El cliente no genera nada. Recibe chunks y teselas de terreno lejano, los malla en un
//! pool de hilos y entrega las mallas al renderer con un presupuesto por frame.

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use glam::{DVec3, IVec3, Vec3};
use rustc_hash::FxHashMap;
use kuravoxel_common::block::Block;
use kuravoxel_common::chunk::{Chunk, CHUNK_BITS, CHUNK_SIZE_I};
use kuravoxel_common::lod::{self, LodTile, TileKey, MAX_LEVEL, MIN_LEVEL, TILE_CELLS};
use kuravoxel_common::physics::{Collider, Shape};
use kuravoxel_common::shape::{BlockProps, BlockTable};

use crate::mesher::{self, ChunkMesh, MeshTable, Neighborhood};

/// Mallas nuevas que se entregan al renderer como mucho por frame.
const MESH_UPLOADS_PER_FRAME: usize = 24;

/// Identifica una malla en el renderer: un chunk real (nivel 0) o una pieza de terreno
/// lejano (nivel 1, 2...). `pos` va en unidades de chunk de su nivel (32 · 2^nivel metros).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MeshKey {
    pub level: u8,
    pub pos: IVec3,
}

impl MeshKey {
    /// Tamaño de sus celdas en voxeles.
    pub fn scale(self) -> i32 {
        1 << self.level
    }

    /// Esquina mínima, en chunks de 32 voxeles.
    pub fn origin(self) -> IVec3 {
        self.pos * self.scale()
    }
}

pub enum WorldEvent {
    /// Hay una malla nueva.
    Mesh { key: MeshKey, mesh: ChunkMesh },
    /// La malla ya no hace falta.
    Unload { key: MeshKey },
}

pub struct WorldStats {
    pub chunks: usize,
    pub meshing: usize,
    pub lod_tiles: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MeshState {
    /// Aún no se ha pedido la malla.
    None,
    /// Hay un trabajo de mallado en curso.
    Pending,
    /// Mallado (o resuelto como vacío).
    Done,
}

struct Slot {
    chunk: Arc<Chunk>,
    /// Cuántos de sus 26 vecinos se han recibido ya. Con 26 se puede mallar.
    ready_neighbors: u8,
    mesh: MeshState,
    /// Sube con cada edición: una malla calculada para una versión anterior se descarta.
    version: u32,
    /// Tiene una malla subida al renderer.
    has_mesh: bool,
}

struct Tile {
    data: Arc<LodTile>,
    ready: bool,
    /// Alturas (en chunks de su nivel) de las mallas que tiene subidas.
    chunks: Vec<i32>,
    /// El servidor ha pedido soltarla; se hace cuando sus reemplazos estén mallados.
    retiring: bool,
}

type TileMeshes = Vec<(i32, ChunkMesh)>;

pub struct World {
    pool: rayon::ThreadPool,
    mesh_tx: Sender<(IVec3, u32, ChunkMesh)>,
    mesh_rx: Receiver<(IVec3, u32, ChunkMesh)>,
    lod_tx: Sender<(TileKey, TileMeshes)>,
    lod_rx: Receiver<(TileKey, TileMeshes)>,

    slots: FxHashMap<IVec3, Slot>,
    /// Solo se mallan los chunks a esta distancia horizontal (en chunks) o menos; lo que
    /// queda más lejos lo dibuja el terreno lejano.
    mesh_radius: f32,
    center: Option<IVec3>,
    neighbors: [IVec3; 26],
    /// Chunks que acaban de completar su vecindad y esperan turno de mallado.
    mesh_queue: VecDeque<IVec3>,
    mesh_in_flight: usize,
    max_mesh_in_flight: usize,

    tiles: FxHashMap<TileKey, Tile>,
    lod_in_flight: usize,
    lod_dirty: bool,

    /// Propiedades de cada número de bloque (física, rayo) y su versión para el mallador.
    /// Llegan con el contenido; hasta entonces todo lo que no es aire es un cubo sólido.
    blocks: Arc<BlockTable>,
    mesh_table: Arc<MeshTable>,
}

impl World {
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        // El resto de núcleos son para el hilo principal y el servidor integrado.
        let workers = (cores / 2).saturating_sub(1).max(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .thread_name(|i| format!("mesher-{i}"))
            .build()
            .expect("failed to create the meshing thread pool");

        let mut neighbors = [IVec3::ZERO; 26];
        let mut n = 0;
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if (dx, dy, dz) != (0, 0, 0) {
                        neighbors[n] = IVec3::new(dx, dy, dz);
                        n += 1;
                    }
                }
            }
        }

        let (mesh_tx, mesh_rx) = channel();
        let (lod_tx, lod_rx) = channel();
        Self {
            pool,
            mesh_tx,
            mesh_rx,
            lod_tx,
            lod_rx,
            slots: FxHashMap::default(),
            mesh_radius: 0.0,
            center: None,
            neighbors,
            mesh_queue: VecDeque::new(),
            mesh_in_flight: 0,
            max_mesh_in_flight: workers * 4,
            tiles: FxHashMap::default(),
            lod_in_flight: 0,
            lod_dirty: false,
            blocks: Arc::default(),
            mesh_table: Arc::default(),
        }
    }

    /// Propiedades de los bloques del contenido actual.
    pub fn blocks(&self) -> &BlockTable {
        &self.blocks
    }

    /// Nuevas tablas de bloques (al recibir contenido). Si cambian formas o
    /// transparencias, todos los chunks se vuelven a mallar; si solo cambian colores,
    /// basta con la paleta.
    pub fn set_tables(&mut self, blocks: Arc<BlockTable>, mesh_table: Arc<MeshTable>) {
        let remesh = !self.mesh_table.same_geometry(&mesh_table);
        self.blocks = blocks;
        self.mesh_table = mesh_table;
        if remesh {
            for (&pos, slot) in self.slots.iter_mut() {
                slot.version += 1;
                slot.mesh = MeshState::None;
                if slot.ready_neighbors == 26 {
                    self.mesh_queue.push_back(pos);
                }
            }
        }
    }

    /// Distancia de voxeles reales concedida por el servidor, en chunks.
    pub fn set_view_distance(&mut self, chunks: i32) {
        self.mesh_radius = chunks as f32 + 2.5;
        // Fuerza a reconsiderar qué chunks entran en el radio.
        self.center = None;
    }

    pub fn stats(&self) -> WorldStats {
        WorldStats {
            chunks: self.slots.len(),
            meshing: self.mesh_in_flight + self.lod_in_flight,
            lod_tiles: self.tiles.len(),
        }
    }

    /// `true` si no queda nada por mallar de lo recibido hasta ahora.
    pub fn is_idle(&self) -> bool {
        self.mesh_in_flight == 0 && self.lod_in_flight == 0 && self.mesh_queue.is_empty()
    }

    /// Cuántos chunks de la capa horizontal de la cámara, dentro de `radius` chunks, aún
    /// no están listos para dibujarse (sin recibir, o recibidos y sin mallar).
    ///
    /// Es la medida de "se ve un hueco cerca": esa capa siempre le toca al cliente, así
    /// que si falta algo es que el servidor o el mallado no dan abasto.
    pub fn missing_near(&self, center: IVec3, radius: i32) -> usize {
        let mut missing = 0;
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dz * dz > radius * radius {
                    continue;
                }
                let ready = self
                    .slots
                    .get(&(center + IVec3::new(dx, 0, dz)))
                    // Un chunk que se está remallando tras una edición sigue mostrando
                    // su malla anterior: no es un hueco.
                    .is_some_and(|slot| slot.mesh == MeshState::Done || slot.has_mesh);
                missing += !ready as usize;
            }
        }
        missing
    }

    /// Altura aproximada del suelo en una columna, según el terreno lejano recibido.
    pub fn ground_height(&self, wx: i32, wz: i32) -> Option<i32> {
        (MIN_LEVEL..=MAX_LEVEL).find_map(|level| {
            let cell = 1i32 << level;
            let span = CHUNK_SIZE_I * cell;
            let tile = self.tiles.get(&(level, wx.div_euclid(span), wz.div_euclid(span)))?;
            let (cx, cz) = (wx.rem_euclid(span) / cell + 1, wz.rem_euclid(span) / cell + 1);
            let top = tile.data.top_at(cx as usize + cz as usize * TILE_CELLS)?;
            Some((top + 1) * cell)
        })
    }

    // --- Mensajes del servidor -------------------------------------------------------

    pub fn insert_chunk(&mut self, pos: IVec3, chunk: Arc<Chunk>) {
        // Un chunk que ya se tenía llega de nuevo cuando el servidor lo cambia en bloque
        // (`fill`): se sustituye y se vuelve a mallar con sus vecinos.
        if let Some(slot) = self.slots.get_mut(&pos) {
            slot.chunk = chunk;
            for offset in self.neighbors.into_iter().chain([IVec3::ZERO]) {
                if let Some(slot) = self.slots.get_mut(&(pos + offset)) {
                    slot.version += 1;
                    slot.mesh = MeshState::None;
                    if slot.ready_neighbors == 26 {
                        self.mesh_queue.push_front(pos + offset);
                    }
                }
            }
            return;
        }
        let mut ready = 0u8;
        for offset in self.neighbors {
            let neighbor_pos = pos + offset;
            if let Some(neighbor) = self.slots.get_mut(&neighbor_pos) {
                ready += 1;
                neighbor.ready_neighbors += 1;
                if neighbor.ready_neighbors == 26 && neighbor.mesh == MeshState::None {
                    self.mesh_queue.push_back(neighbor_pos);
                }
            }
        }
        self.slots.insert(
            pos,
            Slot { chunk, ready_neighbors: ready, mesh: MeshState::None, version: 0, has_mesh: false },
        );
        if ready == 26 {
            self.mesh_queue.push_back(pos);
        }
    }

    pub fn unload_chunks(&mut self, positions: &[[i32; 3]], events: &mut Vec<WorldEvent>) {
        for &pos in positions {
            let pos = IVec3::from_array(pos);
            let Some(slot) = self.slots.remove(&pos) else { continue };
            for offset in self.neighbors {
                if let Some(neighbor) = self.slots.get_mut(&(pos + offset)) {
                    neighbor.ready_neighbors -= 1;
                }
            }
            if slot.has_mesh {
                events.push(WorldEvent::Unload { key: MeshKey { level: 0, pos } });
            }
        }
    }

    /// Chunk y coordenadas locales de un voxel del mundo.
    fn locate(pos: IVec3) -> (IVec3, IVec3) {
        let chunk = IVec3::new(pos.x >> CHUNK_BITS, pos.y >> CHUNK_BITS, pos.z >> CHUNK_BITS);
        (chunk, pos - chunk * CHUNK_SIZE_I)
    }

    /// Bloque en una posición del mundo, si su chunk se ha recibido.
    pub fn block_at(&self, pos: IVec3) -> Option<Block> {
        let (chunk, local) = Self::locate(pos);
        let slot = self.slots.get(&chunk)?;
        Some(slot.chunk.get(local.x as usize, local.y as usize, local.z as usize))
    }

    /// Cambia un voxel y vuelve a mallar lo afectado, con prioridad. Lo usan tanto la
    /// predicción local (al hacer clic) como los cambios que confirma el servidor.
    pub fn set_block(&mut self, pos: IVec3, block: Block) {
        let (chunk_pos, local) = Self::locate(pos);
        let Some(slot) = self.slots.get_mut(&chunk_pos) else { return };
        let (x, y, z) = (local.x as usize, local.y as usize, local.z as usize);
        if slot.chunk.get(x, y, z) == block {
            return;
        }
        slot.chunk = Arc::new(slot.chunk.with_block(x, y, z, block));

        // Un voxel en el borde también cambia caras, aristas y oclusión de los chunks
        // vecinos (hasta 7 más si está en una esquina).
        let reach = |c: i32| match c {
            0 => -1..=0,
            c if c == CHUNK_SIZE_I - 1 => 0..=1,
            _ => 0..=0,
        };
        for dz in reach(local.z) {
            for dy in reach(local.y) {
                for dx in reach(local.x) {
                    let affected = chunk_pos + IVec3::new(dx, dy, dz);
                    if let Some(slot) = self.slots.get_mut(&affected) {
                        slot.version += 1;
                        slot.mesh = MeshState::None;
                        self.mesh_queue.push_front(affected);
                    }
                }
            }
        }
    }

    /// Primer bloque que se puede editar en la dirección de la mirada (atraviesa el agua,
    /// no las flores). Devuelve su posición y la del hueco inmediatamente anterior (donde
    /// se colocaría un bloque nuevo).
    pub fn raycast(&self, origin: DVec3, direction: Vec3, reach: f64) -> Option<(IVec3, IVec3)> {
        self.raycast_hit(origin, direction, reach, |props| props.targetable)
            .map(|(voxel, previous, _)| (voxel, previous))
    }

    /// Distancia desde `origin` hasta el primer bloque opaco en esa dirección, si lo hay
    /// antes de `reach`. Sirve para que la cámara en tercera persona no atraviese paredes.
    pub fn raycast_distance(&self, origin: DVec3, direction: Vec3, reach: f64) -> Option<f64> {
        self.raycast_hit(origin, direction, reach, |props| props.opaque)
            .map(|(_, _, distance)| distance)
    }

    /// El voxel tapa la vista (cubo opaco o aún sin recibir).
    pub fn is_opaque(&self, voxel: IVec3) -> bool {
        self.block_at(voxel).is_none_or(|block| self.blocks.get(block).opaque)
    }

    /// Como `raycast`, con la distancia recorrida hasta entrar en el bloque. Si el origen
    /// ya está dentro de un bloque sólido no hay resultado.
    fn raycast_hit(
        &self,
        origin: DVec3,
        direction: Vec3,
        reach: f64,
        stops: impl Fn(&BlockProps) -> bool,
    ) -> Option<(IVec3, IVec3, f64)> {
        let direction = direction.as_dvec3().normalize_or_zero();
        if direction == DVec3::ZERO {
            return None;
        }
        let mut voxel = origin.floor().as_ivec3();
        let step = direction.signum().as_ivec3();
        // Distancia a lo largo del rayo hasta el siguiente plano de cada eje, y entre planos.
        let delta = (DVec3::ONE / direction).abs();
        let next_plane = voxel.as_dvec3() + step.max(IVec3::ZERO).as_dvec3();
        let mut t_max = ((next_plane - origin) / direction).abs();
        for axis in 0..3 {
            if direction[axis] == 0.0 {
                t_max[axis] = f64::INFINITY;
            }
        }

        let mut previous = voxel;
        let mut travelled = 0.0;
        loop {
            if self.block_at(voxel).is_some_and(|block| stops(self.blocks.get(block))) {
                return (voxel != previous).then_some((voxel, previous, travelled));
            }
            let axis = if t_max.x <= t_max.y && t_max.x <= t_max.z {
                0
            } else if t_max.y <= t_max.z {
                1
            } else {
                2
            };
            if t_max[axis] > reach {
                return None;
            }
            previous = voxel;
            travelled = t_max[axis];
            voxel[axis] += step[axis];
            t_max[axis] += delta[axis];
        }
    }

    pub fn insert_lod(&mut self, data: Arc<LodTile>, events: &mut Vec<WorldEvent>) {
        let key = data.key;
        if !lod::is_valid_level(key.0) {
            return;
        }
        // Si ya existía (el servidor la reenvía), la versión nueva la sustituye.
        self.remove_tile(key, events);
        self.tiles.insert(
            key,
            Tile { data: data.clone(), ready: false, chunks: Vec::new(), retiring: false },
        );
        self.lod_in_flight += 1;
        let tx = self.lod_tx.clone();
        self.pool.spawn(move || {
            let meshes = mesher::mesh_lod_tile(&data);
            let _ = tx.send((key, meshes));
        });
    }

    pub fn unload_lod(&mut self, keys: &[TileKey]) {
        for key in keys {
            if let Some(tile) = self.tiles.get_mut(key) {
                tile.retiring = true;
                self.lod_dirty = true;
            }
        }
    }

    // --- Avance por frame ------------------------------------------------------------

    /// `center` es el chunk en el que está la cámara.
    pub fn update(&mut self, center: IVec3, events: &mut Vec<WorldEvent>) {
        if self.center != Some(center) {
            self.center = Some(center);
            // Chunks que estaban fuera del radio de mallado y ahora han entrado.
            for (&pos, slot) in &self.slots {
                if slot.mesh == MeshState::None
                    && slot.ready_neighbors == 26
                    && self.in_mesh_radius(pos, center)
                {
                    self.mesh_queue.push_back(pos);
                }
            }
        }
        self.schedule_meshing(center, events);
        self.receive_meshes(events);
        self.update_lod(events);
    }

    fn in_mesh_radius(&self, pos: IVec3, center: IVec3) -> bool {
        let (dx, dz) = ((pos.x - center.x) as f32, (pos.z - center.z) as f32);
        dx * dx + dz * dz <= self.mesh_radius * self.mesh_radius
    }

    fn schedule_meshing(&mut self, center: IVec3, events: &mut Vec<WorldEvent>) {
        while self.mesh_in_flight < self.max_mesh_in_flight {
            let Some(pos) = self.mesh_queue.pop_front() else { break };
            let Some(slot) = self.slots.get(&pos) else { continue };
            if slot.mesh != MeshState::None || slot.ready_neighbors != 26 {
                continue;
            }
            // Fuera del radio se queda sin malla; `update` lo vuelve a encolar si la
            // cámara se acerca.
            if !self.in_mesh_radius(pos, center) {
                continue;
            }

            // El caso más común con diferencia: cielo. Ni siquiera se reúne la vecindad.
            if slot.chunk.uniform() == Some(Block::AIR) {
                self.finish_empty(pos, events);
                continue;
            }
            let version = slot.version;

            let mut chunks = Vec::with_capacity(27);
            for dz in -1..=1 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if let Some(slot) = self.slots.get(&(pos + IVec3::new(dx, dy, dz))) {
                            chunks.push(slot.chunk.clone());
                        }
                    }
                }
            }
            let Ok(hood) = Neighborhood::try_from(chunks) else { continue };

            if mesher::is_trivially_empty(&hood, &self.mesh_table) {
                self.finish_empty(pos, events);
                continue;
            }

            if let Some(slot) = self.slots.get_mut(&pos) {
                slot.mesh = MeshState::Pending;
            }
            self.mesh_in_flight += 1;
            let tx = self.mesh_tx.clone();
            let table = self.mesh_table.clone();
            self.pool.spawn(move || {
                let mesh = mesher::mesh_chunk(&hood, &table);
                let _ = tx.send((pos, version, mesh));
            });
        }
    }

    /// El chunk no tiene geometría: se da por mallado y se retira la malla que tuviera.
    fn finish_empty(&mut self, pos: IVec3, events: &mut Vec<WorldEvent>) {
        if let Some(slot) = self.slots.get_mut(&pos) {
            slot.mesh = MeshState::Done;
            if std::mem::take(&mut slot.has_mesh) {
                events.push(WorldEvent::Unload { key: MeshKey { level: 0, pos } });
            }
        }
    }

    fn receive_meshes(&mut self, events: &mut Vec<WorldEvent>) {
        let mut uploads = 0;
        while uploads < MESH_UPLOADS_PER_FRAME {
            let Ok((pos, version, mesh)) = self.mesh_rx.try_recv() else { break };
            self.mesh_in_flight -= 1;
            // Se ignora si el chunk se descargó o se editó mientras se mallaba.
            let Some(slot) = self.slots.get_mut(&pos) else { continue };
            if slot.mesh != MeshState::Pending || slot.version != version {
                continue;
            }
            slot.mesh = MeshState::Done;
            let key = MeshKey { level: 0, pos };
            if !mesh.quads.is_empty() {
                slot.has_mesh = true;
                events.push(WorldEvent::Mesh { key, mesh });
                uploads += 1;
            } else if std::mem::take(&mut slot.has_mesh) {
                events.push(WorldEvent::Unload { key });
            }
        }
    }

    fn update_lod(&mut self, events: &mut Vec<WorldEvent>) {
        while let Ok((key, meshes)) = self.lod_rx.try_recv() {
            self.lod_in_flight -= 1;
            // Se ignora si la tesela se soltó o se sustituyó mientras se mallaba.
            let Some(tile) = self.tiles.get_mut(&key) else { continue };
            if tile.ready {
                continue;
            }
            tile.ready = true;
            for (y, mesh) in meshes {
                tile.chunks.push(y);
                events.push(WorldEvent::Mesh {
                    key: MeshKey { level: key.0, pos: IVec3::new(key.1, y, key.2) },
                    mesh,
                });
            }
            self.lod_dirty = true;
        }

        if !self.lod_dirty {
            return;
        }
        self.lod_dirty = false;
        // Una tesela en retirada desaparece cuando las que la sustituyen (las de otros
        // niveles que cubren su terreno) están ya malladas; así nunca queda un hueco al
        // cambiar de nivel.
        let obsolete: Vec<TileKey> = self
            .tiles
            .iter()
            .filter(|(_, tile)| tile.retiring)
            .map(|(key, _)| *key)
            .filter(|&key| {
                self.tiles.iter().all(|(&other, relative)| {
                    other == key
                        || !lod::overlaps(other, key)
                        || relative.ready
                        || relative.retiring
                })
            })
            .collect();
        for key in obsolete {
            self.remove_tile(key, events);
        }
    }

    fn remove_tile(&mut self, key: TileKey, events: &mut Vec<WorldEvent>) {
        if let Some(tile) = self.tiles.remove(&key) {
            for y in tile.chunks {
                events.push(WorldEvent::Unload {
                    key: MeshKey { level: key.0, pos: IVec3::new(key.1, y, key.2) },
                });
            }
        }
    }
}

/// La física choca contra la copia local del mundo. Lo que aún no se ha recibido cuenta
/// como sólido, para no caer por un borde que todavía no ha llegado.
impl Collider for World {
    fn shape(&self, voxel: IVec3) -> Shape<'_> {
        match self.block_at(voxel) {
            None => Shape::Full,
            Some(block) => self.blocks.get(block).collision.shape(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mundo de 3×3×3 chunks con el suelo macizo hasta y = 0 en el chunk central inferior.
    fn flat_world() -> World {
        let mut world = World::new();
        world.set_view_distance(4);
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    let block = if y < 0 { Block(1) } else { Block::AIR };
                    world.insert_chunk(IVec3::new(x, y, z), Arc::new(Chunk::Uniform(block)));
                }
            }
        }
        world
    }

    #[test]
    fn raycast_finds_the_block_and_the_gap_before_it() {
        let world = flat_world();
        let eye = DVec3::new(3.5, 2.5, 3.5);
        let hit = world.raycast(eye, Vec3::NEG_Y, 8.0);
        assert_eq!(hit, Some((IVec3::new(3, -1, 3), IVec3::new(3, 0, 3))));
        // Demasiado lejos, o mirando al cielo: nada.
        assert_eq!(world.raycast(eye, Vec3::NEG_Y, 2.0), None);
        assert_eq!(world.raycast(eye, Vec3::Y, 8.0), None);
        assert_eq!(world.raycast_distance(eye, Vec3::NEG_Y, 8.0), Some(2.5));
        // En diagonal entra por la cara superior del suelo.
        let (block, before) = world.raycast(eye, Vec3::new(1.0, -1.0, 0.0), 8.0).unwrap();
        assert_eq!((block.y, before.y), (-1, 0));
    }

    #[test]
    fn missing_near_counts_chunks_not_ready_to_draw() {
        let mut world = flat_world();
        let mut events = Vec::new();
        // Nada mallado todavía: falta el propio chunk de la cámara.
        assert_eq!(world.missing_near(IVec3::ZERO, 0), 1);
        world.update(IVec3::ZERO, &mut events);
        // El central (aire, con sus 26 vecinos) queda listo; los 8 de alrededor no pueden
        // mallarse porque les faltan vecinos, y más lejos no hay nada recibido.
        assert_eq!(world.missing_near(IVec3::ZERO, 0), 0);
        assert_eq!(world.missing_near(IVec3::ZERO, 1), 4);
        assert_eq!(world.missing_near(IVec3::new(5, 0, 0), 0), 1);
    }

    #[test]
    fn editing_remeshes_the_chunk_and_its_neighbors() {
        let mut world = flat_world();
        let mut events = Vec::new();
        // Un bloque en la esquina del chunk central toca también a 7 vecinos.
        world.set_block(IVec3::new(0, 0, 0), Block(2));
        assert_eq!(world.block_at(IVec3::new(0, 0, 0)), Some(Block(2)));
        // 8 chunks afectados, más el central que ya esperaba su primer mallado.
        assert_eq!(world.mesh_queue.len(), 9);
        assert_eq!(world.slots[&IVec3::ZERO].version, 1);

        // Solo el chunk central tiene sus 26 vecinos: es el único que llega a mallarse.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !events.iter().any(|e| matches!(e, WorldEvent::Mesh { key, .. } if key.pos == IVec3::ZERO)) {
            assert!(std::time::Instant::now() < deadline, "mesh never arrived");
            world.update(IVec3::ZERO, &mut events);
            std::thread::yield_now();
        }

        // Al romperlo el chunk vuelve a quedar vacío y su malla se retira.
        events.clear();
        world.set_block(IVec3::new(0, 0, 0), Block::AIR);
        while !events.iter().any(|e| matches!(e, WorldEvent::Unload { key } if key.pos == IVec3::ZERO)) {
            assert!(std::time::Instant::now() < deadline, "mesh was never removed");
            world.update(IVec3::ZERO, &mut events);
            std::thread::yield_now();
        }
    }
}
