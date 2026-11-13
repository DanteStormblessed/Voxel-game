//! Ventana y bucle principal del motor.
//!
//! El motor es dueño de la ventana, el renderer y la partida en curso (si hay). El juego
//! implementa [`Game`] y decide qué pasa: cuándo se entra en un mundo, cuándo se sale y
//! qué interfaz se ve encima. Las órdenes las da a través de [`Context`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use kuravoxel_common::block::Block;
use kuravoxel_common::content::ItemId;
use kuravoxel_common::physics::MoveMode;

use crate::camera::Camera;
use crate::render::{FrameStatus, Gpu, OverlayTarget, RenderSettings, Renderer, SceneView};
use crate::session::{ClientSettings, Session, SessionConfig, SessionEvent, SessionInfo};

/// Frames que se esperan antes de la captura de desarrollo cuando no hay partida (da
/// tiempo a que la interfaz cargue fuentes y termine su primera maquetación).
const MENU_SCREENSHOT_FRAME: u32 = 30;
/// Frames entre dos clics del guion de desarrollo (`KURAVOXEL_UI_CLICKS`).
const SCRIPT_STEP_FRAMES: u32 = 12;

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

    /// Cada frame, con la escena ya dibujada en `view`: aquí se pinta la interfaz.
    fn draw_overlay(&mut self, _view: &wgpu::TextureView, _target: OverlayTarget) {}
}

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
}

impl State {
    fn new(event_loop: &ActiveEventLoop, config: AppConfig, game: Box<dyn Game>) -> anyhow::Result<Self> {
        let screenshot = std::env::var_os("KURAVOXEL_SCREENSHOT").map(PathBuf::from);
        let attributes = Window::default_attributes()
            .with_title(config.title.clone())
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(event_loop.create_window(attributes)?);

        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            &RenderSettings {
                far_distance: 32,
                vsync: config.client.vsync,
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
        };
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
            }
        }
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
            WindowEvent::Resized(size) => self.renderer.resize(size.width, size.height),
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

    fn frame(&mut self) {
        let now = Instant::now();
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else { return };
        if matches!(event, WindowEvent::RedrawRequested) {
            state.frame();
        } else {
            state.window_event(event);
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

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
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
