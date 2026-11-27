//! Servidor autoritativo.
//!
//! El mismo servidor sirve para jugar solo (integrado: corre en un hilo del propio juego
//! y el cliente le habla por canales en memoria) y para multijugador (dedicado o abierto
//! a la red: los clientes llegan por QUIC). La lógica no distingue un caso del otro: cada
//! conexión es un `Sink` por el que salen mensajes y eventos `Event::Message` que entran.
//!
//! Bucle: los eventos (conexiones, mensajes, chunks recién generados) se atienden en
//! cuanto llegan; la simulación avanza a ticks fijos de `TICK_RATE` por segundo.

pub mod biomes;
mod builds;
mod commands;
mod drops;
pub mod graph;
mod noise;
pub mod preview;
mod prefabs;
pub mod quic;
mod storage;
mod world;
pub mod worldgen;
pub mod worldgen_doc;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;

use glam::{DVec3, IVec3, Vec3};
use rustc_hash::{FxHashMap, FxHashSet};
use kuravoxel_common::chunk::{Chunk, CHUNK_SIZE};
use kuravoxel_common::physics::EYE_HEIGHT;
use kuravoxel_common::block::Block;
use kuravoxel_common::content::{self, ContentData, ItemId};
use kuravoxel_common::shape::BlockTable;
use kuravoxel_common::lod::{self, LodTile, TileKey};
use kuravoxel_common::prefab::Prefab;
use kuravoxel_common::protocol::{
    ClientMsg, DropState, PlayerId, PlayerState, ServerMsg, TerrainOp, TerrainStatus, WorldTime, LOAD_MARGIN, PROTOCOL_VERSION, REACH,
    TICK_RATE, UNLOAD_MARGIN, VERTICAL_RADIUS,
};

use commands::Command;
use storage::{Storage, WorldMeta};
use drops::Drops;
use world::{World, WorldCollider};
use biomes::{Terrain, WorldSource};
use worldgen::WorldGen;

/// Chunks que un jugador puede tener esperando generación a la vez. Acota el trabajo
/// encargado por adelantado: si el jugador se mueve, lo cercano no queda en cola detrás.
const MAX_PENDING_CHUNKS: usize = 256;
/// Posiciones de la lista de carga revisadas como mucho por jugador en cada pasada.
const SCAN_BUDGET: usize = 8192;
const GC_INTERVAL: Duration = Duration::from_secs(3);
/// Cada cuánto se escriben en disco los cambios del mundo.
const SAVE_INTERVAL: Duration = Duration::from_secs(5);
/// Holgura al validar el alcance de una edición: la posición que conoce el servidor va
/// hasta un tick por detrás de la del cliente.
const REACH_SLACK: f64 = 4.0;
/// Cada cuánto se mira si los archivos de contenido han cambiado.
const CONTENT_POLL: Duration = Duration::from_millis(500);
/// Cada cuántos ticks se recuerda la hora a los clientes (entre medias la avanzan solos).
const TIME_SYNC_TICKS: u32 = 10 * TICK_RATE;
/// Cada cuánto se guarda la hora del mundo aunque nadie la haya cambiado.
const META_INTERVAL: Duration = Duration::from_secs(60);
/// Velocidad máxima aceptada a un cliente, en voxeles por segundo, y holgura fija.
const MAX_SPEED: f64 = 160.0;
const MOVE_SLACK: f64 = 8.0;
/// Jugadores por datagrama de entidades (para no pasar del tamaño de un paquete).
const ENTITIES_PER_PACKET: usize = 24;

#[derive(Clone)]
pub struct ServerConfig {
    /// Semilla para un mundo nuevo. Un mundo ya guardado conserva la suya.
    pub seed: u32,
    /// Carpeta del mundo (se crea si no existe).
    pub world_dir: PathBuf,
    /// Carpeta con los paquetes de contenido.
    pub content_dir: PathBuf,
    /// Límites que se aplican a lo que pide cada cliente, en chunks.
    pub max_view_distance: u8,
    pub max_far_distance: u8,
    /// Acepta cualquier posición del cliente sin validar la velocidad. Solo para
    /// herramientas de desarrollo en el servidor integrado.
    pub trust_movement: bool,
    /// Genera el mundo entero con un solo bioma, para trabajarlo por separado.
    pub solo_biome: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            seed: 1337,
            world_dir: PathBuf::from("worlds").join("default"),
            content_dir: std::env::var_os("KURAVOXEL_CONTENT")
                .map_or_else(|| PathBuf::from("content"), PathBuf::from),
            max_view_distance: 16,
            max_far_distance: 160,
            trust_movement: false,
            solo_biome: std::env::var("KURAVOXEL_BIOME").ok().filter(|name| !name.is_empty()),
        }
    }
}

/// Canal de salida hacia un cliente. Devuelve `false` si el cliente ya no está.
pub type Sink = Box<dyn Fn(ServerMsg) -> bool + Send>;

pub enum Event {
    Connect { id: PlayerId, sink: Sink, operator: bool },
    Message { id: PlayerId, msg: ClientMsg },
    Disconnect { id: PlayerId },
    /// Con la generación del generador que los hizo (cambia al recargar el terreno).
    Generated(u32, IVec3, Chunk),
    LodGenerated(u32, LodTile),
    /// Revisión de una columna de chunks editada, para el terreno lejano.
    BuildsScanned { column: (i32, i32), seq: u64, built: Vec<builds::BuiltColumn> },
    Shutdown,
}

/// Forma de hablar con un servidor en marcha, sea cual sea el transporte.
#[derive(Clone)]
pub struct ServerHandle {
    events: Sender<Event>,
    next_id: Arc<AtomicU32>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl ServerHandle {
    /// Registra una conexión nueva y devuelve su identificador.
    pub fn connect(&self, sink: Sink) -> PlayerId {
        self.register(sink, false)
    }

    /// Como `connect`, pero la conexión puede ejecutar comandos de consola del servidor.
    /// Es la del anfitrión: quien tiene el servidor en su propio proceso.
    pub fn connect_operator(&self, sink: Sink) -> PlayerId {
        self.register(sink, true)
    }

    fn register(&self, sink: Sink, operator: bool) -> PlayerId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let _ = self.events.send(Event::Connect { id, sink, operator });
        id
    }

    pub fn message(&self, id: PlayerId, msg: ClientMsg) {
        let _ = self.events.send(Event::Message { id, msg });
    }

    pub fn disconnect(&self, id: PlayerId) {
        let _ = self.events.send(Event::Disconnect { id });
    }

    /// Detiene el servidor y espera a que termine de guardar el mundo.
    pub fn shutdown(&self) {
        let _ = self.events.send(Event::Shutdown);
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }
}

/// Arranca el servidor en su propio hilo. Falla si el contenido no se puede cargar.
pub fn start(config: ServerConfig) -> anyhow::Result<ServerHandle> {
    let dir = &config.content_dir;
    let fingerprint = content::fingerprint(dir);
    let (storage, meta) = Storage::open(&config.world_dir)?;
    // Los chunks guardados usan los números de bloque de cuando se guardaron: se parte de
    // la tabla del mundo para que cada estado de bloque (y cada ítem) conserve el suyo.
    let (known_states, known_items) =
        meta.as_ref().map_or((Vec::new(), Vec::new()), |meta| (meta.blocks.clone(), meta.items.clone()));
    let loaded = content::load(dir)
        .and_then(|loaded| ContentData::assign(loaded, &known_states, &known_items))
        .with_context(|| format!("failed to load content from {}", dir.display()))?;
    log::info!(
        "content: {} blocks ({} states), {} items from {}",
        loaded.block_count(),
        loaded.numbered_states().count(),
        loaded.item_count(),
        dir.display()
    );

    let seed = meta.as_ref().map_or(config.seed, |meta| meta.seed);
    let time = meta
        .as_ref()
        .map_or_else(WorldTime::default, |meta| WorldTime {
            hours: meta.time,
            day_length: meta.day_length,
            day: meta.day,
        })
        .sanitized();
    let terrain_fingerprint = biomes::fingerprint(dir);
    let profiles = meta.as_ref().map(|meta| meta.profiles.clone()).unwrap_or_default();
    let source = WorldSource::load_with(dir, &profiles)?.without_editor();
    let terrain = Terrain::compile(&source, &loaded, seed, config.solo_biome.as_deref())
        .context("failed to build the terrain generator")?;
    log::info!(
        "terrain: {} biomes{}",
        terrain.bands().len(),
        config.solo_biome.as_ref().map_or(String::new(), |name| format!(" (only {name})"))
    );
    storage.write_meta(&world_meta(seed, &loaded, time, &profiles))?;
    log::info!(
        "world: {} ({})",
        config.world_dir.display(),
        if meta.is_some() { "loaded" } else { "new" }
    );

    let (events, inbox) = channel();
    let server_events = events.clone();
    let thread = std::thread::Builder::new()
        .name("server".into())
        .spawn(move || {
            let world = World::new(seed, Arc::new(terrain), storage, server_events);
            let mut server = Server::new(config, seed, time, Arc::new(loaded), fingerprint, world, inbox);
            server.terrain_source = Some(source);
            server.terrain_fingerprint = terrain_fingerprint;
            server.terrain.profiles = profiles;
            server.run()
        })
        .context("failed to spawn the server thread")?;
    Ok(ServerHandle {
        events,
        next_id: Arc::new(AtomicU32::new(1)),
        thread: Arc::new(Mutex::new(Some(thread))),
    })
}

fn world_meta(
    seed: u32,
    content: &ContentData,
    time: WorldTime,
    profiles: &BTreeMap<String, String>,
) -> WorldMeta {
    WorldMeta {
        seed,
        blocks: content.state_names(),
        items: content.item_names(),
        time: time.hours,
        day_length: time.day_length,
        day: time.day,
        profiles: profiles.clone(),
    }
}

struct Player {
    sink: Sink,
    /// Puede ejecutar comandos de consola del servidor.
    operator: bool,
    /// `None` hasta que llega un `Hello` válido.
    session: Option<Session>,
    alive: bool,
}

struct Session {
    name: String,
    pos: DVec3,
    yaw: f32,
    pitch: f32,
    last_move: Instant,
    view_distance: i32,
    far_distance: i32,

    // --- Streaming de chunks ---
    /// Chunk en el que está; `None` fuerza a recalcular todo en la siguiente pasada.
    center: Option<IVec3>,
    load_order: Arc<Vec<IVec3>>,
    cursor: usize,
    /// Chunks que el cliente tiene.
    sent: FxHashSet<IVec3>,
    /// Chunks que le tocan pero aún se están generando.
    pending: Vec<IVec3>,

    // --- Terreno lejano ---
    lod_sent: FxHashSet<TileKey>,
    /// Teselas por enviar, de lejos a cerca (se consumen desde el final).
    lod_pending: Vec<TileKey>,
    /// Teselas que el cliente debe soltar en cuanto tenga sus reemplazos.
    lod_retiring: Vec<TileKey>,

    // --- Herramientas de construcción ---
    /// Lo último que copió, cortó o cargó.
    clipboard: Option<Arc<Prefab>>,
    /// Cambios que puede deshacer, del más antiguo al más reciente: cómo estaban los
    /// chunks antes de cada uno.
    undo: Vec<Vec<(IVec3, Arc<Chunk>)>>,
}

struct Server {
    config: ServerConfig,
    inbox: Receiver<Event>,
    world: World,
    players: FxHashMap<PlayerId, Player>,
    load_orders: FxHashMap<i32, Arc<Vec<IVec3>>>,
    tick: u32,
    seed: u32,
    last_gc: Instant,
    last_save: Instant,
    content: Arc<ContentData>,
    /// Propiedades por número de bloque del contenido actual (física de los ítems).
    blocks: BlockTable,
    content_fingerprint: (usize, Option<SystemTime>),
    last_content_poll: Instant,
    /// El terreno tal como está escrito en disco (el último que compiló) y su huella.
    terrain_source: Option<WorldSource>,
    terrain_fingerprint: (usize, Option<SystemTime>),
    drops: Drops,
    /// Hora del mundo; avanza con los ticks.
    time: WorldTime,
    /// Última vez que se escribió `world.toml` y si la hora ha cambiado a mano desde entonces.
    last_meta: Instant,
    time_changed: bool,
    /// Recarga en caliente, cambios a la espera y perfiles de los biomas en este mundo.
    terrain: TerrainStatus,
}

fn chunk_of(pos: DVec3) -> IVec3 {
    (pos / CHUNK_SIZE as f64).floor().as_ivec3()
}

/// ¿Debe conservarse `pos` para un jugador centrado en `center`?
fn in_keep_volume(pos: IVec3, center: IVec3, view_distance: i32) -> bool {
    let keep_h = (view_distance + LOAD_MARGIN + UNLOAD_MARGIN) as i64;
    let d = pos - center;
    let (dx, dz) = (d.x as i64, d.z as i64);
    dx * dx + dz * dz <= keep_h * keep_h && d.y.abs() <= VERTICAL_RADIUS + UNLOAD_MARGIN
}

/// Offsets de todos los chunks del volumen de carga (un cilindro), de cerca a lejos.
fn build_load_order(view_distance: i32) -> Vec<IVec3> {
    let (rh, rv) = (view_distance + LOAD_MARGIN, VERTICAL_RADIUS);
    let mut order = Vec::new();
    for dy in -rv..=rv {
        for dz in -rh..=rh {
            for dx in -rh..=rh {
                if dx * dx + dz * dz <= rh * rh {
                    order.push(IVec3::new(dx, dy, dz));
                }
            }
        }
    }
    order.sort_unstable_by_key(|d| d.length_squared());
    order
}

impl Server {
    fn new(
        config: ServerConfig,
        seed: u32,
        time: WorldTime,
        content: Arc<ContentData>,
        content_fingerprint: (usize, Option<SystemTime>),
        mut world: World,
        inbox: Receiver<Event>,
    ) -> Self {
        let blocks = BlockTable::new(&content);
        world.set_block_table(&blocks);
        Self {
            world,
            seed,
            last_save: Instant::now(),
            blocks,
            content,
            content_fingerprint,
            drops: Drops::default(),
            time,
            last_meta: Instant::now(),
            time_changed: false,
            last_content_poll: Instant::now(),
            terrain_source: None,
            terrain_fingerprint: (0, None),
            terrain: TerrainStatus { auto: true, ..TerrainStatus::default() },
            config,
            inbox,
            players: FxHashMap::default(),
            load_orders: FxHashMap::default(),
            tick: 0,
            last_gc: Instant::now(),
        }
    }

    fn run(mut self) {
        let tick_length = Duration::from_secs(1) / TICK_RATE;
        let mut next_tick = Instant::now() + tick_length;
        'running: loop {
            let wait = next_tick.saturating_duration_since(Instant::now());
            match self.inbox.recv_timeout(wait) {
                Ok(event) => {
                    if !self.handle(event) {
                        break;
                    }
                    // Se despacha todo lo acumulado antes de volver a repartir chunks.
                    while let Ok(event) = self.inbox.try_recv() {
                        if !self.handle(event) {
                            break 'running;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }

            self.stream_world();

            let now = Instant::now();
            if now >= next_tick {
                self.tick();
                next_tick += tick_length;
                // Tras un parón no se intenta recuperar los ticks perdidos.
                if next_tick < now {
                    next_tick = now + tick_length;
                }
            }
        }
        self.time_changed = true;
        self.save_world();
        log::info!("server stopped");
    }

    /// Devuelve `false` para detener el servidor.
    fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::Connect { id, sink, operator } => {
                self.players.insert(id, Player { sink, operator, session: None, alive: true });
            }
            Event::Message { id, msg } => self.on_message(id, msg),
            Event::Disconnect { id } => self.remove_player(id),
            Event::Generated(epoch, pos, chunk) => self.world.on_generated(epoch, pos, chunk),
            Event::LodGenerated(epoch, tile) => self.world.on_lod_generated(epoch, tile),
            Event::BuildsScanned { column, seq, built } => {
                // Las teselas que cambian se reenvían a quien ya las tenga; el cliente
                // sustituye la suya.
                for tile in self.world.on_builds_scanned(column, seq, built) {
                    for player in self.players.values_mut() {
                        if player.session.as_ref().is_some_and(|s| s.lod_sent.contains(&tile.key)) {
                            player.alive &= (player.sink)(ServerMsg::LodTile(tile.clone()));
                        }
                    }
                }
            }
            Event::Shutdown => return false,
        }
        true
    }

    fn on_message(&mut self, id: PlayerId, msg: ClientMsg) {
        match msg {
            ClientMsg::Hello { version, name, view_distance, far_distance } => {
                self.on_hello(id, version, name, view_distance, far_distance)
            }
            ClientMsg::SetBlock { pos, block } => {
                let (pos, block) = (IVec3::from_array(pos), Block(block));
                let Some(session) = self.players.get(&id).and_then(|p| p.session.as_ref()) else {
                    return;
                };
                let center = pos.as_dvec3() + DVec3::splat(0.5);
                let in_reach = center.distance(session.pos) <= REACH + REACH_SLACK;
                let cells = self.edit_cells(pos, block);
                match cells.filter(|_| in_reach) {
                    Some(cells) => {
                        for (pos, block) in cells {
                            if let Some(chunk) = self.world.set_block(pos, block) {
                                self.broadcast_block(pos, block, chunk);
                            }
                        }
                    }
                    // Rechazada: se devuelve al cliente lo que hay de verdad en todas las
                    // celdas que habrá cambiado al predecir.
                    None => {
                        let touched = if block == Block::AIR { vec![(pos, block)] } else { self.content.structure(block, pos) };
                        for (pos, _) in touched {
                            if let Some(actual) = self.world.block_at(pos) {
                                self.send(id, ServerMsg::BlockChange { pos: pos.to_array(), block: actual.0 });
                            }
                        }
                    }
                }
            }
            ClientMsg::Move { pos, yaw, pitch } => {
                let trust = self.config.trust_movement;
                let bounds = self.world.generator.bounds();
                let Some(player) = self.players.get_mut(&id) else { return };
                let Some(session) = player.session.as_mut() else { return };
                let pos = DVec3::from_array(pos);
                if !pos.is_finite() || !yaw.is_finite() || !pitch.is_finite() {
                    return;
                }
                // Validación de autoridad: el cliente propone, el servidor comprueba que
                // el desplazamiento es posible en el tiempo transcurrido.
                let now = Instant::now();
                let elapsed = (now - session.last_move).as_secs_f64().min(1.0);
                let allowed = MAX_SPEED * elapsed + MOVE_SLACK;
                if !bounds.contains(pos, MOVE_SLACK) {
                    let inside = bounds.clamp(pos, 0.5);
                    player.alive &= (player.sink)(ServerMsg::Teleport { pos: inside.to_array() });
                    session.pos = inside;
                    session.last_move = now;
                    return;
                }
                if !trust && pos.distance(session.pos) > allowed {
                    player.alive &= (player.sink)(ServerMsg::Teleport { pos: session.pos.to_array() });
                    session.last_move = now;
                    return;
                }
                session.pos = pos;
                session.yaw = yaw;
                session.pitch = pitch.clamp(-1.6, 1.6);
                session.last_move = now;
            }
            ClientMsg::DropItem { item, dir } => {
                let Some(session) = self.players.get(&id).and_then(|p| p.session.as_ref()) else {
                    return;
                };
                let dir = Vec3::from_array(dir).as_dvec3();
                let item = ItemId(item);
                if self.content.item(item).is_none() || !dir.is_finite() || dir.length_squared() < 1e-6 {
                    return;
                }
                let eye = session.pos;
                let (drop, evicted) = self.drops.throw(item, eye, dir);
                let pos = self.drops.iter().find(|(d, _)| *d == drop).map(|(_, d)| d.body.pos).unwrap_or(eye);
                self.broadcast(|| ServerMsg::DropSpawned { id: drop, item: item.0, pos: pos.to_array() });
                if let Some(old) = evicted {
                    self.broadcast(|| ServerMsg::DropRemoved { id: old });
                }
            }
            ClientMsg::Prefab(op) => {
                let (text, ok) = match self.prefab_op(id, op) {
                    Ok(text) => (text, true),
                    Err(text) => (text, false),
                };
                self.send(id, ServerMsg::PrefabResult { text, ok });
            }
            ClientMsg::SetTime { hours, day_length } => {
                let allowed = self.players.get(&id).is_some_and(|p| p.operator && p.session.is_some());
                if allowed {
                    self.set_time(hours, day_length);
                }
            }
            ClientMsg::Terrain(op) => {
                let allowed = self.players.get(&id).is_some_and(|p| p.operator && p.session.is_some());
                if allowed {
                    if let Err(problem) = self.terrain_op(op) {
                        // El estado no cambia: se le vuelve a enviar con el problema.
                        log::warn!("terrain: {problem}");
                        let status = TerrainStatus { error: Some(problem), ..self.terrain.clone() };
                        self.send(id, ServerMsg::TerrainStatus(status));
                    }
                }
            }
            ClientMsg::Command { line } => {
                let (text, ok) = match self.run_command(id, &line) {
                    Ok(text) => (text, true),
                    Err(text) => (text, false),
                };
                self.send(id, ServerMsg::CommandOutput { text, ok });
            }
        }
    }

    /// Ejecuta una línea de consola en nombre de un jugador. Devuelve el texto que verá
    /// en su consola; `Err` si el comando no se ejecutó.
    fn run_command(&mut self, id: PlayerId, line: &str) -> Result<String, String> {
        let player = self.players.get(&id).filter(|player| player.session.is_some());
        let Some(player) = player else { return Err("Not in the world yet.".into()) };
        if !player.operator {
            return Err("Only the host of this world can run server commands.".into());
        }
        let command = commands::parse(line)?;
        log::info!("command from #{id}: {}", line.trim());
        // Las coordenadas relativas (`~`) parten de los ojos para `tp` y de los pies para
        // los bloques.
        let eye = player.session.as_ref().map_or(DVec3::ZERO, |session| session.pos);
        let feet = eye - DVec3::new(0.0, EYE_HEIGHT, 0.0);
        let block_named = |content: &ContentData, name: &str| {
            if name == "air" {
                return Ok(Block::AIR);
            }
            content.block_id(name).ok_or_else(|| {
                format!("Unknown block '{name}'. Use names like base:stone or base:wood[axis=x].")
            })
        };
        match command {
            Command::Help => Ok(commands::HELP.join("\n")),
            Command::Seed => Ok(format!("Seed: {}", self.seed)),
            Command::Biome => {
                let terrain = self.world.generator.terrain().clone();
                let here = terrain.biome_at(feet.x.floor() as i32, feet.z.floor() as i32);
                let mut lines = vec![format!("You are in {} ({}).", here.name, here.id)];
                for (index, biome) in terrain.bands().iter().enumerate() {
                    let (from, to) = terrain.band_range(index);
                    lines.push(format!("  {:<28} z {from} to {to}   (tp 0 ~ {})", biome.id, (from + to) / 2));
                }
                Ok(lines.join("\n"))
            }
            Command::Terrain(op) => self.terrain_op(op),
            Command::TerrainProfiles => {
                let dir = &self.config.content_dir;
                let ids: Vec<String> =
                    self.world.generator.terrain().bands().iter().map(|biome| biome.id.clone()).collect();
                let mut lines = vec!["Profiles on disk (terrain profile <biome> <name|default>):".to_string()];
                for id in ids {
                    let profiles = biomes::list_profiles(dir, &id);
                    let active = self.terrain.profiles.get(&id).map_or("default", String::as_str);
                    let others = if profiles.is_empty() { "-".to_string() } else { profiles.join(", ") };
                    lines.push(format!("  {id:<28} using {active:<14} available: {others}"));
                }
                Ok(lines.join("\n"))
            }
            Command::Save => {
                self.save_world();
                Ok("World saved.".into())
            }
            Command::Time => Ok(self.describe_time()),
            Command::TimeSet(hours) => {
                self.set_time(Some(hours), None);
                Ok(self.describe_time())
            }
            Command::TimeCycle(seconds) => {
                self.set_time(None, Some(seconds));
                Ok(self.describe_time())
            }
            Command::TimeDay(day) => {
                self.set_day(day);
                Ok(self.describe_time())
            }
            Command::TimeMoon(phase) => {
                // Esta noche (o la de hoy, si aún es de día) pasa a tener esa fase: la
                // noche `n` empieza la tarde del día `n`.
                let cycle = kuravoxel_common::protocol::MOON_CYCLE_NIGHTS;
                let wanted = (phase * cycle / 8 + cycle / 2) % cycle;
                let before_dawn = self.time.hours < 6.0;
                let tonight = if before_dawn { self.time.day.saturating_sub(1) } else { self.time.day };
                let night = tonight + (wanted + cycle - tonight % cycle) % cycle;
                let day = if before_dawn { night + 1 } else { night };
                self.set_day(day);
                Ok(self.describe_time())
            }
            Command::Players => {
                let mut lines: Vec<String> = self
                    .players
                    .iter()
                    .filter_map(|(id, player)| {
                        let session = player.session.as_ref()?;
                        let pos = session.pos;
                        Some(format!(
                            "#{id} {} at {:.0} {:.0} {:.0}",
                            session.name, pos.x, pos.y, pos.z
                        ))
                    })
                    .collect();
                lines.sort();
                Ok(lines.join("\n"))
            }
            Command::Teleport(pos) => {
                let pos = commands::resolve(pos, eye);
                // Ni siquiera el anfitrión sale de la zona jugable.
                let pos = self.world.generator.bounds().clamp(pos, 0.5);
                let Some(player) = self.players.get_mut(&id) else {
                    return Err("Not in the world yet.".into());
                };
                if let Some(session) = player.session.as_mut() {
                    // La autoridad cambia la posición y el cliente la adopta; los `Move`
                    // que vinieran de camino se rechazan solos por la distancia.
                    session.pos = pos;
                    session.last_move = Instant::now();
                }
                player.alive &= (player.sink)(ServerMsg::Teleport { pos: pos.to_array() });
                Ok(format!("Teleported to {:.1} {:.1} {:.1}", pos.x, pos.y, pos.z))
            }
            Command::SetBlock(pos, name) => {
                let block = block_named(&self.content, &name)?;
                let pos = commands::resolve_voxel(pos, feet);
                let chunk = self
                    .world
                    .set_block(pos, block)
                    .ok_or("That position is not loaded: get closer to it.")?;
                self.broadcast_block(pos, block, chunk);
                Ok(format!("Placed {name} at {} {} {}", pos.x, pos.y, pos.z))
            }
            Command::Fill(from, to, name) => {
                let block = block_named(&self.content, &name)?;
                let (a, b) = (commands::resolve_voxel(from, feet), commands::resolve_voxel(to, feet));
                let (min, max) = (a.min(b), a.max(b));
                let size = (max - min + IVec3::ONE).as_i64vec3();
                let volume = (size.x * size.y * size.z) as u64;
                if volume > commands::MAX_FILL {
                    return Err(format!(
                        "That box has {volume} blocks; the limit is {}.",
                        commands::MAX_FILL
                    ));
                }
                let filled = self.world.fill(min, max, block);
                if filled.changed.is_empty() && filled.missing > 0 {
                    return Err("That box is not loaded: get closer to it.".into());
                }
                self.resend_chunks(&filled.changed);
                let mut text = format!("Filled {volume} blocks with {name}.");
                if filled.missing > 0 {
                    text += &format!(" {} chunks were not loaded and were skipped.", filled.missing);
                }
                Ok(text)
            }
        }
    }

    /// Cambia la hora o la duración del día (lo que venga) y avisa a todos.
    fn set_time(&mut self, hours: Option<f32>, day_length: Option<f32>) {
        let changed = WorldTime {
            hours: hours.unwrap_or(self.time.hours),
            day_length: day_length.unwrap_or(self.time.day_length),
            day: self.time.day,
        }
        .sanitized();
        self.time = changed;
        self.time_changed = true;
        self.broadcast(|| ServerMsg::Time(changed));
    }

    /// La hora y el ritmo del día, para la consola.
    fn describe_time(&self) -> String {
        let cycle = match self.time.day_length {
            length if length <= 0.0 => "time is stopped".to_string(),
            length => format!("a day lasts {:.1} min", length / 60.0),
        };
        format!(
            "Time: day {}, {} ({cycle}); tonight: {}",
            self.time.day,
            self.time.clock(),
            self.time.moon_phase()
        )
    }

    /// Cambia el día del mundo (y con él la fase de la luna) y avisa a todos.
    fn set_day(&mut self, day: u32) {
        self.time = WorldTime { day, ..self.time };
        self.time_changed = true;
        let time = self.time;
        self.broadcast(|| ServerMsg::Time(time));
    }

    /// Reenvía enteros unos chunks que han cambiado en bloque a quien los tenga.
    fn resend_chunks(&mut self, changed: &[IVec3]) {
        for &pos in changed {
            let Some(chunk) = self.world.loaded_chunk(pos) else { continue };
            for player in self.players.values_mut() {
                if player.session.as_ref().is_some_and(|s| s.sent.contains(&pos)) {
                    let msg = ServerMsg::Chunk { pos: pos.to_array(), chunk: chunk.clone() };
                    player.alive &= (player.sink)(msg);
                }
            }
        }
    }

    /// Qué cambia una edición de un jugador, o `None` si no es válida. Colocar pone el
    /// objeto entero (las dos mitades de una puerta) y solo si todas sus celdas están
    /// libres; romper una celda quita todo el objeto al que pertenece.
    fn edit_cells(&self, pos: IVec3, block: Block) -> Option<Vec<(IVec3, Block)>> {
        if block == Block::AIR {
            let current = self.world.block_at(pos)?;
            let cells = self
                .content
                .structure(current, pos)
                .into_iter()
                .filter(|&(cell, expected)| cell == pos || self.world.block_at(cell) == Some(expected))
                .map(|(cell, _)| (cell, Block::AIR))
                .collect();
            return Some(cells);
        }
        // Solo se coloca la celda de anclaje: el resto lo pone el servidor.
        let state = self.content.state(block)?;
        if state.part != 0 {
            return None;
        }
        let cells = self.content.structure(block, pos);
        let free = cells.iter().all(|&(cell, _)| {
            self.world.block_at(cell).is_some_and(|current| self.content.replaceable(current))
        });
        free.then_some(cells)
    }

    /// Avisa del cambio de un voxel a quien tenga su chunk.
    fn broadcast_block(&mut self, pos: IVec3, block: Block, chunk: IVec3) {
        let msg = || ServerMsg::BlockChange { pos: pos.to_array(), block: block.0 };
        for player in self.players.values_mut() {
            if player.session.as_ref().is_some_and(|s| s.sent.contains(&chunk)) {
                player.alive &= (player.sink)(msg());
            }
        }
    }

    fn on_hello(&mut self, id: PlayerId, version: u32, name: String, view: u8, far: u8) {
        let Some(player) = self.players.get_mut(&id) else { return };
        if player.session.is_some() {
            return;
        }
        if version != PROTOCOL_VERSION {
            (player.sink)(ServerMsg::Disconnect {
                reason: format!(
                    "incompatible protocol version {version} (the server uses {PROTOCOL_VERSION})"
                ),
            });
            player.alive = false;
            return;
        }

        let name: String = name.chars().filter(|c| !c.is_control()).take(24).collect();
        let name = if name.trim().is_empty() { format!("Player{id}") } else { name };
        let view_distance = view.clamp(3, self.config.max_view_distance) as i32;
        let far_distance = (far as i32).clamp(view_distance, self.config.max_far_distance as i32);

        let (x, ground, z) = spawn_column(&self.world.generator);
        // Los ojos, con los pies sobre el terreno. Si justo ahí hay un árbol, la física
        // del cliente saca al jugador hacia arriba.
        let spawn = DVec3::new(x as f64 + 0.5, ground as f64 + 1.0 + EYE_HEIGHT, z as f64 + 0.5);

        let load_order = self
            .load_orders
            .entry(view_distance)
            .or_insert_with(|| Arc::new(build_load_order(view_distance)))
            .clone();

        player.alive &= (player.sink)(ServerMsg::Welcome {
            id,
            spawn: spawn.to_array(),
            bounds: self.world.generator.bounds(),
            view_distance: view_distance as u8,
            far_distance: far_distance as u8,
        });
        // El contenido va antes que cualquier chunk: sin él no se sabe pintar los bloques.
        player.alive &= (player.sink)(ServerMsg::Content(self.content.clone()));
        player.alive &= (player.sink)(ServerMsg::Time(self.time));
        if player.operator {
            player.alive &= (player.sink)(ServerMsg::TerrainStatus(self.terrain.clone()));
        }
        player.session = Some(Session {
            name: name.clone(),
            pos: spawn,
            yaw: 0.0,
            pitch: 0.0,
            last_move: Instant::now(),
            view_distance,
            far_distance,
            center: None,
            load_order,
            cursor: 0,
            sent: FxHashSet::default(),
            pending: Vec::new(),
            lod_sent: FxHashSet::default(),
            lod_pending: Vec::new(),
            lod_retiring: Vec::new(),
            clipboard: None,
            undo: Vec::new(),
        });
        log::info!("{name} (#{id}) joined: voxels {view_distance}, far {far_distance}");

        // Presentaciones: los que ya estaban al nuevo, y el nuevo a los demás.
        let others: Vec<(PlayerId, String)> = self
            .players
            .iter()
            .filter(|(other, _)| **other != id)
            .filter_map(|(other, p)| p.session.as_ref().map(|s| (*other, s.name.clone())))
            .collect();
        for (other, other_name) in others {
            self.send(id, ServerMsg::PlayerJoined { id: other, name: other_name });
            self.send(other, ServerMsg::PlayerJoined { id, name: name.clone() });
        }
        // Los ítems que ya había sueltos.
        let drops: Vec<ServerMsg> = self
            .drops
            .iter()
            .map(|(drop, d)| ServerMsg::DropSpawned { id: drop, item: d.item.0, pos: d.body.pos.to_array() })
            .collect();
        for msg in drops {
            self.send(id, msg);
        }
    }

    /// Envía un mensaje a todos los jugadores que ya están en el mundo.
    fn broadcast(&mut self, msg: impl Fn() -> ServerMsg) {
        for player in self.players.values_mut() {
            if player.session.is_some() {
                player.alive &= (player.sink)(msg());
            }
        }
    }

    fn send(&mut self, id: PlayerId, msg: ServerMsg) {
        if let Some(player) = self.players.get_mut(&id) {
            player.alive &= (player.sink)(msg);
        }
    }

    fn remove_player(&mut self, id: PlayerId) {
        let Some(player) = self.players.remove(&id) else { return };
        let Some(session) = player.session else { return };
        log::info!("{} (#{id}) left", session.name);
        for other in self.players.values_mut() {
            if other.session.is_some() {
                other.alive &= (other.sink)(ServerMsg::PlayerLeft { id });
            }
        }
    }

    /// Reparte a cada jugador los chunks y el terreno lejano que le corresponden.
    fn stream_world(&mut self) {
        let Self { players, world, .. } = self;
        for player in players.values_mut() {
            let Some(session) = player.session.as_mut() else { continue };
            let sink = &player.sink;
            let mut alive = player.alive;
            let mut send = |msg: ServerMsg| alive &= sink(msg);

            let center = chunk_of(session.pos);
            if session.center != Some(center) {
                let moved_columns =
                    session.center.map_or(true, |old| (old.x, old.z) != (center.x, center.z));
                session.center = Some(center);
                session.cursor = 0;
                session.pending.clear();

                // Fuera lo que queda lejos.
                let view = session.view_distance;
                let mut unload = Vec::new();
                session.sent.retain(|&pos| {
                    let keep = in_keep_volume(pos, center, view);
                    if !keep {
                        unload.push(pos.to_array());
                    }
                    keep
                });
                if !unload.is_empty() {
                    send(ServerMsg::UnloadChunks(unload));
                }

                if moved_columns {
                    retarget_lod(session, center);
                }
            }

            // Chunks que esperaban generación.
            session.pending.retain(|&pos| match world.chunk(pos) {
                Some(chunk) => {
                    send(ServerMsg::Chunk { pos: pos.to_array(), chunk });
                    session.sent.insert(pos);
                    false
                }
                None => true,
            });

            // Chunks nuevos, de cerca a lejos.
            let mut scanned = 0;
            while session.cursor < session.load_order.len()
                && session.pending.len() < MAX_PENDING_CHUNKS
                && scanned < SCAN_BUDGET
            {
                let pos = center + session.load_order[session.cursor];
                session.cursor += 1;
                scanned += 1;
                if session.sent.contains(&pos) {
                    continue;
                }
                match world.chunk(pos) {
                    Some(chunk) => {
                        send(ServerMsg::Chunk { pos: pos.to_array(), chunk });
                        session.sent.insert(pos);
                    }
                    None => session.pending.push(pos),
                }
            }

            // Terreno lejano: se pide todo de golpe (es poco) y se envía según llega.
            let lod_sent = &mut session.lod_sent;
            session.lod_pending.retain(|&key| match world.lod_tile(key) {
                Some(tile) => {
                    send(ServerMsg::LodTile(tile));
                    lod_sent.insert(key);
                    false
                }
                None => true,
            });
            // Una tesela vieja solo se retira cuando sus reemplazos ya se han enviado,
            // para que el cliente nunca vea un hueco al cambiar de nivel.
            let lod_pending = &session.lod_pending;
            let mut retired = Vec::new();
            session.lod_retiring.retain(|&key| {
                let waiting = lod_pending.iter().any(|&k| lod::overlaps(k, key));
                if !waiting {
                    retired.push(key);
                }
                waiting
            });
            if !retired.is_empty() {
                send(ServerMsg::UnloadLod(retired));
            }

            player.alive = alive;
        }
        self.drop_dead_players();
    }

    fn drop_dead_players(&mut self) {
        let dead: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, player)| !player.alive)
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            self.remove_player(id);
        }
    }

    /// Un paso de simulación: ítems sueltos y difusión de jugadores e ítems en movimiento.
    fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.world.update_builds();

        self.time = self.time.advanced(1.0 / TICK_RATE as f32);
        if self.tick % TIME_SYNC_TICKS == 0 {
            let time = self.time;
            self.broadcast(|| ServerMsg::Time(time));
        }

        let collider = WorldCollider { world: &self.world, blocks: &self.blocks };
        let (moved, expired) = self.drops.step(Duration::from_secs(1) / TICK_RATE, &collider);
        let feet: Vec<(PlayerId, DVec3)> = self
            .players
            .iter()
            .filter_map(|(id, p)| p.session.as_ref().map(|s| (*id, s.pos - DVec3::new(0.0, EYE_HEIGHT, 0.0))))
            .collect();
        for (drop, item, player) in self.drops.pick_up(&feet) {
            self.send(player, ServerMsg::PickedUp { item: item.0 });
            self.broadcast(|| ServerMsg::DropRemoved { id: drop });
        }
        for drop in expired {
            self.broadcast(|| ServerMsg::DropRemoved { id: drop });
        }
        let drops: Vec<DropState> =
            moved.into_iter().map(|(id, pos)| DropState { id, pos: pos.to_array() }).collect();

        let states: Vec<PlayerState> = self
            .players
            .iter()
            .filter_map(|(id, player)| {
                player.session.as_ref().map(|s| PlayerState {
                    id: *id,
                    pos: s.pos.to_array(),
                    yaw: s.yaw,
                    pitch: s.pitch,
                })
            })
            .collect();
        if states.len() > 1 || !drops.is_empty() {
            let tick = self.tick;
            for (id, player) in self.players.iter_mut() {
                if player.session.is_none() {
                    continue;
                }
                let others: Vec<PlayerState> =
                    states.iter().filter(|s| s.id != *id).copied().collect();
                for group in others.chunks(ENTITIES_PER_PACKET) {
                    player.alive &= (player.sink)(ServerMsg::Entities {
                        tick,
                        players: group.to_vec(),
                        drops: Vec::new(),
                    });
                }
                for group in drops.chunks(ENTITIES_PER_PACKET) {
                    player.alive &= (player.sink)(ServerMsg::Entities {
                        tick,
                        players: Vec::new(),
                        drops: group.to_vec(),
                    });
                }
            }
        }

        if self.last_content_poll.elapsed() >= CONTENT_POLL {
            self.last_content_poll = Instant::now();
            self.reload_content_if_changed();
            self.reload_terrain_if_changed();
        }

        if self.last_save.elapsed() >= SAVE_INTERVAL {
            self.save_world();
        }

        if self.last_gc.elapsed() >= GC_INTERVAL {
            self.last_gc = Instant::now();
            self.collect_garbage();
        }
    }

    fn save_world(&mut self) {
        self.last_save = Instant::now();
        // La hora vive en `world.toml`: se escribe si alguien la cambió o cada cierto tiempo.
        if self.time_changed || self.last_meta.elapsed() >= META_INTERVAL {
            self.write_meta();
        }
        match self.world.storage.save() {
            Ok(0) => {}
            Ok(regions) => log::info!("world saved ({regions} regions)"),
            Err(error) => log::error!("failed to save the world: {error:#}"),
        }
    }

    fn write_meta(&mut self) {
        self.last_meta = Instant::now();
        self.time_changed = false;
        let meta = world_meta(self.seed, &self.content, self.time, &self.terrain.profiles);
        if let Err(error) = self.world.storage.write_meta(&meta) {
            log::error!("failed to update world.toml: {error:#}");
        }
    }

    /// Recarga en caliente: si algún archivo de contenido ha cambiado, se vuelve a leer
    /// todo y se envía a los jugadores conectados. Un error deja el contenido anterior.
    fn reload_content_if_changed(&mut self) {
        let dir = &self.config.content_dir;
        let fingerprint = content::fingerprint(dir);
        if fingerprint == self.content_fingerprint {
            return;
        }
        self.content_fingerprint = fingerprint;

        let reloaded = content::load(dir)
            .and_then(|loaded| {
                let (states, items) = (self.content.state_names(), self.content.item_names());
                ContentData::assign(loaded, &states, &items)
            })
            // El generador ya tiene resueltos sus bloques: no pueden desaparecer.
            .and_then(|data| match &self.terrain_source {
                Some(source) => {
                    Terrain::compile(source, &data, self.seed, self.config.solo_biome.as_deref()).map(|_| data)
                }
                None => Ok(data),
            });
        match reloaded {
            Ok(data) => {
                log::info!(
                    "content reloaded: {} blocks, {} items",
                    data.block_count(),
                    data.item_count()
                );
                // Los bloques nuevos tienen número nuevo: la tabla del mundo debe saberlo.
                self.blocks = BlockTable::new(&data);
                self.world.set_block_table(&self.blocks);
                self.content = Arc::new(data);
                self.write_meta();
                for player in self.players.values_mut() {
                    if player.session.is_some() {
                        player.alive &= (player.sink)(ServerMsg::Content(self.content.clone()));
                    }
                }
            }
            Err(error) => log::error!("content not reloaded, keeping the previous one: {error:#}"),
        }
    }

    /// Recarga en caliente del terreno: si cambia algún archivo de `worldgen/`, se
    /// compila de nuevo y, si la recarga automática está activada, todo lo que no esté
    /// editado se regenera y se reenvía. Si no, el cambio espera a `TerrainOp::Apply`.
    /// Un error deja el terreno anterior.
    fn reload_terrain_if_changed(&mut self) {
        let fingerprint = biomes::fingerprint(&self.config.content_dir);
        if fingerprint == self.terrain_fingerprint {
            return;
        }
        self.terrain_fingerprint = fingerprint;
        self.check_terrain(false);
    }

    /// Compila el terreno de los archivos (con los perfiles de este mundo) y lo usa si
    /// cambia algo y toca: con la recarga automática o si se pide (`apply`).
    fn check_terrain(&mut self, apply: bool) {
        let before = self.terrain.clone();
        self.update_terrain(apply);
        if self.terrain != before {
            self.send_terrain_status();
        }
    }

    /// Envía el estado del terreno a quien puede cambiarlo.
    fn send_terrain_status(&mut self) {
        let status = self.terrain.clone();
        for player in self.players.values_mut() {
            if player.operator && player.session.is_some() {
                player.alive &= (player.sink)(ServerMsg::TerrainStatus(status.clone()));
            }
        }
    }

    /// Una orden sobre el terreno (de la consola o del editor de biomas). Devuelve el
    /// texto para la consola.
    fn terrain_op(&mut self, op: TerrainOp) -> Result<String, String> {
        match op {
            TerrainOp::Status => {}
            TerrainOp::Auto(on) => {
                self.terrain.auto = on;
                // Al volver a activarla se usa ya lo que estuviera esperando.
                self.update_terrain(false);
            }
            TerrainOp::Apply => self.update_terrain(true),
            TerrainOp::Profile { biome, profile } => {
                let id = match &self.terrain_source {
                    Some(source) => source.resolve(&biome).map_err(|error| format!("{error:#}"))?.to_string(),
                    None => biome,
                };
                match profile {
                    Some(profile) => {
                        if !biomes::valid_profile_name(&profile) {
                            return Err(format!(
                                "\"{profile}\" is not a valid profile name: use lowercase letters, digits and _"
                            ));
                        }
                        if biomes::find_profile(&self.config.content_dir, &id, &profile).is_none() {
                            return Err(format!("{id} has no profile called \"{profile}\" (terrain profiles lists them)"));
                        }
                        self.terrain.profiles.insert(id, profile);
                    }
                    None => {
                        self.terrain.profiles.remove(&id);
                    }
                }
                self.write_meta();
                self.update_terrain(false);
            }
        }
        self.send_terrain_status();
        Ok(self.describe_terrain())
    }

    fn describe_terrain(&self) -> String {
        let status = &self.terrain;
        let mut lines = vec![format!(
            "Terrain: live reload {}{}",
            if status.auto { "on" } else { "off" },
            if status.pending { "; changes are waiting (terrain apply)" } else { "" }
        )];
        if let Some(error) = &status.error {
            lines.push(format!("The files have an error, the world keeps the previous terrain: {error}"));
        }
        if status.profiles.is_empty() {
            lines.push("Every biome uses its default file.".into());
        }
        for (biome, profile) in &status.profiles {
            lines.push(format!("  {biome:<28} profile {profile}"));
        }
        lines.join("\n")
    }

    fn update_terrain(&mut self, apply: bool) {
        let dir = &self.config.content_dir;
        let compiled = WorldSource::load_with(dir, &self.terrain.profiles).map(WorldSource::without_editor).and_then(|source| {
            let terrain = Terrain::compile(&source, &self.content, self.seed, self.config.solo_biome.as_deref())?;
            Ok((source, terrain))
        });
        let (source, terrain) = match compiled {
            Ok(compiled) => compiled,
            Err(error) => {
                log::error!("terrain not reloaded, keeping the previous one: {error:#}");
                self.terrain.error = Some(format!("{error:#}"));
                return;
            }
        };
        self.terrain.error = None;
        // Solo ha cambiado lo que no genera nada (comentarios, el lienzo del editor): no
        // hay que regenerar el mundo.
        if self.terrain_source.as_ref() == Some(&source) {
            self.terrain.pending = false;
            return;
        }
        if !apply && !self.terrain.auto {
            if !self.terrain.pending {
                log::info!("terrain changed: waiting to be applied");
            }
            self.terrain.pending = true;
            return;
        }
        self.terrain.pending = false;
        log::info!("terrain reloaded: {} biomes", terrain.bands().len());
        self.terrain_source = Some(source);
        self.world.set_generator(WorldGen::new(self.seed, Arc::new(terrain)));

        // Los clientes sustituyen lo que ya tienen según les llega la versión nueva.
        for player in self.players.values_mut() {
            let Some(session) = player.session.as_mut() else { continue };
            let center = chunk_of(session.pos);
            // Lo que cae dentro de la lista de carga se reenviará; el resto se suelta.
            let (reach, view) = ((session.view_distance + LOAD_MARGIN) as i64, session.view_distance);
            let unload: Vec<[i32; 3]> = session
                .sent
                .drain()
                .filter(|&pos| {
                    let d = pos - center;
                    let flat = (d.x as i64).pow(2) + (d.z as i64).pow(2);
                    flat > reach * reach || d.y.abs() > VERTICAL_RADIUS || !in_keep_volume(pos, center, view)
                })
                .map(|pos| pos.to_array())
                .collect();
            if !unload.is_empty() {
                player.alive &= (player.sink)(ServerMsg::UnloadChunks(unload));
            }
            session.center = Some(center);
            session.cursor = 0;
            session.pending.clear();
            retarget_lod(session, center);
            let resend: Vec<TileKey> = session.lod_sent.drain().collect();
            session.lod_pending.extend(resend);
        }
    }

    /// Suelta los chunks y teselas que ningún jugador tiene a su alcance.
    fn collect_garbage(&mut self) {
        let Self { players, world, .. } = self;
        let sessions: Vec<&Session> =
            players.values().filter_map(|player| player.session.as_ref()).collect();
        let cache_center = match sessions.as_slice() {
            [only] => only.center.map(|c| (c.x, c.z, only.far_distance + 8)),
            _ => None,
        };
        world.retain(
            |pos| {
                sessions.iter().any(|s| {
                    s.center.is_some_and(|center| in_keep_volume(pos, center, s.view_distance))
                })
            },
            |key| {
                sessions
                    .iter()
                    .any(|s| s.lod_sent.contains(&key) || s.lod_pending.contains(&key))
            },
            cache_center,
        );
        log::debug!("gc: {} chunks in memory", world.chunk_count());
    }
}

/// Dónde aparecen los jugadores: la columna en tierra firme más cercana al origen (x,
/// altura del suelo, z). Si el origen cae en un río o en el mar, se busca alrededor.
fn spawn_column(generator: &WorldGen) -> (i32, i32, i32) {
    const STEP: i32 = 8;
    for ring in 0..96 {
        let radius = ring * STEP;
        for offset in (-radius..=radius).step_by(STEP as usize) {
            for (x, z) in [(offset, -radius), (offset, radius), (-radius, offset), (radius, offset)] {
                let ground = generator.height_at(x, z);
                if ground > worldgen::SEA_LEVEL {
                    return (x, ground, z);
                }
            }
        }
    }
    (0, generator.height_at(0, 0).max(worldgen::SEA_LEVEL), 0)
}

/// Recalcula qué teselas de terreno lejano necesita el jugador.
fn retarget_lod(session: &mut Session, center: IVec3) {
    // El nivel más fino llega siempre algo más allá de los voxeles reales.
    let fine_radius = session.view_distance + 6;
    let desired = lod::desired_tiles(center.x, center.z, session.far_distance, fine_radius);
    let wanted: FxHashSet<TileKey> = desired.iter().copied().collect();

    for &key in &session.lod_sent {
        if !wanted.contains(&key) && !session.lod_retiring.contains(&key) {
            session.lod_retiring.push(key);
        }
    }
    session.lod_retiring.retain(|key| !wanted.contains(key));
    let retiring = &session.lod_retiring;
    session.lod_sent.retain(|key| !retiring.contains(key));

    // De lejos a cerca: `stream_world` las pide todas y las envía según se generan.
    session.lod_pending = desired
        .into_iter()
        .filter(|key| !session.lod_sent.contains(key))
        .rev()
        .collect();
}
