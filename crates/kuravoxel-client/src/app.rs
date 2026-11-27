//! Ventana y bucle principal del motor.
//!
//! El motor es dueño de la ventana, el renderer y la partida en curso (si hay). El juego
//! implementa [`Game`] y decide qué pasa: cuándo se entra en un mundo, cuándo se sale y
//! qué interfaz se ve encima. Las órdenes las da a través de [`Context`].
//!
//! Además de la ventana principal (la escena), el juego puede abrir **ventanas extra**
//! para sus herramientas (`Context::open_window`): comparten el dispositivo gráfico, no
//! dibujan escena y el juego pinta en ellas lo que quiera (`Game::draw_window`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use kuravoxel_common::block::Block;
use kuravoxel_common::content::ItemId;
use kuravoxel_common::physics::MoveMode;
use kuravoxel_common::protocol::{PrefabOp, TerrainOp};

use crate::camera::Camera;
use crate::render::{Environment, FrameStatus, Gpu, OverlayTarget, RenderSettings, Renderer, SceneView, WindowSurface};
use crate::session::{ClientSettings, FrameLimit, Overlay, Session, SessionConfig, SessionEvent, SessionInfo};

/// Frames que se esperan antes de la captura de desarrollo cuando no hay partida (da
/// tiempo a que la interfaz cargue fuentes y termine su primera maquetación).
const MENU_SCREENSHOT_FRAME: u32 = 30;
/// Frames entre dos clics del guion de desarrollo (`KURAVOXEL_UI_CLICKS`).
const SCRIPT_STEP_FRAMES: u32 = 12;

/// Nombre que da el juego a una ventana extra.
pub type WindowKey = u32;

/// Cómo se abre una ventana extra.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowSpec {
    pub title: String,
    /// Tamaño inicial en píxeles lógicos.
    pub size: [f64; 2],
}

pub struct AppConfig {
    /// Título de la ventana.
    pub title: String,
    /// Ajustes iniciales; después se cambian con `Context::configure`.
    pub client: ClientSettings,
}

/// Lo que el juego puede pedirle al motor.
pub enum Command {
    /// Entrar en un mundo (termina antes la partida en curso, si la hay).
    StartSession(SessionConfig),
    /// Salir del mundo actual; el servidor integrado guarda antes de cerrarse.
    EndSession,
    /// Cerrar el juego.
    Exit,
    /// Cambiar los ajustes del cliente (vsync, campo de visión, ratón).
    Configure(ClientSettings),
    /// Elegir el bloque que coloca el clic derecho (`Block::AIR` = ninguno).
    SelectBlock(Block),
    /// Capturar o soltar el ratón de la partida.
    GrabMouse(bool),
    /// Enviar una línea de consola al servidor de la partida.
    ServerCommand(String),
    /// Cambiar cómo se mueve el jugador (a pie, volando, atravesando bloques).
    SetMoveMode(MoveMode),
    /// Alejar (> 0) o acercar la cámara ese número de pasos.
    ZoomCamera(i32),
    /// Fijar la distancia de cámara en metros (0 = primera persona).
    SetCameraDistance(f32),
    /// Tirar un ítem al suelo, hacia donde mira la cámara.
    DropItem(ItemId),
    /// Activar (con su alcance en metros) o desactivar el modo herramienta: los clics
    /// dejan de editar y llegan al juego como `SessionEvent::ToolClick`.
    SetTool(Option<f64>),
    /// Marcas que se dibujan sobre el mundo (selección, vista previa de un prefab).
    SetOverlay(Overlay),
    /// Orden de construcción para el servidor (copiar, pegar, guardar un prefab...).
    Prefab(PrefabOp),
    /// Cambiar la hora del mundo (0-24) o la duración del día en segundos reales (0 =
    /// tiempo parado); lo que va en `None` no cambia. Solo lo acepta el servidor al anfitrión.
    SetTime { hours: Option<f32>, day_length: Option<f32> },
    /// Ajustes del ciclo de día y noche y de las sombras (colores, sol, penumbra).
    SetEnvironment(Environment),
    /// Parte de la ventana en la que la escena queda a la vista, como fracciones
    /// `[x0, y0, x1, y1]` (0 = izquierda / arriba); `None` = toda. Fuera de ella no se
    /// dibuja nada: es para cuando una pantalla opaca tapa el resto.
    SetSceneRegion(Option<[f32; 4]>),
    /// Recarga del terreno y perfiles de los biomas, para el servidor de la partida.
    Terrain(TerrainOp),
    /// Abrir una ventana extra (si ya está abierta, se le da el foco).
    OpenWindow(WindowKey, WindowSpec),
    CloseWindow(WindowKey),
}

/// Vista del motor que recibe el juego en cada llamada.
pub struct Context {
    /// La partida en curso, si la hay, tal como estaba al empezar el frame.
    pub session: Option<SessionInfo>,
    /// Lo que ha pasado en la partida desde la llamada anterior.
    pub session_events: Vec<SessionEvent>,
    /// El jugador ha pedido "atrás" (Escape con el ratón libre) durante la partida.
    pub escape: bool,
    /// La partida terminó sola (el servidor cerró la conexión o no se pudo iniciar).
    pub session_lost: Option<String>,
    commands: Vec<Command>,
}

impl Context {
    pub fn new(session: Option<SessionInfo>) -> Self {
        Self {
            session,
            session_events: Vec::new(),
            escape: false,
            session_lost: None,
            commands: Vec::new(),
        }
    }

    /// Hay una partida en curso.
    pub fn in_session(&self) -> bool {
        self.session.is_some()
    }

    pub fn configure(&mut self, settings: ClientSettings) {
        self.commands.push(Command::Configure(settings));
    }

    pub fn select_block(&mut self, block: Block) {
        self.commands.push(Command::SelectBlock(block));
    }

    pub fn grab_mouse(&mut self, grab: bool) {
        self.commands.push(Command::GrabMouse(grab));
    }

    pub fn server_command(&mut self, line: impl Into<String>) {
        self.commands.push(Command::ServerCommand(line.into()));
    }

    pub fn set_move_mode(&mut self, mode: MoveMode) {
        self.commands.push(Command::SetMoveMode(mode));
    }

    pub fn zoom_camera(&mut self, steps: i32) {
        self.commands.push(Command::ZoomCamera(steps));
    }

    pub fn set_camera_distance(&mut self, meters: f32) {
        self.commands.push(Command::SetCameraDistance(meters));
    }

    pub fn drop_item(&mut self, item: ItemId) {
        self.commands.push(Command::DropItem(item));
    }

    pub fn set_tool(&mut self, reach: Option<f64>) {
        self.commands.push(Command::SetTool(reach));
    }

    pub fn set_overlay(&mut self, overlay: Overlay) {
        self.commands.push(Command::SetOverlay(overlay));
    }

    pub fn prefab(&mut self, op: PrefabOp) {
        self.commands.push(Command::Prefab(op));
    }

    pub fn set_time(&mut self, hours: Option<f32>, day_length: Option<f32>) {
        self.commands.push(Command::SetTime { hours, day_length });
    }

    pub fn set_environment(&mut self, environment: Environment) {
        self.commands.push(Command::SetEnvironment(environment));
    }

    pub fn set_scene_region(&mut self, region: Option<[f32; 4]>) {
        self.commands.push(Command::SetSceneRegion(region));
    }

    pub fn terrain(&mut self, op: TerrainOp) {
        self.commands.push(Command::Terrain(op));
    }

    pub fn open_window(&mut self, key: WindowKey, spec: WindowSpec) {
        self.commands.push(Command::OpenWindow(key, spec));
    }

    pub fn close_window(&mut self, key: WindowKey) {
        self.commands.push(Command::CloseWindow(key));
    }

    pub fn start_session(&mut self, config: SessionConfig) {
        self.commands.push(Command::StartSession(config));
    }

    pub fn end_session(&mut self) {
        self.commands.push(Command::EndSession);
    }

    pub fn exit(&mut self) {
        self.commands.push(Command::Exit);
    }

    /// Órdenes acumuladas (las consume el motor; útil también para pruebas).
    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }
}

/// Un juego construido sobre el motor.
///
/// Solo `init` y `update` son obligatorios. Los demás ganchos son para dibujar una
/// interfaz encima de la escena; el motor no impone con qué se hace.
pub trait Game {
    /// Una vez, con la ventana y el dispositivo gráfico ya listos.
    fn init(&mut self, ctx: &mut Context, host: &Host);

    /// Cada frame, antes de dibujar.
    fn update(&mut self, ctx: &mut Context);

    /// Cada evento de ventana, antes que la partida. Devuelve `true` si el juego se lo
    /// queda (por ejemplo, un clic sobre un menú) y la partida no debe verlo.
    fn window_event(&mut self, _event: &WindowEvent) -> bool {
        false
    }

    /// `true` mientras el juego tiene en pantalla algo que acapara ratón y teclado; la
    /// partida sigue viva pero suelta el ratón.
    fn wants_input(&self) -> bool {
        false
    }

    /// `true` mientras no se está jugando (un menú de pausa): el motor dibuja menos frames
    /// (`ClientSettings::background_fps`), como con la ventana sin foco.
    fn idle(&self) -> bool {
        false
    }

    /// Cada frame, con la escena ya dibujada en `view`: aquí se pinta la interfaz.
    fn draw_overlay(&mut self, _view: &wgpu::TextureView, _target: OverlayTarget) {}

    /// Una ventana extra ya está abierta: `host` trae su ventana y el dispositivo (con el
    /// formato de su imagen), para montar en ella una interfaz.
    fn window_opened(&mut self, _key: WindowKey, _host: &Host) {}

    /// Un evento de una ventana extra. Cerrarla (`CloseRequested`) la cierra el motor
    /// solo, y después avisa con `window_closed`.
    fn extra_window_event(&mut self, _key: WindowKey, _event: &WindowEvent) {}

    /// Cada frame, una vez por ventana extra visible, sobre su fondo ya limpio.
    fn draw_window(&mut self, _key: WindowKey, _view: &wgpu::TextureView, _target: OverlayTarget) {}

    /// Una ventana extra se ha cerrado (la cerró el jugador, o se pidió).
    fn window_closed(&mut self, _key: WindowKey) {}
}

/// Una ventana extra abierta.
struct ExtraWindow {
    key: WindowKey,
    window: Arc<Window>,
    surface: WindowSurface,
    focused: bool,
    hidden: bool,
}

/// Fondo de las ventanas extra antes de que el juego pinte encima.
const EXTRA_CLEAR: [f64; 3] = [0.008, 0.009, 0.012];

/// Lo que el motor presta al juego al arrancar para que monte su interfaz.
pub struct Host<'a> {
    pub gpu: Gpu<'a>,
    pub window: &'a Arc<Window>,
}

/// Abre la ventana y ejecuta el juego hasta que se cierre.
pub fn run(config: AppConfig, game: impl Game + 'static) -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App { pending: Some((config, Box::new(game))), state: None };
    event_loop.run_app(&mut app)?;
    // Al soltar el estado, la partida en curso cierra su servidor y guarda.
    drop(app.state.take());
    Ok(())
}

struct App {
    pending: Option<(AppConfig, Box<dyn Game>)>,
    state: Option<State>,
}

struct State {
    window: Arc<Window>,
    title: String,
    client: ClientSettings,
    session: Option<Session>,
    renderer: Renderer,
    game: Box<dyn Game>,
    /// Cámara que se usa cuando no hay partida (solo se ve el cielo).
    idle_camera: Camera,
    last_frame: Instant,
    frames: u32,
    escape: bool,
    session_lost: Option<String>,
    shown_title: String,
    /// Desarrollo: guardar un frame y salir (KURAVOXEL_SCREENSHOT).
    screenshot: Option<PathBuf>,
    /// Desarrollo: clics pendientes de inyectar, como (x, y) en píxeles de ventana, y el
    /// frame en el que toca el siguiente (KURAVOXEL_UI_CLICKS="x,y;x,y").
    script: std::collections::VecDeque<(f64, f64)>,
    script_frame: u32,
    exit: bool,
    /// Ritmo de frames: cuándo toca el siguiente y si la ventana tiene el foco o no se ve.
    next_frame: Instant,
    focused: bool,
    hidden: bool,
    /// Frecuencia del monitor de la ventana (se vuelve a mirar al moverla y de vez en cuando).
    display_hz: Option<u32>,
    extra: Vec<ExtraWindow>,
    /// Ventanas extra que abrir en cuanto se pueda (hace falta el bucle de eventos).
    to_open: Vec<(WindowKey, WindowSpec)>,
    /// Se ha pedido un frame que aún no se ha dibujado (puede llegar por cualquier ventana).
    frame_requested: bool,
}

impl State {
    fn new(event_loop: &ActiveEventLoop, config: AppConfig, game: Box<dyn Game>) -> anyhow::Result<Self> {
        let screenshot = std::env::var_os("KURAVOXEL_SCREENSHOT").map(PathBuf::from);
        // Desarrollo: KURAVOXEL_WINDOW_SIZE="1920x1080" (píxeles lógicos).
        let (width, height) = std::env::var("KURAVOXEL_WINDOW_SIZE")
            .ok()
            .and_then(|spec| {
                let (w, h) = spec.split_once('x')?;
                Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
            })
            .unwrap_or((1280.0, 720.0));
        let attributes = Window::default_attributes()
            .with_title(config.title.clone())
            .with_inner_size(LogicalSize::new(width, height));
        let window = Arc::new(event_loop.create_window(attributes)?);

        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            &RenderSettings {
                far_distance: 32,
                vsync: config.client.vsync,
                shadows: config.client.shadows,
                allow_screenshots: screenshot.is_some(),
            },
        ))?;

        let now = Instant::now();
        let mut state = Self {
            window,
            title: config.title,
            client: config.client,
            session: None,
            renderer,
            game,
            idle_camera: Camera::new(glam::DVec3::new(0.0, 64.0, 0.0)),
            last_frame: now,
            frames: 0,
            escape: false,
            session_lost: None,
            shown_title: String::new(),
            screenshot,
            script: std::env::var("KURAVOXEL_UI_CLICKS")
                .map(|spec| parse_clicks(&spec))
                .unwrap_or_default(),
            script_frame: MENU_SCREENSHOT_FRAME,
            exit: false,
            next_frame: now,
            focused: true,
            hidden: false,
            display_hz: None,
            extra: Vec::new(),
            to_open: Vec::new(),
            frame_requested: false,
        };
        state.display_hz = state.read_display_rate();
        state.call_game(true);
        Ok(state)
    }

    /// Llama al juego y ejecuta lo que pida.
    fn call_game(&mut self, init: bool) {
        let commands = {
            let mut ctx = Context::new(self.session.as_ref().map(Session::info));
            if let Some(session) = self.session.as_mut() {
                ctx.session_events = session.take_events();
            }
            ctx.escape = std::mem::take(&mut self.escape);
            ctx.session_lost = self.session_lost.take();
            if init {
                let host = Host { gpu: self.renderer.gpu(), window: &self.window };
                self.game.init(&mut ctx, &host);
            } else {
                self.game.update(&mut ctx);
            }
            ctx.take_commands()
        };
        for command in commands {
            match command {
                Command::StartSession(config) => {
                    self.end_session();
                    match Session::start(config) {
                        Ok(mut session) => {
                            session.apply_settings(&self.client);
                            self.session = Some(session);
                        }
                        Err(error) => {
                            log::error!("could not start the session: {error:#}");
                            self.session_lost = Some(format!("{error:#}"));
                        }
                    }
                }
                Command::EndSession => self.end_session(),
                Command::Exit => self.exit = true,
                Command::Configure(settings) => {
                    self.client = settings;
                    self.renderer.set_vsync(settings.vsync);
                    self.renderer.set_shadow_quality(settings.shadows);
                    if let Some(session) = self.session.as_mut() {
                        session.apply_settings(&settings);
                    }
                }
                Command::SelectBlock(block) => {
                    if let Some(session) = self.session.as_mut() {
                        session.select_block(block);
                    }
                }
                Command::DropItem(item) => {
                    if let Some(session) = self.session.as_mut() {
                        session.drop_item(item);
                    }
                }
                Command::GrabMouse(grab) => {
                    if let Some(session) = self.session.as_mut() {
                        session.set_grab(&self.window, grab);
                    }
                }
                Command::ServerCommand(line) => {
                    if let Some(session) = self.session.as_mut() {
                        session.send_command(line);
                    }
                }
                Command::SetMoveMode(mode) => {
                    if let Some(session) = self.session.as_mut() {
                        session.set_move_mode(mode);
                    }
                }
                Command::ZoomCamera(steps) => {
                    if let Some(session) = self.session.as_mut() {
                        session.zoom_camera(steps);
                    }
                }
                Command::SetCameraDistance(meters) => {
                    if let Some(session) = self.session.as_mut() {
                        session.set_camera_distance(meters);
                    }
                }
                Command::SetTool(reach) => {
                    if let Some(session) = self.session.as_mut() {
                        session.set_tool(reach);
                    }
                }
                Command::SetOverlay(overlay) => {
                    if let Some(session) = self.session.as_mut() {
                        session.set_overlay(overlay);
                    }
                }
                Command::Prefab(op) => {
                    if let Some(session) = self.session.as_mut() {
                        session.prefab_op(op);
                    }
                }
                Command::SetTime { hours, day_length } => {
                    if let Some(session) = self.session.as_mut() {
                        session.set_time(hours, day_length);
                    }
                }
                Command::SetEnvironment(environment) => self.renderer.set_environment(environment),
                Command::SetSceneRegion(region) => self.renderer.set_scene_region(region),
                Command::Terrain(op) => {
                    if let Some(session) = self.session.as_mut() {
                        session.terrain_op(op);
                    }
                }
                Command::OpenWindow(key, spec) => match self.extra.iter().find(|extra| extra.key == key) {
                    Some(extra) => extra.window.focus_window(),
                    None => self.to_open.push((key, spec)),
                },
                Command::CloseWindow(key) => self.close_window(key),
            }
        }
    }

    /// Abre las ventanas extra pedidas.
    fn open_windows(&mut self, event_loop: &ActiveEventLoop) {
        for (key, spec) in std::mem::take(&mut self.to_open) {
            if self.extra.iter().any(|extra| extra.key == key) {
                continue;
            }
            let attributes = Window::default_attributes()
                .with_title(spec.title.clone())
                .with_inner_size(LogicalSize::new(spec.size[0], spec.size[1]));
            let opened = event_loop
                .create_window(attributes)
                .map_err(anyhow::Error::from)
                .and_then(|window| {
                    let window = Arc::new(window);
                    let surface = self.renderer.create_window_surface(window.clone())?;
                    Ok(ExtraWindow { key, window, surface, focused: true, hidden: false })
                });
            match opened {
                Ok(extra) => {
                    let host = Host { gpu: self.renderer.gpu_for(&extra.surface), window: &extra.window };
                    self.game.window_opened(key, &host);
                    self.extra.push(extra);
                }
                Err(error) => log::error!("could not open the window \"{}\": {error:#}", spec.title),
            }
        }
    }

    fn close_window(&mut self, key: WindowKey) {
        let before = self.extra.len();
        self.extra.retain(|extra| extra.key != key);
        if self.extra.len() != before {
            self.game.window_closed(key);
        }
    }

    /// Evento de una ventana extra.
    fn extra_event(&mut self, id: WindowId, event: WindowEvent) {
        let Some(extra) = self.extra.iter_mut().find(|extra| extra.window.id() == id) else { return };
        let key = extra.key;
        match &event {
            WindowEvent::CloseRequested => {
                self.close_window(key);
                return;
            }
            WindowEvent::Resized(size) => {
                extra.hidden = size.width == 0 || size.height == 0;
                extra.surface.resize(&self.renderer, size.width, size.height);
            }
            WindowEvent::Focused(focused) => extra.focused = *focused,
            WindowEvent::Occluded(occluded) => extra.hidden = *occluded,
            _ => {}
        }
        self.game.extra_window_event(key, &event);
    }

    fn end_session(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.release_input(&self.window);
            // Al soltarla, el servidor integrado guarda y se detiene.
            drop(session);
            self.renderer.clear_world();
        }
    }

    /// Desarrollo: inyecta el siguiente clic del guion como si viniera del ratón, por el
    /// mismo camino que la entrada real (interfaz primero, partida después).
    fn run_script(&mut self) {
        if self.frames < self.script_frame {
            return;
        }
        let Some((x, y)) = self.script.pop_front() else { return };
        self.script_frame = self.frames + SCRIPT_STEP_FRAMES;
        let device_id = DeviceId::dummy();
        let position = winit::dpi::PhysicalPosition::new(x, y);
        self.window_event(WindowEvent::CursorMoved { device_id, position });
        for state in [ElementState::Pressed, ElementState::Released] {
            self.window_event(WindowEvent::MouseInput { device_id, state, button: MouseButton::Left });
        }
    }

    fn window_event(&mut self, event: WindowEvent) {
        match &event {
            WindowEvent::CloseRequested => self.exit = true,
            WindowEvent::Resized(size) => {
                // Minimizada, la ventana pasa a medir 0 × 0 (Windows).
                self.hidden = size.width == 0 || size.height == 0;
                self.renderer.resize(size.width, size.height);
            }
            WindowEvent::Focused(focused) => self.focused = *focused,
            WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.display_hz = self.read_display_rate();
            }
            WindowEvent::Occluded(occluded) => self.hidden = *occluded,
            _ => {}
        }

        // El juego (su interfaz) va primero; si se queda el evento, la partida no lo ve.
        if self.game.window_event(&event) {
            return;
        }
        let Some(session) = self.session.as_mut() else { return };

        // Escape: primero suelta el ratón; con el ratón libre, es "atrás" para el juego.
        if let WindowEvent::KeyboardInput {
            event:
                KeyEvent {
                    physical_key: PhysicalKey::Code(KeyCode::Escape),
                    state: ElementState::Pressed,
                    repeat: false,
                    ..
                },
            ..
        } = &event
        {
            if session.is_grabbed() {
                session.set_grab(&self.window, false);
            } else {
                self.escape = true;
            }
            return;
        }
        session.window_event(&self.window, &event);
    }

    /// Tiempo mínimo entre dos frames ahora mismo; `None` = sin límite.
    fn frame_period(&self) -> Option<Duration> {
        // Las capturas de desarrollo no esperan: la ventana puede no tener el foco.
        let background = self.client.background_fps;
        // Con una ventana extra a la vista o con el foco, cuenta como si fuera la principal.
        let hidden = self.hidden && self.extra.iter().all(|extra| extra.hidden);
        let focused = self.focused || self.extra.iter().any(|extra| extra.focused);
        let rate = if background > 0 && self.screenshot.is_none() && hidden {
            Some((background / 2).max(1))
        } else if background > 0 && self.screenshot.is_none() && (!focused || self.game.idle()) {
            Some(background)
        } else {
            None
        };
        let limit = match self.client.max_fps {
            FrameLimit::Unlimited => None,
            FrameLimit::Fps(fps) => Some(fps.max(1)),
            FrameLimit::Display => self.display_hz,
        };
        let fps = match (rate, limit) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }?;
        Some(Duration::from_secs_f64(1.0 / fps as f64))
    }

    /// Frecuencia del monitor en el que está la ventana, redondeada.
    fn read_display_rate(&self) -> Option<u32> {
        let millihertz = self.window.current_monitor()?.refresh_rate_millihertz()?;
        Some(((millihertz + 500) / 1000).max(1))
    }

    /// Decide si toca dibujar ya o hasta cuándo puede dormir el bucle.
    fn pace(&mut self, event_loop: &ActiveEventLoop) {
        self.open_windows(event_loop);
        let now = Instant::now();
        match self.frame_period() {
            Some(_) if now < self.next_frame => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
            }
            _ => {
                event_loop.set_control_flow(ControlFlow::Poll);
                self.frame_requested = true;
                self.window.request_redraw();
                // Con la ventana principal minimizada puede no llegar su aviso: el frame
                // lo dispara entonces una ventana extra.
                if self.hidden {
                    for extra in &self.extra {
                        extra.window.request_redraw();
                    }
                }
            }
        }
    }

    fn frame(&mut self) {
        self.frame_requested = false;
        let now = Instant::now();
        // El siguiente frame, a un periodo de este; si vamos tarde, no se acumula deuda.
        if let Some(period) = self.frame_period() {
            self.next_frame = (self.next_frame + period).max(now);
        }
        if self.frames % 600 == 0 {
            self.display_hz = self.read_display_rate();
        }
        // Tope al delta para que un tirón no teletransporte la cámara.
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.frames += 1;

        if self.session.as_ref().is_some_and(|session| session.is_disconnected()) {
            self.end_session();
            self.session_lost = Some("disconnected from the server".into());
        }

        // El juego ve la partida tal como estaba al empezar el frame: la captura de
        // desarrollo espera a que su interfaz ya sepa que el mundo ha cargado.
        let loaded_before = self.session.as_ref().is_some_and(Session::is_loaded);

        self.run_script();
        self.call_game(false);

        // Con una pantalla de interfaz encima, la partida sigue viva pero sin entrada.
        if self.game.wants_input() {
            if let Some(session) = self.session.as_mut() {
                if session.is_grabbed() {
                    session.release_input(&self.window);
                }
            }
        }

        let (camera, scene) = match self.session.as_mut() {
            Some(session) => {
                session.update(&mut self.renderer, now, dt);
                (session.camera().clone(), session.scene_view())
            }
            None => (self.idle_camera.clone(), SceneView::empty(self.idle_camera.pos)),
        };

        // Captura de desarrollo: cuando el mundo ha cargado o, sin partida, tras unos frames.
        let ready = self.script.is_empty()
            && match &self.session {
                Some(session) => loaded_before && session.is_loaded(),
                None => self.frames >= self.script_frame + SCRIPT_STEP_FRAMES,
            };
        if ready {
            if let Some(path) = self.screenshot.take() {
                // Cada ventana extra, junto a la principal: `captura-<ventana>.ppm`.
                for extra in &mut self.extra {
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                    let file = format!("{stem}-window{}.ppm", extra.key);
                    extra.surface.request_screenshot(path.with_file_name(file));
                }
                self.renderer.request_screenshot(path);
                self.exit = true;
            }
        }

        let game = &mut self.game;
        let status = self.renderer.render(&camera, &scene, &mut |view, target| {
            game.draw_overlay(view, target)
        });
        if status == FrameStatus::Reconfigure {
            self.renderer.reconfigure();
        }
        for extra in self.extra.iter_mut().filter(|extra| !extra.hidden) {
            let key = extra.key;
            let game = &mut self.game;
            extra.surface.render(&self.renderer, EXTRA_CLEAR, &mut |view, target| game.draw_window(key, view, target));
        }

        let title = match self.session.as_ref().map(|session| session.status_line()) {
            Some(status) if !status.is_empty() => format!("{} | {status}", self.title),
            _ => self.title.clone(),
        };
        if title != self.shown_title {
            self.window.set_title(&title);
            self.shown_title = title;
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some((config, game)) = self.pending.take() else { return };
        match State::new(event_loop, config, game) {
            Ok(state) => self.state = Some(state),
            Err(error) => {
                log::error!("failed to start: {error:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else { return };
        let main = id == state.window.id();
        if matches!(event, WindowEvent::RedrawRequested) {
            // Un frame por cada uno pedido, llegue el aviso por la ventana que llegue.
            if main || state.frame_requested {
                state.frame();
            }
        } else if main {
            state.window_event(event);
        } else {
            state.extra_event(id, event);
        }
        if state.exit {
            event_loop.exit();
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let Some(state) = self.state.as_mut() else { return };
        if let (DeviceEvent::MouseMotion { delta }, Some(session)) = (event, state.session.as_mut()) {
            session.mouse_motion(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(state) = self.state.as_mut() {
            state.pace(event_loop);
        }
    }
}

/// Interpreta `"x,y;x,y"`; los puntos mal escritos se ignoran.
fn parse_clicks(spec: &str) -> std::collections::VecDeque<(f64, f64)> {
    spec.split(';')
        .filter_map(|point| {
            let (x, y) = point.split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect()
}
