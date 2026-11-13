//! Tokikura sobre el motor: decide qué pantalla se ve, cuándo se entra o sale de un mundo
//! y qué hace cada tecla de interfaz.
//!
//! Las pantallas (`screens/`) no se conocen entre sí ni conocen al motor: aquí se recogen
//! sus peticiones, se les copia el estado que enseñan y se dan las órdenes al motor.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kuravoxel_client::app::{Context, Game, Host};
use kuravoxel_client::reexports::winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use kuravoxel_client::reexports::winit::keyboard::{KeyCode, PhysicalKey};
use kuravoxel_client::reexports::wgpu;
use kuravoxel_client::player::{ViewKind, THIRD_PERSON_MAX};
use kuravoxel_client::render::OverlayTarget;
use kuravoxel_client::session::{SessionConfig, SessionEvent, SessionInfo, SessionMode};
use kuravoxel_common::block::Block;
use kuravoxel_common::content::ContentData;
use kuravoxel_ui::{IcedLayer, Screen};

use crate::commands::{self, Command};
use crate::game_mode::GameMode;
use crate::hotbar::{self, Hotbar};
use crate::icons::Icons;
use crate::log_capture::LogCapture;
use crate::screens::console::{self, Console, Kind};
use crate::screens::hud::Hud;
use crate::screens::inventory::{self, Inventory};
use crate::screens::main_menu::{self, MainMenu};
use crate::screens::pause_menu::{self, PauseMenu};
use crate::screens::settings::{self as settings_screen, SettingsScreen};
use crate::screens::world_create::{self, WorldCreate};
use crate::screens::world_select::{self, WorldSelect};
use crate::settings::UserSettings;

/// Distancia de la cámara la primera vez que se pasa a tercera persona, en metros.
const DEFAULT_THIRD_PERSON: f32 = 5.0;
/// Tiempo que se ve el aviso de cambio de modo de juego.
const NOTICE_TIME: Duration = Duration::from_secs(2);

/// Algo pedido con una tecla que necesita al motor: se resuelve en el siguiente `update`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    /// Pasar al siguiente modo de juego.
    CycleMode,
    /// Alternar entre primera y tercera persona.
    ToggleView,
    /// Alejar (> 0) o acercar la cámara.
    Zoom(i32),
    /// Tirar al suelo el ítem de la ranura activa.
    DropActive,
}

/// Lo que se decide al arrancar y no cambia con el juego en marcha.
pub struct Config {
    pub worlds_dir: PathBuf,
    pub content_dir: PathBuf,
    /// Archivo donde se guardan los ajustes del jugador.
    pub settings_path: PathBuf,
    /// Semilla de los mundos que se crean sin elegir una (línea de comandos).
    pub seed: u32,
    /// Desarrollo: pantalla que se abre sola al entrar en un mundo (`pause`, `inventory`,
    /// `console`, o `debug` para los datos de F3). Sirve para capturarlas.
    pub dev_open: Option<String>,
    /// Desarrollo: líneas de consola que se ejecutan solas al entrar en un mundo.
    pub dev_console: Vec<String>,
}

/// Qué hacer al arrancar.
pub enum Launch {
    Menu,
    /// Directo a una partida (argumentos de línea de comandos).
    Session(SessionMode),
}

/// Pantalla que tiene la entrada. Dentro de un mundo, debajo siempre está el HUD.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shown {
    /// Ninguna: se está jugando.
    None,
    MainMenu,
    WorldSelect,
    WorldCreate,
    /// Ajustes; al cerrarse vuelve a `settings_return`.
    Settings,
    Pause,
    Inventory,
    Console,
}

pub struct Tokikura {
    config: Config,
    settings: UserSettings,
    launch: Option<Launch>,
    logs: LogCapture,
    /// Capa de Iced; existe desde que el motor tiene dispositivo gráfico.
    ui: Option<IcedLayer>,
    shown: Shown,
    /// Pantalla a la que se vuelve al cerrar los ajustes.
    settings_return: Shown,
    /// Hay un mundo abierto (según la última llamada del motor).
    in_session: bool,
    /// Capturar el ratón en cuanto se esté jugando (al entrar o al cerrar una pantalla).
    grab_requested: bool,
    show_debug: bool,
    /// Las opciones de desarrollo (`dev_open`, `dev_console`) ya se han aplicado.
    dev_applied: bool,

    /// Modo de juego y vista de la partida, copiados del motor en cada frame.
    mode: GameMode,
    view: ViewKind,
    camera_distance: f32,
    /// Distancia a la que vuelve la cámara al pasar a tercera persona.
    third_person_distance: f32,
    /// Alt pulsado: la rueda mueve la cámara en vez de la barra.
    alt: bool,
    actions: Vec<Action>,
    /// Aviso en pantalla y hasta cuándo se ve.
    notice: Option<(String, Instant)>,

    /// Contenido del mundo abierto y barra de herramientas del jugador.
    content: Arc<ContentData>,
    /// Iconos de los ítems de `content`.
    icons: Arc<Icons>,
    hotbar: Hotbar,

    main_menu: MainMenu,
    world_select: WorldSelect,
    world_create: WorldCreate,
    settings_screen: SettingsScreen,
    pause_menu: PauseMenu,
    hud: Hud,
    inventory: Inventory,
    console: Console,
}

impl Tokikura {
    pub fn new(config: Config, settings: UserSettings, launch: Launch, logs: LogCapture) -> Self {
        let world_select = WorldSelect::new(config.worlds_dir.clone());
        Self {
            config,
            settings,
            launch: Some(launch),
            logs,
            ui: None,
            shown: Shown::None,
            settings_return: Shown::MainMenu,
            in_session: false,
            grab_requested: false,
            show_debug: false,
            dev_applied: false,
            mode: GameMode::default(),
            view: ViewKind::default(),
            camera_distance: 0.0,
            third_person_distance: DEFAULT_THIRD_PERSON,
            alt: false,
            actions: Vec::new(),
            notice: None,
            content: Arc::default(),
            icons: Arc::default(),
            hotbar: Hotbar::default(),
            main_menu: MainMenu::default(),
            world_select,
            world_create: WorldCreate::default(),
            settings_screen: SettingsScreen::default(),
            pause_menu: PauseMenu::default(),
            hud: Hud::default(),
            inventory: Inventory::default(),
            console: Console::default(),
        }
    }

    // --- Navegación entre pantallas ---

    fn open_main_menu(&mut self, status: impl Into<String>) {
        self.main_menu.set_status(status);
        self.shown = Shown::MainMenu;
    }

    fn open_world_select(&mut self, status: impl Into<String>) {
        self.world_select.refresh(status);
        self.shown = Shown::WorldSelect;
    }

    fn open_settings(&mut self) {
        self.settings_return = self.shown;
        self.settings_screen.open(&self.settings, self.in_session);
        self.shown = Shown::Settings;
    }

    fn open_console(&mut self) {
        self.shown = Shown::Console;
        self.focus(console::INPUT);
    }

    /// Da el foco del teclado a una caja de texto de la pantalla que se va a mostrar.
    fn focus(&mut self, input: &'static str) {
        if let Some(ui) = self.ui.as_mut() {
            ui.focus(input);
        }
    }

    fn open_inventory(&mut self) {
        self.shown = Shown::Inventory;
        // Si se dejó abierta la búsqueda, se puede seguir escribiendo sin hacer clic.
        if self.inventory.typing() {
            self.focus(inventory::SEARCH_INPUT);
        }
    }

    /// Cierra la pantalla que haya dentro del mundo y devuelve el control al jugador.
    fn resume(&mut self) {
        self.shown = Shown::None;
        self.grab_requested = true;
    }

    /// "Atrás" (Escape). Devuelve `false` si no hay a dónde volver.
    fn back(&mut self) -> bool {
        match self.shown {
            Shown::None if self.in_session => self.shown = Shown::Pause,
            Shown::Pause | Shown::Inventory | Shown::Console => self.resume(),
            Shown::Settings => self.shown = self.settings_return,
            Shown::WorldCreate => self.open_world_select(""),
            Shown::WorldSelect => self.open_main_menu(""),
            Shown::None | Shown::MainMenu => return false,
        }
        true
    }

    // --- Mundos ---

    fn play(&mut self, ctx: &mut Context, mode: SessionMode, seed: u32) {
        ctx.start_session(SessionConfig {
            mode,
            player_name: self.settings.player_name.clone(),
            view_distance: self.settings.view_distance,
            far_distance: self.settings.far_distance,
            seed,
            content_dir: self.config.content_dir.clone(),
        });
        // Cada mundo numera los bloques a su manera: la barra empieza de cero.
        self.content = Arc::default();
        self.icons = Arc::default();
        self.hotbar = Hotbar::default();
        self.shown = Shown::None;
    }

    fn play_world(&mut self, ctx: &mut Context, world: &str, seed: u32) {
        let world_dir = self.config.worlds_dir.join(world);
        self.play(ctx, SessionMode::Host { world_dir, listen: None }, seed);
        self.grab_requested = true;
    }

    fn leave_world(&mut self, ctx: &mut Context) {
        ctx.end_session();
        self.in_session = false;
        self.open_main_menu("");
    }

    /// Arranque, separado de `init` para poder probarlo sin dispositivo gráfico.
    fn start(&mut self, ctx: &mut Context) {
        match self.launch.take() {
            Some(Launch::Session(mode)) => self.play(ctx, mode, self.config.seed),
            _ => self.open_main_menu(""),
        }
    }

    /// Desarrollo: abre la pantalla y ejecuta las órdenes pedidas al arrancar.
    fn apply_dev_options(&mut self, ctx: &mut Context) {
        match self.config.dev_open.as_deref() {
            Some("pause") => self.shown = Shown::Pause,
            Some("inventory") => self.open_inventory(),
            Some("console") => self.open_console(),
            Some("debug") => self.show_debug = true,
            Some(other) => log::warn!("KURAVOXEL_UI_OPEN: unknown screen '{other}'"),
            None => {}
        }
        for line in std::mem::take(&mut self.config.dev_console) {
            self.console.update(console::Message::InputChanged(line));
            self.console.update(console::Message::Submit);
        }
        for console::Request::Run(line) in self.console.take_requests() {
            self.run_console_line(ctx, &line);
        }
    }

    // --- Ajustes ---

    fn apply_settings(&mut self, ctx: &mut Context, settings: UserSettings) {
        self.settings = settings.sanitized();
        if let Err(error) = self.settings.save(&self.config.settings_path) {
            log::error!("could not save {}: {error:#}", self.config.settings_path.display());
        }
        ctx.configure(self.settings.client());
        if let Some(ui) = self.ui.as_mut() {
            ui.set_scale(self.settings.ui_scale);
        }
    }

    // --- Teclado y ratón ---

    /// Tecla pulsada. Devuelve `true` si era un atajo de la interfaz: en ese caso no
    /// llega ni a las pantallas ni a la partida.
    fn key_pressed(&mut self, key: KeyCode, repeat: bool) -> bool {
        // Con la consola abierta las flechas recorren las órdenes anteriores.
        if self.shown == Shown::Console {
            match key {
                KeyCode::ArrowUp => self.console.update(console::Message::HistoryPrevious),
                KeyCode::ArrowDown => self.console.update(console::Message::HistoryNext),
                _ => {}
            }
            if matches!(key, KeyCode::ArrowUp | KeyCode::ArrowDown) {
                return true;
            }
        }
        if repeat {
            return false;
        }
        if key == KeyCode::Escape {
            return self.back();
        }
        if !self.in_session {
            return false;
        }
        let playing = self.shown == Shown::None;
        // En la vista de mapa solo se mira y se camina: ni barra ni inventario.
        let map = self.view == ViewKind::Map;
        // Inventario abierto sin estar escribiendo en su buscador.
        let inventory = self.shown == Shown::Inventory && !self.inventory.typing();
        match key {
            // La tecla a la izquierda del 1 ("º" en un teclado español) o F1.
            KeyCode::Backquote | KeyCode::F1 => match self.shown {
                Shown::Console => self.resume(),
                Shown::None | Shown::Inventory | Shown::Pause => self.open_console(),
                _ => return false,
            },
            KeyCode::F5 if playing => self.actions.push(Action::CycleMode),
            KeyCode::KeyV if playing => self.actions.push(Action::ToggleView),
            KeyCode::KeyE if playing && !map => self.open_inventory(),
            KeyCode::KeyE if inventory => self.resume(),
            KeyCode::F3 if playing => self.show_debug = !self.show_debug,
            KeyCode::KeyQ if playing && !map => self.actions.push(Action::DropActive),
            _ if (playing && !map) || inventory => match digit(key) {
                Some(slot) => self.hotbar.select(slot),
                None => return false,
            },
            _ => return false,
        }
        true
    }

    /// Entrada que el juego se queda antes de que la vean las pantallas o la partida.
    fn shortcut(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.physical_key {
                    PhysicalKey::Code(key) => self.key_pressed(key, event.repeat),
                    PhysicalKey::Unidentified(_) => false,
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.alt = modifiers.state().alt_key();
                false
            }
            WindowEvent::MouseWheel { delta, .. } if self.in_session && self.shown == Shown::None => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32,
                };
                self.wheel(lines);
                true
            }
            _ => false,
        }
    }

    /// Rueda del ratón mientras se juega: cambia de ranura; con Alt (o en la vista de
    /// mapa, donde no hay barra) acerca o aleja la cámara.
    fn wheel(&mut self, lines: f32) {
        if lines == 0.0 {
            return;
        }
        let step = if lines > 0.0 { -1 } else { 1 };
        if self.alt || self.view == ViewKind::Map {
            self.actions.push(Action::Zoom(step));
        } else {
            self.hotbar.scroll(step);
        }
    }

    // --- Modo de juego y cámara ---

    fn set_mode(&mut self, ctx: &mut Context, mode: GameMode) {
        self.mode = mode;
        ctx.set_move_mode(mode.move_mode());
        self.notice = Some((mode.name().to_string(), Instant::now() + NOTICE_TIME));
    }

    fn run_actions(&mut self, ctx: &mut Context) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                Action::CycleMode => self.set_mode(ctx, self.mode.next()),
                Action::ToggleView => {
                    let distance =
                        if self.camera_distance > 0.0 { 0.0 } else { self.third_person_distance };
                    self.camera_distance = distance;
                    ctx.set_camera_distance(distance);
                }
                Action::Zoom(steps) => ctx.zoom_camera(steps),
                // Es creativo: el ítem se tira, pero la ranura lo conserva.
                Action::DropActive => {
                    if let Some(item) = self.hotbar.active_item() {
                        ctx.drop_item(item);
                    }
                }
            }
        }
    }

    // --- Consola ---

    fn run_console_line(&mut self, ctx: &mut Context, line: &str) {
        let command = match commands::parse(line) {
            Ok(command) => command,
            Err(problem) => return self.console.print(Kind::Error, &problem),
        };
        let print = |console: &mut Console, lines: &[String]| {
            for line in lines {
                console.print(Kind::Output, line);
            }
        };
        match command {
            Command::Help => {
                self.console.print(Kind::Output, "Client commands:");
                self.console.print(Kind::Output, &commands::HELP.join("\n"));
                if ctx.in_session() {
                    self.console.print(Kind::Output, "Server commands:");
                    ctx.server_command("help");
                }
            }
            Command::Clear => self.console.clear(),
            Command::Position => match &ctx.session {
                Some(SessionInfo { position: [x, y, z], .. }) => {
                    self.console.print(Kind::Output, &format!("{x:.2} {y:.2} {z:.2}"))
                }
                None => self.console.print(Kind::Error, "Not in a world."),
            },
            Command::Stats => match &ctx.session {
                Some(info) => self.console.print(Kind::Output, &info.status.replace(" | ", "\n")),
                None => self.console.print(Kind::Error, "Not in a world."),
            },
            Command::Blocks(filter) => {
                let filter = filter.unwrap_or_default().to_lowercase();
                let lines: Vec<String> = self
                    .content
                    .blocks
                    .iter()
                    .filter(|block| {
                        block.id.contains(&filter) || block.name.to_lowercase().contains(&filter)
                    })
                    .map(|block| match block.orientation.property() {
                        None => format!("{}  {}", block.id, block.name),
                        Some((key, values)) => {
                            format!("{}  {}  [{key}={}]", block.id, block.name, values.join("|"))
                        }
                    })
                    .collect();
                if lines.is_empty() {
                    self.console.print(Kind::Error, "No blocks match.");
                }
                print(&mut self.console, &lines);
            }
            Command::Items(filter) => {
                let filter = filter.unwrap_or_default();
                let lines: Vec<String> = self
                    .content
                    .numbered_items()
                    .filter(|(_, item)| item.matches(&filter))
                    .map(|(_, item)| {
                        let tool = item.tool.as_ref().map_or(String::new(), |tool| format!("  (tool: {tool})"));
                        format!("{}  {}  [{}]{tool}", item.id, item.name, item.category)
                    })
                    .collect();
                if lines.is_empty() {
                    self.console.print(Kind::Error, "No items match.");
                }
                print(&mut self.console, &lines);
            }
            Command::Give(name, _) | Command::Block(name)
                if hotbar::find_item(&self.content, &name).is_none() =>
            {
                self.console
                    .print(Kind::Error, &format!("No item called '{name}'. Type 'items' for a list."))
            }
            Command::Give(name, slot) => {
                let item = hotbar::find_item(&self.content, &name).expect("checked above");
                let slot = match slot {
                    Some(slot) => {
                        self.hotbar.assign(slot, Some(item));
                        self.hotbar.select(slot);
                        slot
                    }
                    None => self.hotbar.give(item),
                };
                let id = self.content.item(item).map_or("", |item| item.id.as_str());
                self.console.print(Kind::Output, &format!("Gave {id} (slot {}).", slot + 1));
            }
            Command::Block(name) => {
                let item = hotbar::find_item(&self.content, &name).expect("checked above");
                self.hotbar.assign(self.hotbar.active(), Some(item));
                let id = self.content.item(item).map_or("", |item| item.id.as_str());
                self.console.print(Kind::Output, &format!("Selected {id}."));
            }
            Command::Drop(_) if !ctx.in_session() => self.console.print(Kind::Error, "Not in a world."),
            Command::Drop(None) => match self.hotbar.active_item() {
                Some(item) => ctx.drop_item(item),
                None => self.console.print(Kind::Error, "The selected slot is empty."),
            },
            Command::Drop(Some(name)) => match hotbar::find_item(&self.content, &name) {
                Some(item) => ctx.drop_item(item),
                None => self
                    .console
                    .print(Kind::Error, &format!("No item called '{name}'. Type 'items' for a list.")),
            },
            Command::Icons => {
                let dir = PathBuf::from(ICON_EXPORT_DIR);
                match self.icons.export(&self.content, &dir) {
                    Ok(count) => self.console.print(
                        Kind::Output,
                        &format!(
                            "Wrote {count} icons to {}/. Copy one to content/<pack>/icons/ and set [icon] image to use it.",
                            dir.display()
                        ),
                    ),
                    Err(error) => self.console.print(Kind::Error, &format!("Could not export the icons: {error:#}")),
                }
            }
            Command::Look => {
                let target = ctx.session.as_ref().and_then(|info| Some((info.target?, info.target_block?)));
                match target {
                    Some(([x, y, z], block)) => {
                        let lines = describe_block(&self.content, block);
                        self.console.print(Kind::Output, &format!("At {x} {y} {z}:"));
                        print(&mut self.console, &lines);
                    }
                    None => self.console.print(Kind::Error, "You are not looking at a block."),
                }
            }
            Command::Mode(_) | Command::Zoom(_) if !ctx.in_session() => {
                self.console.print(Kind::Error, "Not in a world.")
            }
            Command::Mode(None) => self.console.print(Kind::Output, self.mode.name()),
            Command::Mode(Some(name)) => match GameMode::parse(&name) {
                Some(mode) => {
                    self.set_mode(ctx, mode);
                    self.console.print(Kind::Output, &format!("Game mode: {}.", mode.name()));
                }
                None => self
                    .console
                    .print(Kind::Error, "Unknown mode. Use survival, creative or spectator."),
            },
            Command::Zoom(meters) => ctx.set_camera_distance(meters),
            Command::Set(None) => print(&mut self.console, &self.settings.describe()),
            Command::Set(Some((key, value))) => {
                let mut settings = self.settings.clone();
                match settings.set(&key, &value) {
                    Ok(()) => {
                        self.apply_settings(ctx, settings);
                        let shown = self.settings.describe();
                        let line = shown.iter().find(|line| line.starts_with(&format!("{key} =")));
                        self.console.print(Kind::Output, line.map_or("Done.", String::as_str));
                    }
                    Err(problem) => self.console.print(Kind::Error, &problem),
                }
            }
            Command::Menu if ctx.in_session() => self.leave_world(ctx),
            Command::Menu => self.console.print(Kind::Error, "Not in a world."),
            Command::Quit => ctx.exit(),
            Command::Server(line) if ctx.in_session() => ctx.server_command(line),
            Command::Server(_) => self.console.print(Kind::Error, "Not in a world."),
        }
    }

    // --- Peticiones de las pantallas ---

    fn handle_requests(&mut self, ctx: &mut Context) {
        for request in self.main_menu.take_requests() {
            match request {
                main_menu::Request::Worlds => self.open_world_select(""),
                main_menu::Request::Settings => self.open_settings(),
                main_menu::Request::Quit => ctx.exit(),
            }
        }
        for request in self.world_select.take_requests() {
            match request {
                // Un mundo que ya existe conserva su semilla: la de aquí no se usa.
                world_select::Request::Play(world) => self.play_world(ctx, &world, self.config.seed),
                world_select::Request::Create => {
                    self.world_create.open(self.world_select.names());
                    self.shown = Shown::WorldCreate;
                    self.focus(world_create::NAME_INPUT);
                }
                world_select::Request::Back => self.open_main_menu(""),
            }
        }
        for request in self.world_create.take_requests() {
            match request {
                world_create::Request::Create { name, seed } => self.play_world(ctx, &name, seed),
                world_create::Request::Cancel => self.open_world_select(""),
            }
        }
        for request in self.settings_screen.take_requests() {
            if let settings_screen::Request::Apply(settings) = request {
                self.apply_settings(ctx, settings);
            }
            self.shown = self.settings_return;
        }
        for request in self.pause_menu.take_requests() {
            match request {
                pause_menu::Request::Resume => self.resume(),
                pause_menu::Request::Settings => self.open_settings(),
                pause_menu::Request::Leave => self.leave_world(ctx),
                pause_menu::Request::Quit => ctx.exit(),
            }
        }
        for request in self.inventory.take_requests() {
            match request {
                inventory::Request::Hotbar(hotbar) => self.hotbar = hotbar,
                inventory::Request::FocusSearch => self.focus(inventory::SEARCH_INPUT),
                inventory::Request::Close => self.resume(),
            }
        }
        for console::Request::Run(line) in self.console.take_requests() {
            self.run_console_line(ctx, &line);
        }
    }
}

/// Carpeta (relativa al directorio de trabajo) donde `icons` escribe los iconos.
const ICON_EXPORT_DIR: &str = "icons_export";

/// Descripción de un estado de bloque para la consola (`look`).
fn describe_block(content: &ContentData, block: Block) -> Vec<String> {
    let (Some(name), Some(def)) = (content.state_name(block), content.block(block)) else {
        return vec![format!("unknown block #{}", block.0)];
    };
    let hex = |[r, g, b]: [u8; 3]| format!("#{r:02X}{g:02X}{b:02X}");
    let mut lines = vec![
        format!("{name}  {}  (#{})", def.name, block.0),
        format!("category: {}  tags: {}", def.category, def.tags.join(", ")),
        format!(
            "solid: {}  transparent: {}  light: {}",
            def.solid, def.transparent, def.light
        ),
        format!(
            "color: {}  top: {}  side: {}  bottom: {}",
            hex(def.color),
            hex(def.faces.top),
            hex(def.faces.side),
            hex(def.faces.bottom)
        ),
    ];
    if content.item_for_block(block).is_none() {
        lines.push("no item: it cannot be picked into the hotbar".into());
    }
    lines
}

/// Ranura de la barra que elige una tecla de número (del 1 al 9).
fn digit(key: KeyCode) -> Option<usize> {
    const DIGITS: [KeyCode; hotbar::SLOTS] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    DIGITS.iter().position(|&digit| digit == key)
}

impl Game for Tokikura {
    fn init(&mut self, ctx: &mut Context, host: &Host) {
        let mut ui = IcedLayer::new(host);
        ui.set_scale(self.settings.ui_scale);
        self.ui = Some(ui);
        self.start(ctx);
    }

    fn update(&mut self, ctx: &mut Context) {
        self.in_session = ctx.in_session();
        if let Some(reason) = ctx.session_lost.take() {
            self.open_main_menu(format!("Session ended: {reason}"));
            return;
        }
        // El motor avisa de un Escape que el juego no ha visto: también es "atrás".
        if ctx.escape && self.shown == Shown::None {
            self.back();
        }

        // Lo que ha pasado desde el frame anterior: registro del motor y avisos de la partida.
        for (level, line) in self.logs.drain() {
            let kind = if level <= log::Level::Warn { Kind::Error } else { Kind::Log };
            self.console.print(kind, &line);
        }
        for event in std::mem::take(&mut ctx.session_events) {
            match event {
                SessionEvent::PickedUp(item) => self.hotbar.receive(item),
                SessionEvent::BlockPicked(block) => {
                    // Un bloque sin ítem (`item = false`) no se puede copiar a la barra.
                    if let Some(item) = self.content.item_for_block(block) {
                        self.hotbar.pick(item);
                    }
                }
                SessionEvent::CommandOutput { text, ok } => {
                    self.console.print(if ok { Kind::Output } else { Kind::Error }, &text)
                }
            }
        }
        if let Some(info) = &ctx.session {
            self.mode = GameMode::from_move_mode(info.move_mode);
            self.view = info.view;
            self.camera_distance = info.camera_distance;
            if info.view == ViewKind::ThirdPerson {
                self.third_person_distance = info.camera_distance.min(THIRD_PERSON_MAX);
            }
            // Contenido nuevo (al entrar o por recarga en caliente): la barra se ajusta.
            if !Arc::ptr_eq(&info.content, &self.content) {
                self.content = info.content.clone();
                self.icons = Arc::new(Icons::new(&self.content));
                self.hotbar.sync(&self.content);
            }
        }

        // Desarrollo: con el contenido ya recibido, para que las órdenes tengan mundo.
        if self.in_session && !self.dev_applied && self.content.block_count() > 0 {
            self.dev_applied = true;
            self.apply_dev_options(ctx);
        }
        self.handle_requests(ctx);
        if self.in_session {
            self.run_actions(ctx);
        } else {
            self.actions.clear();
        }
        if self.notice.as_ref().is_some_and(|(_, until)| Instant::now() >= *until) {
            self.notice = None;
        }

        // Con todo ya decidido, el motor y las pantallas se ponen al día.
        if let Some(info) = ctx.session.as_ref().filter(|_| self.in_session) {
            self.hud.notice = self.notice.as_ref().map(|(text, _)| text.clone());
            self.hud.sync(info, &self.icons, &self.hotbar, self.mode.name(), self.show_debug);
            self.inventory.sync(&self.content, &self.icons, &self.hotbar);
            let block = self.hotbar.active_block(&self.content);
            if info.selected != block {
                ctx.select_block(block);
            }
        }
        if self.grab_requested && ctx.in_session() && self.in_session {
            self.grab_requested = false;
            if self.shown == Shown::None {
                ctx.grab_mouse(true);
            }
        }
    }

    fn window_event(&mut self, event: &WindowEvent) -> bool {
        if self.shortcut(event) {
            return true;
        }
        if let Some(ui) = self.ui.as_mut() {
            ui.window_event(event);
        }
        self.wants_input()
    }

    fn wants_input(&self) -> bool {
        self.shown != Shown::None
    }

    fn draw_overlay(&mut self, view: &wgpu::TextureView, _target: OverlayTarget) {
        let Some(ui) = self.ui.as_mut() else { return };
        // Dentro de un mundo el HUD va siempre debajo, sin entrada. En la vista de mapa
        // no hay interfaz: solo el terreno.
        if self.in_session && self.view != ViewKind::Map {
            self.hud.crosshair = self.shown == Shown::None;
            ui.draw(&self.hud, view);
        }
        match self.shown {
            Shown::None => ui.discard_events(),
            Shown::MainMenu => ui.frame(&mut self.main_menu, view),
            Shown::WorldSelect => ui.frame(&mut self.world_select, view),
            Shown::WorldCreate => ui.frame(&mut self.world_create, view),
            Shown::Settings => ui.frame(&mut self.settings_screen, view),
            Shown::Pause => ui.frame(&mut self.pause_menu, view),
            Shown::Inventory => ui.frame(&mut self.inventory, view),
            Shown::Console => ui.frame(&mut self.console, view),
        }
    }
}

#[cfg(test)]
mod tests {
    use kuravoxel_client::app::Command as EngineCommand;
    use kuravoxel_common::block::Block;
    use kuravoxel_common::content::ItemId;

    use super::*;
    use crate::hotbar::tests::content;

    /// El juego sobre una carpeta temporal con un mundo ("alpha"); se borra al terminar.
    struct Fixture {
        game: Tokikura,
        dir: PathBuf,
    }

    impl Fixture {
        fn new(test: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("tokikura-game-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("alpha")).unwrap();
            std::fs::write(dir.join("alpha").join("world.toml"), "seed = 1\nblocks = []\n").unwrap();
            let config = Config {
                worlds_dir: dir.clone(),
                content_dir: "content".into(),
                settings_path: dir.join("settings.toml"),
                seed: 7,
                dev_open: None,
                dev_console: Vec::new(),
            };
            let game =
                Tokikura::new(config, UserSettings::default(), Launch::Menu, LogCapture::default());
            Self { game, dir }
        }

        /// Deja el juego dentro de un mundo con tres bloques, ya con un frame pasado.
        fn in_world(test: &str) -> (Self, SessionInfo) {
            let mut fixture = Self::new(test);
            fixture.game.shown = Shown::None;
            let info = SessionInfo {
                content: Arc::new(content(&["stone", "dirt", "grass"])),
                // El bloque que la barra elige al llenarse: el motor ya está al día.
                selected: Block(1),
                position: [1.0, 2.0, 3.0],
                status: Arc::from("60 fps | chunks 10"),
                hosting: true,
                loaded: true,
                ..SessionInfo::default()
            };
            fixture.frame(&info);
            (fixture, info)
        }

        /// Un frame dentro del mundo; devuelve las órdenes dadas al motor.
        fn frame(&mut self, info: &SessionInfo) -> Vec<EngineCommand> {
            let mut ctx = Context::new(Some(info.clone()));
            self.game.update(&mut ctx);
            ctx.take_commands()
        }

        /// Escribe una línea en la consola y pasa un frame.
        fn console(&mut self, info: &SessionInfo, line: &str) -> Vec<EngineCommand> {
            self.game.console.update(console::Message::InputChanged(line.into()));
            self.game.console.update(console::Message::Submit);
            self.frame(info)
        }

        fn last_line(&self) -> (Kind, String) {
            let (kind, line) = *self.game.console.lines().last().expect("console output");
            (kind, line.to_string())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn started_world(commands: &[EngineCommand]) -> Option<(PathBuf, u32)> {
        commands.iter().find_map(|command| match command {
            EngineCommand::StartSession(config) => match &config.mode {
                SessionMode::Host { world_dir, .. } => Some((world_dir.clone(), config.seed)),
                SessionMode::Join(_) => None,
            },
            _ => None,
        })
    }

    #[test]
    fn the_menus_lead_to_an_existing_world() {
        let mut fixture = Fixture::new("menu");
        let game = &mut fixture.game;
        let mut ctx = Context::new(None);
        game.start(&mut ctx);
        assert!(ctx.take_commands().is_empty());
        assert_eq!(game.shown, Shown::MainMenu);

        game.main_menu.update(main_menu::Message::Play);
        game.update(&mut ctx);
        assert_eq!(game.shown, Shown::WorldSelect);

        game.world_select.update(world_select::Message::Select(0));
        game.world_select.update(world_select::Message::Play);
        game.update(&mut ctx);
        let (world, _) = started_world(&ctx.take_commands()).expect("a world starts");
        assert_eq!(world, fixture.dir.join("alpha"));
        assert!(!fixture.game.wants_input(), "the menu must get out of the way of the world");
    }

    #[test]
    fn a_new_world_starts_with_the_chosen_seed() {
        let mut fixture = Fixture::new("create");
        let game = &mut fixture.game;
        let mut ctx = Context::new(None);
        game.open_world_select("");
        game.world_select.update(world_select::Message::Create);
        game.update(&mut ctx);
        assert_eq!(game.shown, Shown::WorldCreate);

        game.world_create.update(world_create::Message::NameChanged("alpha".into()));
        game.world_create.update(world_create::Message::Create);
        game.update(&mut ctx);
        assert_eq!(game.shown, Shown::WorldCreate, "an existing name is rejected");

        game.world_create.update(world_create::Message::NameChanged("beta".into()));
        game.world_create.update(world_create::Message::SeedChanged("99".into()));
        game.world_create.update(world_create::Message::Create);
        game.update(&mut ctx);
        assert_eq!(started_world(&ctx.take_commands()), Some((fixture.dir.join("beta"), 99)));
    }

    #[test]
    fn command_line_launch_skips_the_menu() {
        let mut fixture = Fixture::new("launch");
        let world_dir = fixture.dir.join("x");
        fixture.game.launch =
            Some(Launch::Session(SessionMode::Host { world_dir: world_dir.clone(), listen: None }));
        let mut ctx = Context::new(None);
        fixture.game.start(&mut ctx);
        assert_eq!(started_world(&ctx.take_commands()), Some((world_dir, 7)));
        assert!(!fixture.game.wants_input());
    }

    #[test]
    fn escape_walks_back_through_the_screens() {
        let (mut fixture, info) = Fixture::in_world("escape");
        let game = &mut fixture.game;
        assert!(game.key_pressed(KeyCode::Escape, false));
        assert_eq!(game.shown, Shown::Pause);

        game.pause_menu.update(pause_menu::Message::Settings);
        fixture.frame(&info);
        let game = &mut fixture.game;
        assert_eq!(game.shown, Shown::Settings);
        assert!(game.key_pressed(KeyCode::Escape, false));
        assert_eq!(game.shown, Shown::Pause, "settings go back to where they were opened");

        assert!(game.key_pressed(KeyCode::Escape, false));
        assert_eq!(game.shown, Shown::None);
        let commands = fixture.frame(&info);
        assert!(
            commands.iter().any(|command| matches!(command, EngineCommand::GrabMouse(true))),
            "going back to the game captures the mouse again"
        );

        // Fuera de un mundo, desde la portada no hay a dónde volver.
        let mut menu = Fixture::new("escape-menu");
        menu.game.open_world_select("");
        assert!(menu.game.key_pressed(KeyCode::Escape, false));
        assert_eq!(menu.game.shown, Shown::MainMenu);
        assert!(!menu.game.key_pressed(KeyCode::Escape, false));
    }

    #[test]
    fn leaving_the_world_ends_the_session_and_shows_the_menu() {
        let (mut fixture, info) = Fixture::in_world("leave");
        fixture.game.key_pressed(KeyCode::Escape, false);
        fixture.game.pause_menu.update(pause_menu::Message::Leave);
        let commands = fixture.frame(&info);
        assert!(matches!(commands[..], [EngineCommand::EndSession]));
        assert_eq!(fixture.game.shown, Shown::MainMenu);
        assert!(!fixture.game.in_session);
    }

    #[test]
    fn a_lost_session_is_explained_on_the_menu() {
        let (mut fixture, _) = Fixture::in_world("lost");
        let mut ctx = Context::new(None);
        ctx.session_lost = Some("disconnected from the server".into());
        fixture.game.update(&mut ctx);
        assert_eq!(fixture.game.shown, Shown::MainMenu);
        assert!(fixture.game.wants_input());
    }

    #[test]
    fn the_hotbar_drives_the_block_the_engine_places() {
        let mut fixture = Fixture::new("hotbar");
        fixture.game.shown = Shown::None;
        let mut info = SessionInfo {
            content: Arc::new(content(&["stone", "dirt", "grass"])),
            ..SessionInfo::default()
        };
        // Al llegar el contenido la barra se llena y el motor recibe el bloque activo.
        let commands = fixture.frame(&info);
        assert!(matches!(commands[..], [EngineCommand::SelectBlock(Block(1))]));
        info.selected = Block(1);
        assert!(fixture.frame(&info).is_empty(), "nothing to say while nothing changes");

        // Teclas de número y rueda de la barra.
        assert!(fixture.game.key_pressed(KeyCode::Digit3, false));
        assert!(matches!(fixture.frame(&info)[..], [EngineCommand::SelectBlock(Block(3))]));
        info.selected = Block(3);

        // Una ranura vacía no coloca nada.
        assert!(fixture.game.key_pressed(KeyCode::Digit9, false));
        assert!(matches!(fixture.frame(&info)[..], [EngineCommand::SelectBlock(Block::AIR)]));
        info.selected = Block::AIR;

        // Copiar un bloque del mundo (clic central) salta a su ranura.
        let mut ctx = Context::new(Some(SessionInfo { selected: Block(2), ..info.clone() }));
        ctx.session_events.push(SessionEvent::BlockPicked(Block(2)));
        fixture.game.update(&mut ctx);
        assert_eq!(fixture.game.hotbar.active(), 1);
        assert!(ctx.take_commands().is_empty());
    }

    #[test]
    fn the_inventory_opens_with_e_and_edits_the_hotbar() {
        let (mut fixture, info) = Fixture::in_world("inventory");
        assert!(fixture.game.key_pressed(KeyCode::KeyE, false));
        assert_eq!(fixture.game.shown, Shown::Inventory);
        assert!(fixture.game.wants_input());
        fixture.frame(&info);

        fixture.game.inventory.update(inventory::Message::SelectSlot(6));
        fixture.game.inventory.update(inventory::Message::Pick(ItemId(1)));
        fixture.frame(&info);
        assert_eq!((fixture.game.hotbar.active(), fixture.game.hotbar.active_block(&fixture.game.content)), (6, Block(2)));

        assert!(fixture.game.key_pressed(KeyCode::KeyE, false));
        assert_eq!(fixture.game.shown, Shown::None);
    }

    #[test]
    fn f5_cycles_the_game_modes() {
        use kuravoxel_common::physics::MoveMode;
        let (mut fixture, mut info) = Fixture::in_world("modes");
        for (mode, name) in [
            (MoveMode::Fly, "Creative"),
            (MoveMode::Ghost, "Spectator"),
            (MoveMode::Walk, "Survival"),
        ] {
            assert!(fixture.game.key_pressed(KeyCode::F5, false));
            let commands = fixture.frame(&info);
            assert!(matches!(commands[..], [EngineCommand::SetMoveMode(m)] if m == mode));
            assert_eq!(fixture.game.hud.notice.as_deref(), Some(name));
            // El motor ya se mueve así en el frame siguiente.
            info.move_mode = mode;
        }

        // También desde la consola, por nombre.
        let commands = fixture.console(&info, "mode spectator");
        assert!(matches!(commands[..], [EngineCommand::SetMoveMode(MoveMode::Ghost)]));
        fixture.console(&info, "mode");
        assert_eq!(fixture.last_line(), (Kind::Output, "Survival".into()));
        fixture.console(&info, "mode hardcore");
        assert_eq!(fixture.last_line().0, Kind::Error);
    }

    #[test]
    fn v_and_the_wheel_move_the_camera() {
        let (mut fixture, mut info) = Fixture::in_world("camera");
        assert!(fixture.game.key_pressed(KeyCode::KeyV, false));
        let commands = fixture.frame(&info);
        assert!(matches!(
            commands[..],
            [EngineCommand::SetCameraDistance(d)] if d == DEFAULT_THIRD_PERSON
        ));

        // La rueda sola es de la barra; con Alt, de la cámara.
        info.view = ViewKind::ThirdPerson;
        info.camera_distance = 12.0;
        fixture.frame(&info);
        fixture.game.wheel(-1.0);
        assert_eq!(fixture.game.hotbar.active(), 1);
        assert!(matches!(fixture.frame(&info)[..], [EngineCommand::SelectBlock(Block(2))]));
        info.selected = Block(2);
        fixture.game.alt = true;
        fixture.game.wheel(-1.0);
        assert_eq!(fixture.game.hotbar.active(), 1);
        assert!(matches!(fixture.frame(&info)[..], [EngineCommand::ZoomCamera(1)]));
        fixture.game.alt = false;

        // V vuelve a los ojos y recuerda la distancia para la próxima vez.
        fixture.game.key_pressed(KeyCode::KeyV, false);
        assert!(matches!(fixture.frame(&info)[..], [EngineCommand::SetCameraDistance(d)] if d == 0.0));
        info.view = ViewKind::FirstPerson;
        info.camera_distance = 0.0;
        fixture.frame(&info);
        fixture.game.key_pressed(KeyCode::KeyV, false);
        assert!(matches!(fixture.frame(&info)[..], [EngineCommand::SetCameraDistance(d)] if d == 12.0));

        let commands = fixture.console(&info, "zoom 200");
        assert!(matches!(commands[..], [EngineCommand::SetCameraDistance(d)] if d == 200.0));
    }

    #[test]
    fn the_map_view_only_lets_you_look_and_walk() {
        let (mut fixture, mut info) = Fixture::in_world("map");
        info.view = ViewKind::Map;
        info.camera_distance = 160.0;
        fixture.frame(&info);
        // Ni barra ni inventario; la rueda, sin Alt, acerca y aleja el mapa.
        assert!(!fixture.game.key_pressed(KeyCode::Digit3, false));
        assert!(!fixture.game.key_pressed(KeyCode::KeyE, false));
        assert_eq!((fixture.game.hotbar.active(), fixture.game.shown), (0, Shown::None));
        fixture.game.wheel(1.0);
        assert!(matches!(fixture.frame(&info)[..], [EngineCommand::ZoomCamera(-1)]));
        // La pausa sigue ahí.
        assert!(fixture.game.key_pressed(KeyCode::Escape, false));
        assert_eq!(fixture.game.shown, Shown::Pause);
    }

    #[test]
    fn typing_in_the_console_does_not_trigger_game_shortcuts() {
        let (mut fixture, _) = Fixture::in_world("typing");
        assert!(fixture.game.key_pressed(KeyCode::Backquote, false));
        assert_eq!(fixture.game.shown, Shown::Console);
        // Letras y números son texto para la consola, no atajos.
        assert!(!fixture.game.key_pressed(KeyCode::KeyE, false));
        assert!(!fixture.game.key_pressed(KeyCode::Digit3, false));
        assert_eq!(fixture.game.shown, Shown::Console);
        assert_eq!(fixture.game.hotbar.active(), 0);
        // Las flechas sí son de la consola (también repetidas).
        assert!(fixture.game.key_pressed(KeyCode::ArrowUp, true));

        assert!(fixture.game.key_pressed(KeyCode::F1, false));
        assert_eq!(fixture.game.shown, Shown::None);
    }

    #[test]
    fn console_commands_run_on_the_client_or_go_to_the_server() {
        let (mut fixture, info) = Fixture::in_world("console");

        assert!(fixture.console(&info, "pos").is_empty());
        assert_eq!(fixture.last_line(), (Kind::Output, "1.00 2.00 3.00".into()));

        let commands = fixture.console(&info, "tp 0 80 0");
        assert!(matches!(&commands[..], [EngineCommand::ServerCommand(line)] if line == "tp 0 80 0"));

        // La respuesta del servidor llega como aviso de la partida.
        let mut ctx = Context::new(Some(info.clone()));
        ctx.session_events
            .push(SessionEvent::CommandOutput { text: "Unknown command 'fly'.".into(), ok: false });
        fixture.game.update(&mut ctx);
        assert_eq!(fixture.last_line(), (Kind::Error, "Unknown command 'fly'.".into()));

        let commands = fixture.console(&info, "help");
        assert!(matches!(&commands[..], [EngineCommand::ServerCommand(line)] if line == "help"));

        let commands = fixture.console(&info, "block grass");
        assert_eq!(fixture.game.hotbar.active_block(&fixture.game.content), Block(3));
        assert!(matches!(commands[..], [EngineCommand::SelectBlock(Block(3))]));
        fixture.console(&info, "block lava");
        assert_eq!(fixture.last_line().0, Kind::Error);

        let commands = fixture.console(&info, "menu");
        assert!(matches!(commands[..], [EngineCommand::EndSession]));
    }

    #[test]
    fn items_are_given_listed_and_described_from_the_console() {
        let (mut fixture, mut info) = Fixture::in_world("items");

        // Ya en la barra: se salta a él. Con ranura: va justo ahí.
        fixture.console(&info, "give dirt");
        assert_eq!(fixture.last_line(), (Kind::Output, "Gave base:dirt (slot 2).".into()));
        info.selected = Block(2);
        let commands = fixture.console(&info, "give base:stone 9");
        assert_eq!(fixture.last_line(), (Kind::Output, "Gave base:stone (slot 9).".into()));
        assert_eq!(fixture.game.hotbar.active(), 8);
        assert!(matches!(commands[..], [EngineCommand::SelectBlock(Block(1))]));
        fixture.console(&info, "give lava");
        assert_eq!(fixture.last_line().0, Kind::Error);

        fixture.console(&info, "items gra");
        assert!(fixture.last_line().1.starts_with("base:grass"));
        fixture.console(&info, "blocks nothing-like-this");
        assert_eq!(fixture.last_line(), (Kind::Error, "No blocks match.".into()));

        fixture.console(&info, "look");
        assert_eq!(fixture.last_line().0, Kind::Error);
        info.target = Some([4, 5, 6]);
        info.target_block = Some(Block(3));
        fixture.console(&info, "look");
        let lines: Vec<String> =
            fixture.game.console.lines().iter().map(|(_, line)| line.to_string()).collect();
        assert!(lines.iter().any(|line| line == "At 4 5 6:"));
        assert!(lines.iter().any(|line| line.starts_with("base:grass  grass  (#3)")));
    }

    #[test]
    fn the_inventory_search_box_keeps_letters_and_digits() {
        let (mut fixture, info) = Fixture::in_world("search");
        assert!(fixture.game.key_pressed(KeyCode::KeyE, false));
        fixture.game.inventory.update(inventory::Message::Tab(inventory::Tab::Search));
        fixture.frame(&info);
        // Mientras se busca, E y los números se escriben en la caja.
        assert!(!fixture.game.key_pressed(KeyCode::KeyE, false));
        assert!(!fixture.game.key_pressed(KeyCode::Digit3, false));
        assert_eq!((fixture.game.shown, fixture.game.hotbar.active()), (Shown::Inventory, 0));
        // Escape cierra igualmente.
        assert!(fixture.game.key_pressed(KeyCode::Escape, false));
        assert_eq!(fixture.game.shown, Shown::None);
    }

    #[test]
    fn settings_changes_reach_the_engine_and_the_disk() {
        let (mut fixture, info) = Fixture::in_world("settings");
        let commands = fixture.console(&info, "set fov 95");
        assert!(matches!(
            commands[..],
            [EngineCommand::Configure(client)] if client.fov_degrees == 95.0
        ));
        assert_eq!(fixture.last_line(), (Kind::Output, "fov = 95".into()));
        assert_eq!(UserSettings::load(&fixture.dir.join("settings.toml")).fov, 95.0);

        fixture.console(&info, "set fov wide");
        assert_eq!(fixture.last_line().0, Kind::Error);

        // Lo mismo desde la pantalla de ajustes.
        fixture.game.shown = Shown::Pause;
        fixture.game.open_settings();
        fixture.game.settings_screen.update(settings_screen::Message::Vsync(false));
        fixture.game.settings_screen.update(settings_screen::Message::Done);
        let commands = fixture.frame(&info);
        assert!(matches!(commands[..], [EngineCommand::Configure(client)] if !client.vsync));
        assert_eq!(fixture.game.shown, Shown::Pause);
        assert!(!fixture.game.settings.vsync);
    }
}
