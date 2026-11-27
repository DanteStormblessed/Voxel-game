//! Una partida: conexión con un servidor, copia local del mundo, jugador y edición.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{DVec3, IVec3, Vec3};
use kuravoxel_common::block::Block;
use kuravoxel_common::content::ContentData;
use kuravoxel_common::chunk::{CHUNK_SIZE, CHUNK_SIZE_I};
use kuravoxel_common::physics::MoveMode;
use kuravoxel_common::shape::{BlockTable, Collision};
use kuravoxel_common::prefab::{Prefab, PrefabInfo};
use kuravoxel_common::protocol::{
    ClientMsg, DropId, PlayerId, PrefabOp, ServerMsg, TerrainOp, TerrainStatus, WorldTime, PROTOCOL_VERSION, REACH,
    TICK_RATE,
    VERTICAL_RADIUS,
};
use kuravoxel_common::content::ItemId;
use kuravoxel_server::{ServerConfig, ServerHandle};
use rustc_hash::FxHashMap;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window};

use crate::camera::{Camera, Input, MOUSE_SENSITIVITY};
use crate::mesher;
use crate::player::{Player, ViewKind, MAP_MIN, THIRD_PERSON_MAX};
use crate::net::Connection;
use crate::render::{Renderer, SceneView, ShadowQuality};
use crate::world::{World, WorldEvent};

/// Rapidez con la que los demás jugadores se acercan a su última posición conocida.
const REMOTE_SMOOTHING: f32 = 14.0;
/// Silencio del servidor que da por terminada la carga inicial.
const INITIAL_LOAD_QUIET: Duration = Duration::from_millis(400);
/// A partir de esta duración, un hueco en el terreno cercano se avisa en el registro.
const NOTICEABLE_GAP: Duration = Duration::from_millis(250);

/// Dónde está el servidor de la partida.
pub enum SessionMode {
    /// Servidor integrado sobre un mundo local; con `listen`, además abierto a la red.
    Host { world_dir: PathBuf, listen: Option<SocketAddr> },
    /// Servidor remoto.
    Join(SocketAddr),
}

pub struct SessionConfig {
    pub mode: SessionMode,
    pub player_name: String,
    /// Distancia de voxeles reales pedida, en chunks.
    pub view_distance: u8,
    /// Distancia total con terreno lejano, en chunks.
    pub far_distance: u8,
    /// Semilla para un mundo nuevo (servidor integrado).
    pub seed: u32,
    /// Carpeta de paquetes de contenido (servidor integrado).
    pub content_dir: PathBuf,
}

/// Cuántos frames por segundo dibuja el motor como mucho.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FrameLimit {
    /// Los que muestra el monitor de la ventana (los de más nunca llegan a verse).
    #[default]
    Display,
    Fps(u32),
    /// Sin límite: la GPU o la CPU marcan el ritmo (para medir).
    Unlimited,
}

/// Ajustes del cliente que pueden cambiar con el juego en marcha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClientSettings {
    pub vsync: bool,
    pub max_fps: FrameLimit,
    /// Tope mientras no se juega (ventana sin foco o el juego en pausa); con la ventana
    /// minimizada u oculta, la mitad. 0 = sin rebaja.
    pub background_fps: u32,
    /// Campo de visión vertical, en grados.
    pub fov_degrees: f32,
    /// Multiplicador de la sensibilidad del ratón (1 = la normal).
    pub mouse_sensitivity: f32,
    pub shadows: ShadowQuality,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            vsync: true,
            max_fps: FrameLimit::Display,
            background_fps: 30,
            fov_degrees: 70.0,
            mouse_sensitivity: 1.0,
            shadows: ShadowQuality::default(),
        }
    }
}

/// Lo que el juego puede saber de la partida en curso. Es una copia que el motor hace
/// una vez por frame: el juego no toca la partida, da órdenes a través de `Context`.
#[derive(Clone, Default)]
pub struct SessionInfo {
    /// Contenido activo (bloques con su nombre y color); cambia al recargarse.
    pub content: Arc<ContentData>,
    /// Bloque que se coloca con el clic derecho (`Block::AIR` = ninguno).
    pub selected: Block,
    /// Posición de los ojos del jugador y hacia dónde mira la cámara (radianes).
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// Cómo se mueve el jugador (a pie, volando o atravesando bloques).
    pub move_mode: MoveMode,
    /// Desde dónde se ve: primera persona, tercera o vista de mapa.
    pub view: ViewKind,
    /// Distancia de cámara pedida, en metros (0 = primera persona).
    pub camera_distance: f32,
    /// Voxel al que se apunta, si hay alguno al alcance.
    pub target: Option<[i32; 3]>,
    /// Hueco justo antes de ese voxel: donde se colocaría un bloque.
    pub target_before: Option<[i32; 3]>,
    /// Rayo de la mira: de dónde sale y hacia dónde (unitario). Con él el juego sabe a
    /// qué apunta el jugador aunque no sea un bloque (una cara de su selección).
    pub aim_origin: [f64; 3],
    pub aim_dir: [f32; 3],
    /// Posición de la cámara y lo necesario para saber dónde cae en pantalla un punto
    /// del mundo (`project`): la matriz de vista y el origen al que es relativa.
    pub camera: [f64; 3],
    pub view_proj: [[f32; 4]; 4],
    pub view_origin: [f64; 3],
    /// Bloque (estado) del voxel al que se apunta.
    pub target_block: Option<Block>,
    /// El ratón está capturado: el jugador está mirando y editando.
    pub grabbed: bool,
    /// Esta partida aloja el servidor (puede usar sus comandos de consola).
    pub hosting: bool,
    /// La carga inicial del mundo ha terminado.
    pub loaded: bool,
    /// Línea de estado (fps, contadores), actualizada dos veces por segundo.
    pub status: Arc<str>,
    /// Hora del mundo, según el servidor.
    pub time: WorldTime,
}

impl SessionInfo {
    /// Dónde se ve en pantalla un punto del mundo: (x, y) de 0 a 1 desde la esquina
    /// superior izquierda. `None` si queda detrás de la cámara o fuera de la imagen.
    pub fn project(&self, point: [f64; 3]) -> Option<[f32; 2]> {
        let rel = (DVec3::from_array(point) - DVec3::from_array(self.view_origin)).as_vec3();
        let clip = glam::Mat4::from_cols_array_2d(&self.view_proj) * rel.extend(1.0);
        if clip.w <= 0.01 {
            return None;
        }
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        (x.abs() <= 1.0 && y.abs() <= 1.0).then_some([(x + 1.0) * 0.5, (1.0 - y) * 0.5])
    }
}

/// Algo que ha pasado en la partida y que al juego le puede interesar.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    /// El jugador ha copiado el bloque al que apuntaba (clic central).
    BlockPicked(Block),
    /// El jugador ha recogido un ítem suelto.
    PickedUp(ItemId),
    /// Respuesta del servidor a un comando de consola.
    CommandOutput { text: String, ok: bool },
    /// Clic con una herramienta activa (`Session::set_tool`): no edita nada, el juego
    /// decide qué hace. Lleva el voxel apuntado y el hueco anterior, si hay alguno.
    ToolClick { button: ToolButton, hit: Option<[i32; 3]>, before: Option<[i32; 3]> },
    /// Se ha soltado un botón con una herramienta activa (fin de un arrastre).
    ToolRelease { button: ToolButton },
    /// Resultado de una orden de construcción (`Session::prefab_op`).
    PrefabResult { text: String, ok: bool },
    /// El servidor ha dejado esto en el portapapeles del jugador.
    Clipboard { prefab: Arc<Prefab>, name: String },
    /// Prefabs guardados en el servidor.
    PrefabList(Vec<PrefabInfo>),
    /// Estado del generador de terreno (recarga, cambios esperando, perfiles). Solo le
    /// llega al anfitrión.
    TerrainStatus(TerrainStatus),
}

/// Botón del ratón de un `ToolClick`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolButton {
    Primary,
    Secondary,
    Middle,
}

impl ToolButton {
    fn from_mouse(button: MouseButton) -> Option<ToolButton> {
        match button {
            MouseButton::Left => Some(ToolButton::Primary),
            MouseButton::Right => Some(ToolButton::Secondary),
            MouseButton::Middle => Some(ToolButton::Middle),
            _ => None,
        }
    }
}

/// Caja que se dibuja solo con sus aristas, de `min` a `max` en metros del mundo. Puede
/// ser plana (una cara: `min` y `max` iguales en un eje).
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub color: [f32; 3],
    /// Grosor relativo de las aristas (1 = el normal).
    pub weight: f32,
    /// Opacidad del relleno de sus caras, con rejilla de bloques (0 = solo aristas).
    pub fill: f32,
}

/// Vista previa de un prefab: sus voxeles visibles como cubos algo encogidos.
#[derive(Clone, Debug)]
pub struct Ghost {
    pub prefab: Arc<Prefab>,
    /// Voxel del mundo donde cae su esquina mínima.
    pub origin: [i32; 3],
}

/// Marcas que el juego pide dibujar sobre el mundo (una selección, lo que se va a
/// pegar). El motor no sabe qué significan.
#[derive(Clone, Debug, Default)]
pub struct Overlay {
    pub outlines: Vec<Outline>,
    pub ghost: Option<Ghost>,
}

impl PartialEq for Overlay {
    fn eq(&self, other: &Self) -> bool {
        let same_ghost = match (&self.ghost, &other.ghost) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(&a.prefab, &b.prefab) && a.origin == b.origin,
            _ => false,
        };
        same_ghost && self.outlines == other.outlines
    }
}

/// Cubos que se dibujan como mucho en una vista previa; el resto se reparte salteado.
const MAX_GHOST_BOXES: usize = 12_000;
/// Lado de cada cubo de la vista previa. Un pelo más que el voxel: los cubos vecinos se
/// unen sin costuras y, si se coloca sobre bloques iguales, no parpadea contra ellos.
const GHOST_BOX: f32 = 1.004;
/// Cuánto se aclara la vista previa hacia el blanco, para distinguirla de lo construido.
const GHOST_TINT: f32 = 0.1;

/// Voxeles visibles de un prefab (los que tocan aire o el borde) con su color.
fn ghost_boxes(prefab: &Prefab, content: &ContentData) -> Vec<(IVec3, [f32; 3])> {
    let colors: Vec<[f32; 3]> = prefab
        .resolve(content)
        .into_iter()
        .map(|block| match block.and_then(|block| content.block(block)) {
            // De sRGB a lineal, que es como pinta el renderer de cajas.
            Some(def) => def.color.map(|c| {
                let linear = (c as f32 / 255.0).powf(2.2);
                linear + (1.0 - linear) * GHOST_TINT
            }),
            None => [1.0, 0.0, 1.0],
        })
        .collect();
    let size = prefab.size();
    let mut boxes = Vec::new();
    for y in 0..size.y {
        for z in 0..size.z {
            for x in 0..size.x {
                let pos = IVec3::new(x, y, z);
                let index = prefab.get(pos);
                let exposed = || IVec3::AXES.iter().any(|&axis| prefab.get(pos + axis) == 0 || prefab.get(pos - axis) == 0);
                if index != 0 && exposed() {
                    boxes.push((pos, colors[index as usize]));
                }
            }
        }
    }
    if boxes.len() > MAX_GHOST_BOXES {
        let step = boxes.len().div_ceil(MAX_GHOST_BOXES);
        boxes = boxes.into_iter().step_by(step).collect();
    }
    boxes
}

/// Las 12 aristas de la caja `min..max` como barras finas.
fn push_edges(boxes: &mut Vec<(DVec3, Vec3, [f32; 3])>, min: DVec3, max: DVec3, thickness: f32, color: [f32; 3]) {
    let center = (min + max) * 0.5;
    let extent = (max - min).as_vec3();
    for axis in 0..3 {
        let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
        let mut size = Vec3::splat(thickness);
        size[axis] = extent[axis] + thickness;
        for (sa, sb) in [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)] {
            let mut offset = DVec3::ZERO;
            offset[a] = sa * (extent[a] as f64 + 0.004);
            offset[b] = sb * (extent[b] as f64 + 0.004);
            boxes.push((center + offset, size, color));
        }
    }
}

/// Otro jugador, tal como lo ve este cliente.
/// Un ítem suelto, tal como se dibuja.
struct ShownDrop {
    item: ItemId,
    /// Última posición recibida del servidor.
    target: DVec3,
    /// Posición dibujada, suavizada hacia `target`.
    shown: DVec3,
    /// Desfase del giro y del vaivén, para que no se muevan todos a la vez.
    phase: f32,
}

/// Altura a la que flota un ítem suelto sobre su base y cuánto sube y baja (m).
const DROP_FLOAT: f64 = 0.12;
const DROP_BOB: f64 = 0.06;
/// Vueltas por segundo de un ítem suelto (en radianes) y vaivenes por segundo.
const DROP_SPIN: f32 = 1.2;
const DROP_BOB_RATE: f32 = 2.4;

struct RemotePlayer {
    name: String,
    /// Última posición recibida del servidor.
    target: Option<(DVec3, f32)>,
    /// Posición que se dibuja, suavizada hacia `target`.
    shown: Option<(DVec3, f32)>,
}

/// Una partida en curso: la conexión con un servidor y todo lo que el cliente sabe del
/// mundo. Se crea al entrar en un mundo y se destruye al salir; el motor puede vivir sin
/// ninguna (por ejemplo, en un menú).
pub struct Session {
    connection: Connection,
    /// Servidor integrado, si esta sesión lo aloja.
    server: Option<ServerHandle>,
    world: World,
    player: Player,
    /// Punto de vista de este frame; lo coloca `player`.
    camera: Camera,
    input: Input,
    events: Vec<WorldEvent>,
    /// Avisos para el juego, hasta que los recoja.
    outbox: Vec<SessionEvent>,
    grabbed: bool,

    /// Identidad y distancias concedidas por el servidor; `None` hasta el `Welcome`.
    session: Option<(PlayerId, i32)>,
    players: FxHashMap<PlayerId, RemotePlayer>,
    drops: FxHashMap<DropId, ShownDrop>,
    /// Inicio de la partida: reloj de las animaciones.
    started: Instant,
    content: Arc<ContentData>,
    /// Bloque que se coloca con el clic derecho.
    selected: Block,
    /// Bloque al que se apunta y hueco anterior, recalculados cada frame.
    target: Option<(IVec3, IVec3)>,
    /// Herramienta activa: los clics no editan, se avisan al juego, y el rayo llega
    /// hasta esta distancia.
    tool_reach: Option<f64>,
    /// Marcas pedidas por el juego.
    overlay: Overlay,
    /// Ancho entre alto de la imagen, según el último frame.
    aspect: f32,
    /// Cubos de la vista previa en curso, para no recalcularlos cada frame.
    ghost_cache: Option<(Arc<Prefab>, Vec<(IVec3, [f32; 3])>)>,
    next_move: Instant,
    last_message: Instant,
    /// El servidor ha cerrado la conexión.
    disconnected: bool,

    stats_since: Instant,
    stats_frames: u32,
    /// Momento en que empezó la carga inicial; `None` cuando ya terminó.
    loading_since: Option<Instant>,
    /// Hueco en curso en el terreno cercano: cuándo empezó y el máximo de chunks que
    /// han llegado a faltar.
    gap: Option<(Instant, usize)>,
    /// Desarrollo: vista inicial fija.
    dev_camera: Option<[f64; 5]>,
    /// Línea de estado (fps, contadores) para el título de la ventana.
    title: Arc<str>,
    /// Radio de voxeles reales calculado en el último `update`.
    clip: f32,
    /// Hora del mundo: la da el servidor y entre aviso y aviso avanza sola.
    time: WorldTime,
}

impl Session {
    /// Entra en un mundo: arranca el servidor integrado (o conecta con uno remoto) y saluda.
    pub fn start(config: SessionConfig) -> anyhow::Result<Self> {
        // Desarrollo: KURAVOXEL_CAMERA="x,y,z,yaw,pitch" (ángulos en grados) fija la vista inicial.
        let dev_camera = std::env::var("KURAVOXEL_CAMERA").ok().and_then(|spec| {
            let v: Vec<f64> = spec.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            let parsed = <[f64; 5]>::try_from(v).ok();
            if parsed.is_none() {
                log::warn!("KURAVOXEL_CAMERA must be \"x,y,z,yaw,pitch\"");
            }
            parsed
        });

        // Jugar solo también pasa por un servidor: el integrado.
        let mut hosted = None;
        let connection = match config.mode {
            SessionMode::Host { world_dir, listen } => {
                let server = kuravoxel_server::start(ServerConfig {
                    world_dir,
                    content_dir: config.content_dir,
                    seed: config.seed,
                    // La vista de desarrollo salta a cualquier sitio: sin validar movimiento.
                    trust_movement: dev_camera.is_some(),
                    ..Default::default()
                })?;
                if let Some(addr) = listen {
                    kuravoxel_server::quic::listen(server.clone(), addr)?;
                }
                let connection = Connection::local(&server);
                hosted = Some(server);
                connection
            }
            SessionMode::Join(addr) => Connection::remote(addr)?,
        };
        connection.send(ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            name: config.player_name,
            view_distance: config.view_distance,
            far_distance: config.far_distance,
        });

        let now = Instant::now();
        Ok(Self {
            connection,
            server: hosted,
            world: World::new(),
            player: Player::new(),
            camera: Camera::new(DVec3::ZERO),
            input: Input::default(),
            events: Vec::new(),
            outbox: Vec::new(),
            grabbed: false,
            session: None,
            players: FxHashMap::default(),
            drops: FxHashMap::default(),
            started: now,
            content: Arc::default(),
            selected: Block::AIR,
            target: None,
            tool_reach: None,
            overlay: Overlay::default(),
            aspect: 16.0 / 9.0,
            ghost_cache: None,
            next_move: now,
            last_message: now,
            disconnected: false,
            stats_since: now,
            stats_frames: 0,
            loading_since: Some(now),
            gap: None,
            dev_camera,
            title: Arc::from(""),
            clip: 0.0,
            time: WorldTime::default(),
        })
    }

    pub fn set_grab(&mut self, window: &Window, grab: bool) {
        if grab {
            let grabbed = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
                .is_ok();
            window.set_cursor_visible(!grabbed);
            self.grabbed = grabbed;
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
            self.grabbed = false;
        }
    }

    fn on_key(&mut self, code: KeyCode, pressed: bool) {
        match code {
            KeyCode::KeyW => self.input.forward = pressed,
            KeyCode::KeyS => self.input.back = pressed,
            KeyCode::KeyA => self.input.left = pressed,
            KeyCode::KeyD => self.input.right = pressed,
            KeyCode::Space => self.input.up = pressed,
            KeyCode::ShiftLeft => self.input.down = pressed,
            KeyCode::ControlLeft => self.input.sprint = pressed,
            _ => {}
        }
    }

    fn block_exists(&self, block: Block) -> bool {
        self.content.state(block).is_some()
    }

    /// Elige el bloque que se coloca con el clic derecho; `Block::AIR` para no colocar
    /// nada. Qué bloques se ofrecen y cómo se eligen (una barra, un inventario) es cosa
    /// del juego. Un bloque que no existe se ignora.
    pub fn select_block(&mut self, block: Block) {
        if block == Block::AIR || self.block_exists(block) {
            self.selected = block;
        }
    }

    /// Tira un ítem hacia donde mira la cámara. Lo simula el servidor; si alguien lo
    /// recoge llega `SessionEvent::PickedUp`. Qué ítem y con qué tecla es cosa del juego.
    pub fn drop_item(&mut self, item: ItemId) {
        if self.content.item(item).is_some() {
            let dir = self.player.forward().to_array();
            self.connection.send(ClientMsg::DropItem { item: item.0, dir });
        }
    }

    /// Activa (con su alcance en metros) o desactiva el modo herramienta: con él, los
    /// clics no rompen ni colocan, llegan al juego como `SessionEvent::ToolClick`.
    pub fn set_tool(&mut self, reach: Option<f64>) {
        self.tool_reach = reach.map(|reach| reach.clamp(1.0, 256.0));
    }

    /// Marcas que se dibujan sobre el mundo hasta que se cambien.
    pub fn set_overlay(&mut self, overlay: Overlay) {
        self.overlay = overlay;
    }

    /// Envía una orden de construcción al servidor; el resultado llega como `SessionEvent`.
    pub fn prefab_op(&mut self, op: PrefabOp) {
        self.connection.send(ClientMsg::Prefab(op));
    }

    /// Pide al servidor cambiar la hora del mundo (0-24) o la duración del día en segundos
    /// reales (0 = parar el tiempo). Aquí se ve al instante; si el servidor no lo acepta
    /// (solo el anfitrión puede), su siguiente aviso de hora lo corrige.
    pub fn set_time(&mut self, hours: Option<f32>, day_length: Option<f32>) {
        self.time = WorldTime {
            hours: hours.unwrap_or(self.time.hours),
            day_length: day_length.unwrap_or(self.time.day_length),
            ..self.time
        }
        .sanitized();
        self.connection.send(ClientMsg::SetTime { hours, day_length });
    }

    /// Orden sobre el terreno (recarga, perfiles); la respuesta llega como
    /// `SessionEvent::TerrainStatus`.
    pub fn terrain_op(&mut self, op: TerrainOp) {
        self.connection.send(ClientMsg::Terrain(op));
    }

    /// Envía una línea de consola al servidor; la respuesta llega como `SessionEvent`.
    pub fn send_command(&mut self, line: String) {
        self.connection.send(ClientMsg::Command { line });
    }

    pub fn apply_settings(&mut self, settings: &ClientSettings) {
        self.camera.fov_y = settings.fov_degrees.clamp(30.0, 120.0).to_radians();
        self.player.sensitivity = MOUSE_SENSITIVITY * settings.mouse_sensitivity.clamp(0.05, 10.0);
    }

    /// Cambia cómo se mueve el jugador. Qué modos se ofrecen y con qué tecla es cosa
    /// del juego.
    pub fn set_move_mode(&mut self, mode: MoveMode) {
        self.player.mode = mode;
    }

    /// Aleja (`steps` > 0) o acerca la cámara: primera persona, tercera y vista de mapa.
    pub fn zoom_camera(&mut self, steps: i32) {
        self.player.zoom(steps);
    }

    /// Fija la distancia de cámara en metros (0 = primera persona).
    pub fn set_camera_distance(&mut self, meters: f32) {
        self.player.set_camera_distance(meters);
    }

    /// Copia para el juego del estado de la partida.
    pub fn info(&self) -> SessionInfo {
        SessionInfo {
            content: self.content.clone(),
            selected: self.selected,
            position: self.player.body.eye().to_array(),
            yaw: self.player.yaw,
            pitch: self.player.pitch,
            move_mode: self.player.mode,
            view: self.player.view(),
            camera_distance: self.player.camera_distance(),
            target: self.target.map(|(hit, _)| hit.to_array()),
            target_before: self.target.map(|(_, before)| before.to_array()),
            aim_origin: self.player.pivot(&self.world).to_array(),
            aim_dir: self.player.forward().to_array(),
            camera: self.camera.pos.to_array(),
            view_proj: self.camera.view_proj(self.aspect).to_cols_array_2d(),
            view_origin: (self.camera.chunk_pos().as_dvec3() * CHUNK_SIZE as f64).to_array(),
            target_block: self.target.and_then(|(hit, _)| self.world.block_at(hit)),
            grabbed: self.grabbed,
            hosting: self.server.is_some(),
            loaded: self.loading_since.is_none(),
            status: self.title.clone(),
            time: self.time,
        }
    }

    /// Avisos acumulados desde la última llamada.
    pub fn take_events(&mut self) -> Vec<SessionEvent> {
        std::mem::take(&mut self.outbox)
    }

    fn selected_name(&self) -> &str {
        self.content.block(self.selected).map_or("-", |block| block.name.as_str())
    }

    /// Clic con el ratón capturado: romper, colocar o copiar el bloque apuntado.
    fn on_click(&mut self, button: MouseButton) {
        if self.tool_reach.is_some() {
            let Some(button) = ToolButton::from_mouse(button) else { return };
            self.outbox.push(SessionEvent::ToolClick {
                button,
                hit: self.target.map(|(hit, _)| hit.to_array()),
                before: self.target.map(|(_, before)| before.to_array()),
            });
            return;
        }
        let Some((hit, before)) = self.target else { return };
        // Celdas que cambian: un objeto de varias celdas (una puerta) se pone y se quita
        // entero, igual que hará el servidor.
        let (pos, block, cells) = match button {
            MouseButton::Left => {
                let Some(current) = self.world.block_at(hit) else { return };
                let cells: Vec<(IVec3, Block)> = self
                    .content
                    .structure(current, hit)
                    .into_iter()
                    .filter(|&(cell, expected)| cell == hit || self.world.block_at(cell) == Some(expected))
                    .map(|(cell, _)| (cell, Block::AIR))
                    .collect();
                (hit, Block::AIR, cells)
            }
            MouseButton::Right if self.selected != Block::AIR => {
                // Los bloques orientables se giran según la cara y la mirada.
                let block = self.content.orient(self.selected, before - hit, self.player.forward());
                let cells = self.content.structure(block, before);
                // Todas sus celdas deben estar libres, y uno no se pone un bloque sólido
                // encima (el espectador no ocupa sitio; una flor no estorba).
                let solid = self.world.blocks().get(block).collision != Collision::Empty;
                let fits = cells.iter().all(|&(cell, _)| {
                    self.world.block_at(cell).is_some_and(|current| self.content.replaceable(current))
                        && !(solid && self.player.mode != MoveMode::Ghost && self.player.body.touches(cell))
                });
                if !fits {
                    return;
                }
                (before, block, cells)
            }
            MouseButton::Middle => {
                if let Some(block) = self.world.block_at(hit).filter(|&block| block != Block::AIR) {
                    // Se copia el bloque, no su orientación: la decide quien lo coloque.
                    let block = self.content.default_state(block);
                    self.selected = block;
                    self.outbox.push(SessionEvent::BlockPicked(block));
                }
                return;
            }
            _ => return,
        };
        // Predicción: el cambio se ve al instante; el servidor lo confirma o lo corrige.
        for (cell, block) in cells {
            self.world.set_block(cell, block);
        }
        self.connection.send(ClientMsg::SetBlock { pos: pos.to_array(), block: block.0 });
    }

    /// Aplica todo lo que ha llegado del servidor desde el frame anterior.
    fn receive(&mut self, renderer: &mut Renderer, now: Instant) {
        while let Ok(msg) = self.connection.incoming.try_recv() {
            // Las fotos de entidades llegan sin parar: no cuentan como carga de mundo.
            if !msg.is_unreliable() {
                self.last_message = now;
            }
            match msg {
                ServerMsg::Welcome { id, spawn, bounds, view_distance, far_distance } => {
                    log::info!(
                        "joined as player #{id}: voxels {view_distance}, far {far_distance}"
                    );
                    self.session = Some((id, view_distance as i32));
                    self.world.set_view_distance(view_distance as i32);
                    renderer.set_far_distance(far_distance as i32);
                    self.player.bounds = bounds;
                    self.player.set_eye(DVec3::from_array(spawn));
                    if let Some([x, y, z, yaw, pitch]) = self.dev_camera {
                        // La vista de desarrollo es una cámara fija en el aire.
                        self.player.mode = MoveMode::Ghost;
                        self.player.set_eye(DVec3::new(x, y, z));
                        self.player.yaw = (yaw as f32).to_radians();
                        self.player.pitch = (pitch as f32).to_radians();
                    }
                }
                ServerMsg::Disconnect { reason } => {
                    log::error!("disconnected from the server: {reason}");
                    self.disconnected = true;
                }
                ServerMsg::Content(content) => {
                    log::info!(
                        "content received: {} blocks, {} items",
                        content.block_count(),
                        content.item_count()
                    );
                    // Tablas de bloques para la física y el mallado, y la paleta de GPU
                    // que va con ellas.
                    let blocks = BlockTable::new(&content);
                    let (mesh_table, palette) = mesher::build_tables(&content, &blocks);
                    renderer.set_palette(&palette);
                    renderer.set_item_shapes(&content, &blocks, &mesh_table);
                    self.world.set_tables(Arc::new(blocks), Arc::new(mesh_table));
                    self.content = content;
                    self.ghost_cache = None;
                    // Si el bloque elegido ya no existe, ninguno: el juego elegirá otro.
                    if !self.block_exists(self.selected) {
                        self.selected = Block::AIR;
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
                ServerMsg::DropSpawned { id, item, pos } => {
                    let pos = DVec3::from_array(pos);
                    let phase = (id.wrapping_mul(2654435761) % 1000) as f32 / 1000.0 * std::f32::consts::TAU;
                    self.drops.insert(id, ShownDrop { item: ItemId(item), target: pos, shown: pos, phase });
                }
                ServerMsg::DropRemoved { id } => {
                    self.drops.remove(&id);
                }
                ServerMsg::PickedUp { item } => self.outbox.push(SessionEvent::PickedUp(ItemId(item))),
                ServerMsg::PlayerLeft { id } => {
                    if let Some(player) = self.players.remove(&id) {
                        log::info!("{} left", player.name);
                    }
                }
                ServerMsg::Entities { players, drops, .. } => {
                    for state in drops {
                        if let Some(drop) = self.drops.get_mut(&state.id) {
                            drop.target = DVec3::from_array(state.pos);
                        }
                    }
                    for state in players {
                        if let Some(player) = self.players.get_mut(&state.id) {
                            player.target = Some((DVec3::from_array(state.pos), state.yaw));
                        }
                    }
                }
                // El servidor no aceptó el movimiento: manda su posición.
                ServerMsg::Teleport { pos } => self.player.set_eye(DVec3::from_array(pos)),
                ServerMsg::CommandOutput { text, ok } => {
                    self.outbox.push(SessionEvent::CommandOutput { text, ok })
                }
                ServerMsg::PrefabResult { text, ok } => {
                    self.outbox.push(SessionEvent::PrefabResult { text, ok })
                }
                ServerMsg::Clipboard { prefab, name } => {
                    self.outbox.push(SessionEvent::Clipboard { prefab, name })
                }
                ServerMsg::PrefabList(list) => self.outbox.push(SessionEvent::PrefabList(list)),
                ServerMsg::Time(time) => self.time = time.sanitized(),
                ServerMsg::TerrainStatus(status) => self.outbox.push(SessionEvent::TerrainStatus(status)),
            }
        }
    }

    /// Avanza la partida un frame: red, movimiento, mundo y lo que hay que dibujar.
    /// No dibuja: deja el renderer listo para que el motor llame a `render`.
    pub fn update(&mut self, renderer: &mut Renderer, now: Instant, dt: f32) {
        // La hora avanza sola entre un aviso del servidor y el siguiente.
        self.time = self.time.advanced(dt);
        self.receive(renderer, now);

        let mut clip = 0.0;
        if let Some((_, view_distance)) = self.session {
            // El cliente mueve a su jugador al instante (predicción) y se lo cuenta al
            // servidor a ritmo de tick; el servidor valida y, si no cuadra, corrige.
            self.player.update(&mut self.input, dt, &self.world);
            self.player.place_camera(&mut self.camera, &self.world);
            if now >= self.next_move {
                self.next_move = now + Duration::from_secs(1) / TICK_RATE;
                self.connection.send(ClientMsg::Move {
                    pos: self.player.body.eye().to_array(),
                    yaw: self.player.body_yaw,
                    pitch: self.player.pitch,
                });
            }

            // Los chunks reales rodean al jugador, no a la cámara: camino de la vista de
            // mapa la cámara se va demasiado lejos y todo pasa a ser terreno lejano.
            let distance = self.player.shown_distance();
            let near = 1.0 - ((distance - THIRD_PERSON_MAX) / (MAP_MIN * 0.5 - THIRD_PERSON_MAX)).clamp(0.0, 1.0);
            clip = (view_distance * 32) as f32 * near;
        }

        self.world.update(self.player.chunk_pos(), &mut self.events);
        for event in self.events.drain(..) {
            match event {
                WorldEvent::Mesh { key, mesh } => renderer.upload_chunk(key, &mesh),
                WorldEvent::Unload { key } => renderer.remove_chunk(key),
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
        // Ítems sueltos: suavizados, flotando un poco sobre su base y girando.
        let time = (now - self.started).as_secs_f32();
        for drop in self.drops.values_mut() {
            drop.shown = drop.shown.lerp(drop.target, blend as f64);
        }
        renderer.set_drops(
            &self.camera,
            self.drops.values().map(|drop| {
                let bob = DROP_FLOAT + DROP_BOB * (time * DROP_BOB_RATE + drop.phase).sin() as f64;
                (drop.item, drop.shown + DVec3::new(0.0, bob, 0.0), time * DROP_SPIN + drop.phase)
            }),
        );

        // Marcador: contorno del bloque apuntado. El punto de mira es cosa de la interfaz
        // del juego, que lo pinta encima.
        // En tercera persona el rayo sale del mismo punto al que mira la cámara, así que
        // el punto de mira señala lo mismo que en primera. En la vista de mapa no se edita.
        let view = self.player.view();
        self.target = (view != ViewKind::Map)
            .then(|| {
                let reach = self.tool_reach.unwrap_or(REACH);
                self.world.raycast(self.player.pivot(&self.world), self.player.forward(), reach)
            })
            .flatten();
        let mut boxes = Vec::new();
        if view == ViewKind::Map {
            // Desde tan lejos el jugador no se distingue: un poste lo señala.
            let scale = self.player.shown_distance();
            let size = Vec3::new(scale * 0.008, scale * 0.06, scale * 0.008);
            let center = self.player.body.pos + DVec3::new(0.0, 3.0 + size.y as f64 * 0.5, 0.0);
            boxes.push((center, size, [1.0, 1.0, 1.0]));
            let head = Vec3::splat(scale * 0.03);
            let top = center + DVec3::new(0.0, (size.y + head.y) as f64 * 0.5, 0.0);
            boxes.push((top, head, [1.0, 0.12, 0.1]));
        }
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
        // Marcas del juego: contornos (más gruesos cuanto más lejos, para que se vean) y
        // la vista previa de lo que se va a pegar.
        // Van en un pase aparte, sin prueba de profundidad: se ven a través del terreno.
        self.aspect = renderer.aspect();
        let mut marks: Vec<(DVec3, Vec3, [f32; 4], bool)> = Vec::new();
        let mut edges = Vec::new();
        for outline in &self.overlay.outlines {
            let (min, max) = (DVec3::from_array(outline.min), DVec3::from_array(outline.max));
            let [r, g, b] = outline.color;
            if outline.fill > 0.0 {
                marks.push(((min + max) * 0.5, (max - min).as_vec3(), [r, g, b, outline.fill], true));
            }
            let nearest = self.camera.pos.clamp(min, max);
            let thickness =
                (self.camera.pos.distance(nearest) as f32 * 0.004).clamp(0.03, 0.5) * outline.weight;
            edges.clear();
            push_edges(&mut edges, min, max, thickness, outline.color);
            marks.extend(edges.iter().map(|&(center, size, _)| (center, size, [r, g, b, 0.95], false)));
        }
        renderer.set_marks(&self.camera, &marks);
        if let Some(ghost) = &self.overlay.ghost {
            if !self.ghost_cache.as_ref().is_some_and(|(prefab, _)| Arc::ptr_eq(prefab, &ghost.prefab)) {
                self.ghost_cache = Some((ghost.prefab.clone(), ghost_boxes(&ghost.prefab, &self.content)));
            }
            let origin = IVec3::from_array(ghost.origin);
            if let Some((_, cubes)) = &self.ghost_cache {
                boxes.extend(cubes.iter().map(|&(pos, color)| {
                    ((origin + pos).as_dvec3() + DVec3::splat(0.5), Vec3::splat(GHOST_BOX), color)
                }));
            }
        } else {
            self.ghost_cache = None;
        }
        renderer.set_entities(
            &self.camera,
            self.players
                .iter()
                .filter_map(|(id, player)| player.shown.map(|(pos, yaw)| (pos, yaw, *id)))
                // El propio jugador solo se ve cuando la cámara está fuera de su cabeza.
                .chain(self.player.draws_self().then(|| {
                    let id = self.session.map_or(0, |(id, _)| id);
                    (self.player.body.visual_eye(), self.player.body_yaw, id)
                })),
            &boxes,
        );

        self.clip = clip;
        self.track_loading(renderer, now);
        self.update_stats(renderer, now);
    }

    /// Registro de la carga del mundo.
    ///
    /// - La **carga inicial** se anuncia una vez (y dispara la captura de desarrollo). El
    ///   cliente no sabe qué le falta por recibir, así que la da por terminada cuando no
    ///   queda nada por mallar y el servidor lleva un momento sin enviar nada.
    /// - Después solo se avisa de lo que el jugador puede notar: que el terreno cercano
    ///   estuvo incompleto un rato (volando más rápido de lo que carga, por ejemplo).
    ///   Editar bloques o cargar los bordes al andar no genera ningún aviso.
    fn track_loading(&mut self, renderer: &Renderer, now: Instant) {
        let Some((_, view_distance)) = self.session else { return };

        if let Some(since) = self.loading_since {
            let quiet = now - self.last_message;
            let settled = self.world.is_idle()
                && self.world.stats().chunks > 0
                && quiet >= INITIAL_LOAD_QUIET;
            if !settled {
                return;
            }
            self.loading_since = None;
            let world = self.world.stats();
            let render = renderer.stats();
            log::info!(
                "world loaded in {:.2} s: {} chunks, {} far tiles, {} meshes, {} quads",
                (now - since).saturating_sub(quiet).as_secs_f32(),
                world.chunks,
                world.lod_tiles,
                render.chunks,
                render.quads,
            );
            return;
        }

        // Tras la carga inicial: ¿falta terreno donde el jugador lo vería?
        let missing = self.world.missing_near(self.player.chunk_pos(), view_distance);
        match (&mut self.gap, missing) {
            (None, 0) => {}
            (None, _) => self.gap = Some((now, missing)),
            (Some((_, worst)), 1..) => *worst = (*worst).max(missing),
            (Some((since, worst)), 0) => {
                let lasted = now - *since;
                let message = format!(
                    "nearby terrain was incomplete for {:.2} s (up to {worst} chunks missing)",
                    lasted.as_secs_f32()
                );
                // Un instante al cruzar un borde es normal; más ya se puede ver.
                if lasted >= NOTICEABLE_GAP {
                    log::warn!("{message}");
                } else {
                    log::debug!("{message}");
                }
                self.gap = None;
            }
        }
    }

    /// Estadísticas en el título de la ventana, dos veces por segundo.
    fn update_stats(&mut self, renderer: &Renderer, now: Instant) {
        self.stats_frames += 1;
        let elapsed = now - self.stats_since;
        if elapsed < Duration::from_millis(500) {
            return;
        }
        let fps = self.stats_frames as f32 / elapsed.as_secs_f32();
        let world = self.world.stats();
        let render = renderer.stats();
        let pos = self.player.body.eye();
        let title = format!(
            "{fps:.0} fps | block: {} | players {} | chunks {} (meshing {}) | far {} | \
             meshes {} visible {} | quads {} | draws {} + {} shadow | time {} | \
             pos {:.0} {:.0} {:.0}",
            self.selected_name(),
            self.players.len() + 1,
            world.chunks,
            world.meshing,
            world.lod_tiles,
            render.chunks,
            render.visible,
            render.quads,
            render.draw_calls,
            render.shadow_draw_calls,
            self.time.clock(),
            pos.x,
            pos.y,
            pos.z,
        );
        log::debug!("{title}");
        self.title = Arc::from(title);
        self.stats_since = now;
        self.stats_frames = 0;
    }
}

impl Session {
    /// Entrada de ventana mientras la partida tiene el foco (ninguna pantalla de interfaz
    /// por encima).
    pub fn window_event(&mut self, window: &Window, event: &WindowEvent) {
        match event {
            WindowEvent::Focused(false) => {
                self.set_grab(window, false);
                self.input = Input::default();
            }
            WindowEvent::MouseInput { state: ElementState::Released, button, .. } => {
                // Con una herramienta, soltar el botón también cuenta (termina un arrastre).
                if let (true, Some(_), Some(button)) = (self.grabbed, self.tool_reach, ToolButton::from_mouse(*button)) {
                    self.outbox.push(SessionEvent::ToolRelease { button });
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                // El primer clic captura el ratón; con el ratón capturado, edita.
                if self.grabbed {
                    self.on_click(*button);
                } else if *button == MouseButton::Left {
                    self.set_grab(window, true);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat: false,
                        ..
                    },
                ..
            } => self.on_key(*code, *state == ElementState::Pressed),
            _ => {}
        }
    }

    /// Movimiento relativo del ratón (solo cuenta con el ratón capturado).
    pub fn mouse_motion(&mut self, dx: f64, dy: f64) {
        if self.grabbed {
            self.input.mouse_dx += dx as f32;
            self.input.mouse_dy += dy as f32;
        }
    }

    /// Suelta el ratón y olvida las teclas pulsadas (al abrir un menú, por ejemplo).
    pub fn release_input(&mut self, window: &Window) {
        self.set_grab(window, false);
        self.input = Input::default();
    }

    pub fn is_grabbed(&self) -> bool {
        self.grabbed
    }

    pub fn camera(&self) -> &Camera {
        &self.camera
    }



    /// Qué parte del mundo se dibuja con voxeles reales y desde dónde se mide la bruma.
    ///
    /// Los voxeles ocupan un cilindro alrededor del jugador: `clip` metros en horizontal
    /// y, en vertical, las capas de chunks que el servidor envía con todos sus vecinos
    /// (las que se pueden mallar). Fuera de él, arriba, abajo o lejos, dibuja el terreno
    /// lejano: en una montaña, el valle de 300 m más abajo sigue viéndose.
    pub fn scene_view(&self) -> SceneView {
        // El servidor centra el envío en el chunk de los ojos.
        let eye_chunk = (self.player.body.eye().y / CHUNK_SIZE as f64).floor() as i32;
        let meshed = VERTICAL_RADIUS - 1;
        SceneView {
            clip: self.clip,
            voxel_band: [
                ((eye_chunk - meshed) * CHUNK_SIZE_I) as f64,
                ((eye_chunk + meshed + 1) * CHUNK_SIZE_I) as f64,
            ],
            focus: self.player.body.visual_eye(),
            time: self.time.hours,
            moon_age: self.time.moon_age(),
        }
    }

    /// Línea de estado para el título de la ventana.
    pub fn status_line(&self) -> &str {
        &self.title
    }

    /// `true` cuando la carga inicial del mundo ha terminado.
    pub fn is_loaded(&self) -> bool {
        self.loading_since.is_none()
    }

    /// El servidor cerró la conexión: la partida ya no sirve.
    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // El servidor integrado guarda el mundo antes de terminar.
        if let Some(server) = self.server.take() {
            server.shutdown();
        }
    }
}
