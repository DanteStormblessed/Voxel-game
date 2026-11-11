//! Ventana, entrada, conexión con el servidor y bucle principal.

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use glam::{DVec3, IVec3, Vec3};
use rustc_hash::FxHashMap;
use voxel_common::block::Block;
use voxel_common::content::ContentData;
use voxel_common::protocol::{
    ClientMsg, PlayerId, ServerMsg, DEFAULT_PORT, PROTOCOL_VERSION, REACH, TICK_RATE,
};
use voxel_server::{ServerConfig, ServerHandle};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::camera::{Camera, Input};
use crate::net::Connection;
use crate::render::{RenderSettings, Renderer};
use crate::world::{World, WorldEvent};

const DEFAULT_VIEW_DISTANCE: u8 = 12;
const DEFAULT_FAR_DISTANCE: u8 = 32;
/// Altura sobre el suelo a partir de la cual los voxeles reales empiezan a ceder todo el
/// terreno al lejano, y altura a la que ya no se dibuja ninguno (por debajo de la cámara
/// el servidor solo envía unos pocos chunks en vertical).
const CLIP_FADE_ALTITUDE: (f32, f32) = (64.0, 112.0);
/// Rapidez con la que los demás jugadores se acercan a su última posición conocida.
const REMOTE_SMOOTHING: f32 = 14.0;

/// Cómo se juega esta sesión.
enum Mode {
    /// Servidor integrado; si hay dirección, además abierto a la red.
    Host(Option<SocketAddr>),
    /// Cliente de un servidor remoto.
    Join(SocketAddr),
}

struct Options {
    mode: Mode,
    name: String,
    /// Mundo que abre el servidor integrado (carpeta dentro de `worlds/`).
    world: String,
}

fn resolve(text: &str) -> anyhow::Result<SocketAddr> {
    let with_port = if text.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok()) {
        text.to_string()
    } else {
        format!("{text}:{DEFAULT_PORT}")
    };
    with_port
        .to_socket_addrs()
        .with_context(|| format!("invalid address: {text}"))?
        .next()
        .with_context(|| format!("could not resolve {text}"))
}

fn parse_options() -> anyhow::Result<Option<Options>> {
    let mut options = Options {
        mode: Mode::Host(None),
        name: std::env::var("USER").unwrap_or_else(|_| "Player".into()),
        world: "default".into(),
    };
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--connect" => {
                let addr = args.next().context("--connect needs an address")?;
                options.mode = Mode::Join(resolve(&addr)?);
            }
            "--host" => {
                // La dirección es opcional: por defecto, todas las interfaces.
                let addr = match args.peek() {
                    Some(next) if !next.starts_with("--") => resolve(&args.next().unwrap())?,
                    _ => resolve("0.0.0.0")?,
                };
                options.mode = Mode::Host(Some(addr));
            }
            "--name" => options.name = args.next().context("--name needs a name")?,
            "--world" => options.world = args.next().context("--world needs a name")?,
            "--help" | "-h" => {
                println!(
                    "usage: voxel [--connect ADDRESS[:PORT]] [--host [ADDRESS[:PORT]]] [--world NAME] [--name NAME]\n\n\
                     With no arguments you play alone, on an integrated server.\n\
                     --host     opens the integrated server to the network (port {DEFAULT_PORT} by default)\n\
                     --connect  joins a server\n\
                     --world    world opened by the integrated server (worlds/NAME, default \"default\")\n\n\
                     Left click breaks, right click places, middle click picks, wheel or 1-9 selects."
                );
                return Ok(None);
            }
            other => anyhow::bail!("unknown argument: {other} (try --help)"),
        }
    }
    Ok(Some(options))
}

pub fn run() -> anyhow::Result<()> {
    let Some(options) = parse_options()? else { return Ok(()) };
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App { options: Some(options), state: None };
    event_loop.run_app(&mut app)?;
    // El servidor integrado guarda el mundo antes de salir.
    if let Some(server) = app.state.and_then(|state| state.server) {
        server.shutdown();
    }
    Ok(())
}

/// Lee un ajuste numérico de una variable de entorno.
fn env_number<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

struct App {
    options: Option<Options>,
    state: Option<State>,
}

/// Otro jugador, tal como lo ve este cliente.
struct RemotePlayer {
    name: String,
    /// Última posición recibida del servidor.
    target: Option<(DVec3, f32)>,
    /// Posición que se dibuja, suavizada hacia `target`.
    shown: Option<(DVec3, f32)>,
}

struct State {
    window: Arc<Window>,
    renderer: Renderer,
    connection: Connection,
    /// Servidor integrado, si esta sesión lo aloja.
    server: Option<ServerHandle>,
    world: World,
    camera: Camera,
    input: Input,
    events: Vec<WorldEvent>,
    grabbed: bool,

    /// Identidad y distancias concedidas por el servidor; `None` hasta el `Welcome`.
    session: Option<(PlayerId, i32)>,
    players: FxHashMap<PlayerId, RemotePlayer>,
    content: Arc<ContentData>,
    /// Bloque que se coloca con el clic derecho.
    selected: Block,
    /// Bloque al que se apunta y hueco anterior, recalculados cada frame.
    target: Option<(IVec3, IVec3)>,
    next_move: Instant,
    last_message: Instant,
    exit: bool,

    last_frame: Instant,
    stats_since: Instant,
    stats_frames: u32,
    /// Momento en que empezó la carga en curso; `None` cuando el mundo está al día.
    loading_since: Option<Instant>,
    /// Desarrollo: vista inicial fija y captura al terminar de cargar.
    dev_camera: Option<[f64; 5]>,
    screenshot: Option<PathBuf>,
}

impl State {
    fn new(event_loop: &ActiveEventLoop, options: Options) -> anyhow::Result<Self> {
        let view_distance = env_number("VOXEL_VIEW_DISTANCE", DEFAULT_VIEW_DISTANCE).max(3);
        let far_distance = env_number("VOXEL_FAR_DISTANCE", DEFAULT_FAR_DISTANCE).max(view_distance);
        let vsync = env_number("VOXEL_VSYNC", 1u8) != 0;
        let screenshot = std::env::var_os("VOXEL_SCREENSHOT").map(PathBuf::from);
        // Desarrollo: VOXEL_CAMERA="x,y,z,yaw,pitch" (ángulos en grados) fija la vista inicial.
        let dev_camera = std::env::var("VOXEL_CAMERA").ok().and_then(|spec| {
            let v: Vec<f64> = spec.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            let parsed = <[f64; 5]>::try_from(v).ok();
            if parsed.is_none() {
                log::warn!("VOXEL_CAMERA must be \"x,y,z,yaw,pitch\"");
            }
            parsed
        });

        let title = match &options.mode {
            Mode::Host(None) => "Voxel".to_string(),
            Mode::Host(Some(addr)) => format!("Voxel (hosting on {addr})"),
            Mode::Join(addr) => format!("Voxel ({addr})"),
        };
        let attributes = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(event_loop.create_window(attributes)?);

        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            &RenderSettings {
                far_distance: far_distance as i32,
                vsync,
                allow_screenshots: screenshot.is_some(),
            },
        ))?;

        // Jugar solo también pasa por un servidor: el integrado.
        let mut hosted = None;
        let connection = match options.mode {
            Mode::Host(listen) => {
                let server = voxel_server::start(ServerConfig {
                    world_dir: PathBuf::from("worlds").join(&options.world),
                    seed: env_number("VOXEL_SEED", ServerConfig::default().seed),
                    // La vista de desarrollo salta a cualquier sitio: sin validar movimiento.
                    trust_movement: dev_camera.is_some(),
                    ..Default::default()
                })?;
                if let Some(addr) = listen {
                    voxel_server::quic::listen(server.clone(), addr)?;
                }
                let connection = Connection::local(&server);
                hosted = Some(server);
                connection
            }
            Mode::Join(addr) => Connection::remote(addr)?,
        };
        connection.send(ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            name: options.name,
            view_distance,
            far_distance,
        });

        let now = Instant::now();
        Ok(Self {
            window,
            renderer,
            connection,
            server: hosted,
            world: World::new(),
            camera: Camera::new(DVec3::ZERO),
            input: Input::default(),
            events: Vec::new(),
            grabbed: false,
            session: None,
            players: FxHashMap::default(),
            content: Arc::default(),
            selected: Block::AIR,
            target: None,
            next_move: now,
            last_message: now,
            exit: false,
            last_frame: now,
            stats_since: now,
            stats_frames: 0,
            loading_since: Some(now),
            dev_camera,
            screenshot,
        })
    }

    fn set_grab(&mut self, grab: bool) {
        if grab {
            let grabbed = self
                .window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined))
                .is_ok();
            self.window.set_cursor_visible(!grabbed);
            self.grabbed = grabbed;
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
            self.window.set_cursor_visible(true);
            self.grabbed = false;
        }
    }

    fn on_key(&mut self, code: KeyCode, pressed: bool, event_loop: &ActiveEventLoop) {
        match code {
            KeyCode::KeyW => self.input.forward = pressed,
            KeyCode::KeyS => self.input.back = pressed,
            KeyCode::KeyA => self.input.left = pressed,
            KeyCode::KeyD => self.input.right = pressed,
            KeyCode::Space => self.input.up = pressed,
            KeyCode::ShiftLeft => self.input.down = pressed,
            KeyCode::ControlLeft => self.input.boost = pressed,
            KeyCode::Digit1 if pressed => self.select_nth(0),
            KeyCode::Digit2 if pressed => self.select_nth(1),
            KeyCode::Digit3 if pressed => self.select_nth(2),
            KeyCode::Digit4 if pressed => self.select_nth(3),
            KeyCode::Digit5 if pressed => self.select_nth(4),
            KeyCode::Digit6 if pressed => self.select_nth(5),
            KeyCode::Digit7 if pressed => self.select_nth(6),
            KeyCode::Digit8 if pressed => self.select_nth(7),
            KeyCode::Digit9 if pressed => self.select_nth(8),
            KeyCode::Escape if pressed => {
                if self.grabbed {
                    self.set_grab(false);
                } else {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    /// Números de los bloques que existen, en orden.
    fn block_ids(&self) -> Vec<Block> {
        (1..self.content.blocks.len())
            .filter(|&i| self.content.blocks[i].is_some())
            .map(|i| Block(i as u8))
            .collect()
    }

    fn select_nth(&mut self, index: usize) {
        if let Some(&block) = self.block_ids().get(index) {
            self.selected = block;
        }
    }

    /// Pasa al bloque siguiente o anterior de la lista.
    fn cycle_selection(&mut self, step: i32) {
        let ids = self.block_ids();
        if ids.is_empty() {
            return;
        }
        let current = ids.iter().position(|&b| b == self.selected).unwrap_or(0) as i32;
        self.selected = ids[(current + step).rem_euclid(ids.len() as i32) as usize];
    }

    fn selected_name(&self) -> &str {
        self.content
            .blocks
            .get(self.selected.0 as usize)
            .and_then(|block| block.as_ref())
            .map_or("-", |block| block.name.as_str())
    }

    /// Clic con el ratón capturado: romper, colocar o copiar el bloque apuntado.
    fn on_click(&mut self, button: MouseButton) {
        let Some((hit, before)) = self.target else { return };
        let (pos, block) = match button {
            MouseButton::Left => (hit, Block::AIR),
            MouseButton::Right if self.selected != Block::AIR => (before, self.selected),
            MouseButton::Middle => {
                if let Some(block) = self.world.block_at(hit) {
                    self.selected = block;
                }
                return;
            }
            _ => return,
        };
        // Predicción: el cambio se ve al instante; el servidor lo confirma o lo corrige.
        self.world.set_block(pos, block);
        self.connection.send(ClientMsg::SetBlock { pos: pos.to_array(), block: block.0 });
    }

    /// Aplica todo lo que ha llegado del servidor desde el frame anterior.
    fn receive(&mut self, now: Instant) {
        while let Ok(msg) = self.connection.incoming.try_recv() {
            // Las fotos de entidades llegan sin parar: no cuentan como carga de mundo.
            if !msg.is_unreliable() {
                self.last_message = now;
            }
            match msg {
                ServerMsg::Welcome { id, spawn, view_distance, far_distance } => {
                    log::info!(
                        "joined as player #{id}: voxels {view_distance}, far {far_distance}"
                    );
                    self.session = Some((id, view_distance as i32));
                    self.world.set_view_distance(view_distance as i32);
                    self.renderer.set_far_distance(far_distance as i32);
                    self.camera.pos = DVec3::from_array(spawn);
                    if let Some([x, y, z, yaw, pitch]) = self.dev_camera {
                        self.camera.pos = DVec3::new(x, y, z);
                        self.camera.yaw = (yaw as f32).to_radians();
                        self.camera.pitch = (pitch as f32).to_radians();
                    }
                }
                ServerMsg::Disconnect { reason } => {
                    log::error!("disconnected from the server: {reason}");
                    self.exit = true;
                }
                ServerMsg::Content(content) => {
                    log::info!("content received: {} blocks", content.block_count());
                    self.renderer.set_palette(&voxel_common::block::palette(&content));
                    self.content = content;
                    // Si el bloque elegido ya no existe (o aún no hay ninguno), el primero.
                    if !self.block_ids().contains(&self.selected) {
                        self.select_nth(0);
                    }
                }
                ServerMsg::Chunk { pos, chunk } => {
                    self.world.insert_chunk(IVec3::from_array(pos), chunk)
                }
                ServerMsg::BlockChange { pos, block } => {
                    self.world.set_block(IVec3::from_array(pos), Block(block))
                }
                ServerMsg::UnloadChunks(positions) => {
                    self.world.unload_chunks(&positions, &mut self.events)
                }
                ServerMsg::LodTile(tile) => self.world.insert_lod(tile, &mut self.events),
                ServerMsg::UnloadLod(keys) => self.world.unload_lod(&keys),
                ServerMsg::PlayerJoined { id, name } => {
                    log::info!("{name} joined");
                    self.players.insert(id, RemotePlayer { name, target: None, shown: None });
                }
                ServerMsg::PlayerLeft { id } => {
                    if let Some(player) = self.players.remove(&id) {
                        log::info!("{} left", player.name);
                    }
                }
                ServerMsg::Entities { players, .. } => {
                    for state in players {
                        if let Some(player) = self.players.get_mut(&state.id) {
                            player.target = Some((DVec3::from_array(state.pos), state.yaw));
                        }
                    }
                }
                // El servidor no aceptó el movimiento: manda su posición.
                ServerMsg::Teleport { pos } => self.camera.pos = DVec3::from_array(pos),
            }
        }
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        // Tope al delta para que un tirón no teletransporte la cámara.
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        self.receive(now);

        let mut clip = 0.0;
        if let Some((_, view_distance)) = self.session {
            // El cliente mueve su cámara al instante (predicción) y se lo cuenta al
            // servidor a ritmo de tick; el servidor valida y, si no cuadra, corrige.
            self.camera.update(&mut self.input, dt);
            if now >= self.next_move {
                self.next_move = now + Duration::from_secs(1) / TICK_RATE;
                self.connection.send(ClientMsg::Move {
                    pos: self.camera.pos.to_array(),
                    yaw: self.camera.yaw,
                    pitch: self.camera.pitch,
                });
            }

            // Al ganar altura los chunks reales del suelo dejan de llegar: el radio de
            // voxeles se encoge y el terreno lejano cubre el hueco.
            let pos = self.camera.pos;
            let ground = self.world.ground_height(pos.x.floor() as i32, pos.z.floor() as i32);
            let altitude = ground.map_or(0.0, |ground| pos.y as f32 - ground as f32);
            let (fade_from, fade_to) = CLIP_FADE_ALTITUDE;
            let fade = ((fade_to - altitude) / (fade_to - fade_from)).clamp(0.0, 1.0);
            clip = (view_distance * 32) as f32 * fade;
        }

        self.world.update(self.camera.chunk_pos(), &mut self.events);
        for event in self.events.drain(..) {
            match event {
                WorldEvent::Mesh { key, mesh } => self.renderer.upload_chunk(key, &mesh),
                WorldEvent::Unload { key } => self.renderer.remove_chunk(key),
            }
        }

        // Los demás jugadores se dibujan suavizados entre una foto del servidor y la siguiente.
        let blend = 1.0 - (-REMOTE_SMOOTHING * dt).exp();
        for player in self.players.values_mut() {
            let Some((target, target_yaw)) = player.target else { continue };
            let (pos, yaw) = player.shown.unwrap_or((target, target_yaw));
            let turn = (target_yaw - yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            player.shown = Some((pos.lerp(target, blend as f64), yaw + turn * blend));
        }
        // Marcadores: punto de mira y contorno del bloque apuntado.
        let forward = self.camera.forward();
        self.target = self.world.raycast(self.camera.pos, forward, REACH);
        let mut boxes = vec![(
            self.camera.pos + (forward * 0.2).as_dvec3(),
            Vec3::splat(0.0018),
            [1.0, 1.0, 1.0],
        )];
        if let Some((hit, _)) = self.target {
            // Las 12 aristas del cubo, como barras finas ligeramente por fuera.
            const THICKNESS: f32 = 0.025;
            let center = hit.as_dvec3() + DVec3::splat(0.5);
            for axis in 0..3 {
                let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
                let mut size = Vec3::splat(THICKNESS);
                size[axis] = 1.0 + THICKNESS;
                for (sa, sb) in [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)] {
                    let mut offset = DVec3::ZERO;
                    offset[a] = sa * 1.004;
                    offset[b] = sb * 1.004;
                    boxes.push((center + offset, size, [0.02, 0.02, 0.02]));
                }
            }
        }
        self.renderer.set_entities(
            &self.camera,
            self.players
                .iter()
                .filter_map(|(id, player)| player.shown.map(|(pos, yaw)| (pos, yaw, *id))),
            &boxes,
        );

        self.track_loading(now);

        match self.renderer.render(&self.camera, clip) {
            Ok(()) => {}
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.renderer.reconfigure()
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                log::error!("the GPU is out of memory");
                event_loop.exit();
            }
            Err(error) => log::warn!("frame dropped: {error:?}"),
        }

        self.update_stats(now);
        if self.exit {
            event_loop.exit();
        }
    }

    /// Registra cuánto tarda cada tanda de carga y dispara la captura de desarrollo.
    ///
    /// El cliente no sabe qué le falta por recibir, así que da el mundo por cargado
    /// cuando no queda nada por mallar y el servidor lleva un momento sin enviar nada.
    fn track_loading(&mut self, now: Instant) {
        let quiet = now - self.last_message;
        let settled = self.session.is_some()
            && self.world.is_idle()
            && self.world.stats().chunks > 0
            && quiet >= Duration::from_millis(400);
        if !settled {
            if quiet < Duration::from_millis(400) {
                self.loading_since.get_or_insert(now);
            }
            return;
        }
        let Some(since) = self.loading_since.take() else { return };
        let world = self.world.stats();
        let render = self.renderer.stats();
        log::info!(
            "world up to date in {:.2} s: {} chunks, {} far tiles, {} meshes, {} quads",
            (now - since - quiet).as_secs_f32(),
            world.chunks,
            world.lod_tiles,
            render.chunks,
            render.quads,
        );
        if let Some(path) = self.screenshot.take() {
            self.renderer.request_screenshot(path);
            self.exit = true;
        }
    }

    /// Estadísticas en el título de la ventana, dos veces por segundo.
    fn update_stats(&mut self, now: Instant) {
        self.stats_frames += 1;
        let elapsed = now - self.stats_since;
        if elapsed < Duration::from_millis(500) {
            return;
        }
        let fps = self.stats_frames as f32 / elapsed.as_secs_f32();
        let world = self.world.stats();
        let render = self.renderer.stats();
        let pos = self.camera.pos;
        let title = format!(
            "Voxel | {fps:.0} fps | block: {} | players {} | chunks {} (meshing {}) | far {} | \
             meshes {} visible {} | quads {} | draws {} | pos {:.0} {:.0} {:.0}",
            self.selected_name(),
            self.players.len() + 1,
            world.chunks,
            world.meshing,
            world.lod_tiles,
            render.chunks,
            render.visible,
            render.quads,
            render.draw_calls,
            pos.x,
            pos.y,
            pos.z,
        );
        log::debug!("{title}");
        self.window.set_title(&title);
        self.stats_since = now;
        self.stats_frames = 0;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(options) = self.options.take() else { return };
        match State::new(event_loop, options) {
            Ok(state) => self.state = Some(state),
            Err(error) => {
                log::error!("failed to start: {error:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.renderer.resize(size.width, size.height),
            WindowEvent::Focused(false) => {
                state.set_grab(false);
                state.input = Input::default();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                // El primer clic captura el ratón; con el ratón capturado, edita.
                if state.grabbed {
                    state.on_click(button);
                } else if button == MouseButton::Left {
                    state.set_grab(true);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32,
                };
                if lines != 0.0 {
                    state.cycle_selection(if lines > 0.0 { -1 } else { 1 });
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: key_state,
                        repeat: false,
                        ..
                    },
                ..
            } => state.on_key(code, key_state == ElementState::Pressed, event_loop),
            WindowEvent::RedrawRequested => state.frame(event_loop),
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let Some(state) = self.state.as_mut() else { return };
        if let DeviceEvent::MouseMotion { delta } = event {
            if state.grabbed {
                state.input.mouse_dx += delta.0 as f32;
                state.input.mouse_dy += delta.1 as f32;
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}
