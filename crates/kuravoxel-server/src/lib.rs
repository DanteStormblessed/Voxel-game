//! Servidor autoritativo.
//!
//! El mismo servidor sirve para jugar solo (integrado: corre en un hilo del propio juego
//! y el cliente le habla por canales en memoria) y para multijugador (dedicado o abierto
//! a la red: los clientes llegan por QUIC). La lógica no distingue un caso del otro: cada
//! conexión es un `Sink` por el que salen mensajes y eventos `Event::Message` que entran.
//!
//! Bucle: los eventos (conexiones, mensajes, chunks recién generados) se atienden en
//! cuanto llegan; la simulación avanza a ticks fijos de `TICK_RATE` por segundo.

mod commands;
mod drops;
mod noise;
pub mod quic;
mod storage;
mod world;
mod worldgen;

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
use kuravoxel_common::protocol::{
    ClientMsg, DropState, PlayerId, PlayerState, ServerMsg, LOAD_MARGIN, PROTOCOL_VERSION, REACH, TICK_RATE,
    UNLOAD_MARGIN, VERTICAL_RADIUS,
};

use commands::Command;
use storage::{Storage, WorldMeta};
use drops::Drops;
use world::{World, WorldCollider};
use worldgen::TerrainBlocks;

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
        }
    }
}

/// Canal de salida hacia un cliente. Devuelve `false` si el cliente ya no está.
pub type Sink = Box<dyn Fn(ServerMsg) -> bool + Send>;

pub enum Event {
    Connect { id: PlayerId, sink: Sink, operator: bool },
    Message { id: PlayerId, msg: ClientMsg },
    Disconnect { id: PlayerId },
    Generated(IVec3, Chunk),
    LodGenerated(LodTile),
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
    let blocks = TerrainBlocks::resolve(&loaded)?;
    log::info!(
        "content: {} blocks ({} states), {} items from {}",
        loaded.block_count(),
        loaded.numbered_states().count(),
        loaded.item_count(),
        dir.display()
    );

    let seed = meta.as_ref().map_or(config.seed, |meta| meta.seed);
    storage.write_meta(&world_meta(seed, &loaded))?;
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
            let world = World::new(seed, blocks, storage, server_events);
            Server::new(config, seed, Arc::new(loaded), fingerprint, world, inbox).run()
        })
        .context("failed to spawn the server thread")?;
    Ok(ServerHandle {
        events,
        next_id: Arc::new(AtomicU32::new(1)),
        thread: Arc::new(Mutex::new(Some(thread))),
    })
}

fn world_meta(seed: u32, content: &ContentData) -> WorldMeta {
    WorldMeta { seed, blocks: content.state_names(), items: content.item_names() }
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
    drops: Drops,
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
        content: Arc<ContentData>,
        content_fingerprint: (usize, Option<SystemTime>),
        world: World,
        inbox: Receiver<Event>,
    ) -> Self {
        Self {
            world,
            seed,
            last_save: Instant::now(),
            blocks: BlockTable::new(&content),
            content,
            content_fingerprint,
            drops: Drops::default(),
            last_content_poll: Instant::now(),
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
            Event::Generated(pos, chunk) => self.world.on_generated(pos, chunk),
            Event::LodGenerated(tile) => self.world.on_lod_generated(tile),
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
            Command::Save => {
                self.save_world();
                Ok("World saved.".into())
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
                // Cada chunk cambiado se reenvía entero a quien lo tenga.
                for &pos in &filled.changed {
                    let Some(chunk) = self.world.loaded_chunk(pos) else { continue };
                    for player in self.players.values_mut() {
                        if player.session.as_ref().is_some_and(|s| s.sent.contains(&pos)) {
                            let msg = ServerMsg::Chunk { pos: pos.to_array(), chunk: chunk.clone() };
                            player.alive &= (player.sink)(msg);
                        }
                    }
                }
                let mut text = format!("Filled {volume} blocks with {name}.");
                if filled.missing > 0 {
                    text += &format!(" {} chunks were not loaded and were skipped.", filled.missing);
                }
                Ok(text)
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

        let ground = self.world.generator.height_at(0, 0).max(0);
        // Los ojos, con los pies sobre el terreno. Si justo ahí hay un árbol, la física
        // del cliente saca al jugador hacia arriba.
        let spawn = DVec3::new(0.5, ground as f64 + 1.0 + EYE_HEIGHT, 0.5);

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
        match self.world.storage.save() {
            Ok(0) => {}
            Ok(regions) => log::info!("world saved ({regions} regions)"),
            Err(error) => log::error!("failed to save the world: {error:#}"),
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
            .and_then(|data| TerrainBlocks::resolve(&data).map(|_| data));
        match reloaded {
            Ok(data) => {
                log::info!(
                    "content reloaded: {} blocks, {} items",
                    data.block_count(),
                    data.item_count()
                );
                // Los bloques nuevos tienen número nuevo: la tabla del mundo debe saberlo.
                if let Err(error) = self.world.storage.write_meta(&world_meta(self.seed, &data)) {
                    log::error!("failed to update world.toml: {error:#}");
                }
                self.blocks = BlockTable::new(&data);
                self.content = Arc::new(data);
                for player in self.players.values_mut() {
                    if player.session.is_some() {
                        player.alive &= (player.sink)(ServerMsg::Content(self.content.clone()));
                    }
                }
            }
            Err(error) => log::error!("content not reloaded, keeping the previous one: {error:#}"),
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
