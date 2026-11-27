//! Tokikura: el juego. Todo lo que hay aquí es específico del juego; el motor
//! (KuraVoxel) pone la ventana, el render, la red y las partidas.

mod biome_preview;
mod builder;
mod commands;
mod environment;
mod game;
mod game_mode;
mod hotbar;
mod icons;
mod log_capture;
mod screens;
mod settings;
mod worlds;

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;

use anyhow::Context as _;
use kuravoxel_client::app::{self, AppConfig};
use kuravoxel_client::session::SessionMode;
use kuravoxel_common::protocol::DEFAULT_PORT;

use game::{Config, Launch, Tokikura};
use settings::UserSettings;

const TITLE: &str = "Tokikura";
const DEFAULT_SEED: u32 = 1337;

/// Lee un ajuste numérico de una variable de entorno.
fn env_number<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default)
}

fn env_path(name: &str, default: &str) -> PathBuf {
    std::env::var_os(name).map_or_else(|| PathBuf::from(default), PathBuf::from)
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

const HELP: &str = "\
usage: tokikura [--world NAME] [--host [ADDRESS[:PORT]]] [--connect ADDRESS[:PORT]] [--name NAME]

With no arguments the game opens on the main menu.
  --world    skips the menu and opens (or creates) worlds/NAME
  --host     opens the integrated server to the network (use with --world)
  --connect  skips the menu and joins a server
  --name     player name

In a world: left click breaks, right click places, middle click picks, wheel or 1-9
selects a hotbar slot, E opens the block palette, Escape opens the game menu, F1 (or
the key left of 1) opens the developer console, F3 shows debug information and F6 opens
the sky and light panel (time of day, sun, shadows, colors).";

fn main() -> anyhow::Result<()> {
    // El registro sale por la terminal y, además, se copia para la consola del juego.
    let logs = log_capture::init(
        "warn,tokikura=info,kuravoxel_client=info,kuravoxel_server=info,kuravoxel_ui=info",
    );

    let config = Config {
        worlds_dir: env_path("KURAVOXEL_WORLDS", "worlds"),
        content_dir: env_path("KURAVOXEL_CONTENT", "content"),
        settings_path: env_path("KURAVOXEL_SETTINGS", "settings.toml"),
        environment_path: env_path("KURAVOXEL_ENVIRONMENT", "environment.toml"),
        seed: env_number("KURAVOXEL_SEED", DEFAULT_SEED),
        dev_open: std::env::var("KURAVOXEL_UI_OPEN").ok(),
        dev_console: std::env::var("KURAVOXEL_CONSOLE")
            .map(|lines| lines.split(';').map(|line| line.trim().to_string()).collect())
            .unwrap_or_default(),
    };
    // Ajustes del jugador; las variables de entorno de desarrollo mandan sobre el archivo.
    let saved = UserSettings::load(&config.settings_path);
    let settings = UserSettings {
        view_distance: env_number("KURAVOXEL_VIEW_DISTANCE", saved.view_distance),
        far_distance: env_number("KURAVOXEL_FAR_DISTANCE", saved.far_distance),
        vsync: env_number("KURAVOXEL_VSYNC", saved.vsync as u8) != 0,
        max_fps: std::env::var("KURAVOXEL_MAX_FPS").unwrap_or(saved.max_fps.clone()),
        background_fps: env_number("KURAVOXEL_BACKGROUND_FPS", saved.background_fps),
        ..saved
    }
    .sanitized();

    // Argumentos: permiten saltarse el menú para ir directo a un mundo o a un servidor.
    let (mut world, mut listen, mut join) = (None::<String>, None::<SocketAddr>, None::<SocketAddr>);
    let mut settings = settings;
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--world" => world = Some(args.next().context("--world needs a name")?),
            "--connect" => join = Some(resolve(&args.next().context("--connect needs an address")?)?),
            "--host" => {
                // La dirección es opcional: por defecto, todas las interfaces.
                listen = Some(match args.peek() {
                    Some(next) if !next.starts_with("--") => resolve(&args.next().unwrap())?,
                    _ => resolve("0.0.0.0")?,
                });
            }
            "--name" => settings.player_name = args.next().context("--name needs a name")?,
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other} (try --help)"),
        }
    }
    let launch = match (join, world, listen) {
        (Some(addr), _, _) => Launch::Session(SessionMode::Join(addr)),
        (None, Some(name), listen) => {
            Launch::Session(SessionMode::Host { world_dir: config.worlds_dir.join(name), listen })
        }
        // Abrir a la red sin decir qué mundo: el de por defecto.
        (None, None, Some(addr)) => Launch::Session(SessionMode::Host {
            world_dir: config.worlds_dir.join(worlds::DEFAULT_WORLD),
            listen: Some(addr),
        }),
        (None, None, None) => Launch::Menu,
    };

    let app_config = AppConfig { title: TITLE.to_string(), client: settings.client() };
    app::run(app_config, Tokikura::new(config, settings, launch, logs))
}
