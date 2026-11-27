//! Prueba de punta a punta de los comandos de consola del servidor: el anfitrión puede
//! ejecutarlos, un jugador cualquiera no.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use kuravoxel_common::block::Block;
use kuravoxel_common::protocol::{ClientMsg, ServerMsg, WorldTime, PROTOCOL_VERSION};
use kuravoxel_server::{ServerConfig, ServerHandle};

struct Client {
    server: ServerHandle,
    id: u32,
    incoming: Receiver<ServerMsg>,
}

impl Client {
    fn join(server: &ServerHandle, operator: bool) -> Self {
        let (tx, incoming) = channel();
        let sink = Box::new(move |msg| tx.send(msg).is_ok());
        let id = if operator { server.connect_operator(sink) } else { server.connect(sink) };
        let client = Self { server: server.clone(), id, incoming };
        client.send(ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            name: if operator { "Host".into() } else { "Guest".into() },
            view_distance: 3,
            far_distance: 3,
        });
        client.wait("welcome", |msg| matches!(msg, ServerMsg::Welcome { .. }).then_some(()));
        client
    }

    fn send(&self, msg: ClientMsg) {
        self.server.message(self.id, msg);
    }

    /// Descarta mensajes hasta que `pick` reconoce el que se espera.
    fn wait<T>(&self, what: &str, mut pick: impl FnMut(ServerMsg) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            let msg = self.incoming.recv_timeout(wait).unwrap_or_else(|_| panic!("timed out: {what}"));
            if let Some(found) = pick(msg) {
                return found;
            }
        }
    }

    /// Ejecuta una línea de consola y devuelve la respuesta del servidor junto con lo que
    /// haya llegado antes de ella (cambios de bloque, chunks).
    fn command_with_effects(&self, line: &str) -> ((String, bool), Vec<ServerMsg>) {
        self.send(ClientMsg::Command { line: line.into() });
        let mut effects = Vec::new();
        let output = self.wait(line, |msg| match msg {
            ServerMsg::CommandOutput { text, ok } => Some((text, ok)),
            other => {
                effects.push(other);
                None
            }
        });
        (output, effects)
    }

    /// Ejecuta una línea de consola y devuelve la respuesta del servidor.
    fn command(&self, line: &str) -> (String, bool) {
        self.send(ClientMsg::Command { line: line.into() });
        self.wait(line, |msg| match msg {
            ServerMsg::CommandOutput { text, ok } => Some((text, ok)),
            _ => None,
        })
    }
}

#[test]
fn the_host_runs_server_commands_and_guests_are_refused() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let world_dir =
        std::env::temp_dir().join(format!("kuravoxel-commands-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&world_dir);
    let server = kuravoxel_server::start(ServerConfig {
        seed: 4242,
        world_dir: world_dir.clone(),
        content_dir: workspace.join("content"),
        ..Default::default()
    })
    .expect("server starts");

    let host = Client::join(&server, true);
    assert_eq!(host.command("seed"), ("Seed: 4242".into(), true));
    assert_eq!(host.command("save"), ("World saved.".into(), true));

    let (help, ok) = host.command("help");
    assert!(ok && help.contains("tp <x> <y> <z>"), "help lists the commands: {help}");

    let (error, ok) = host.command("fly");
    assert!(!ok && error.contains("Unknown command 'fly'"), "{error}");
    let (error, ok) = host.command("tp 1 2");
    assert!(!ok && error.starts_with("Usage:"), "{error}");

    // El teletransporte mueve al jugador en el servidor y se lo dice al cliente.
    host.send(ClientMsg::Command { line: "tp 500.5 90 -250".into() });
    let pos = host.wait("teleport", |msg| match msg {
        ServerMsg::Teleport { pos } => Some(pos),
        _ => None,
    });
    assert_eq!(pos, [500.5, 90.0, -250.0]);
    let confirmation = host.wait("teleport output", |msg| match msg {
        ServerMsg::CommandOutput { text, ok } => Some((text, ok)),
        _ => None,
    });
    assert_eq!(confirmation, ("Teleported to 500.5 90.0 -250.0".into(), true));
    let (players, ok) = host.command("players");
    assert!(ok && players.contains("Host at 500 90 -250"), "{players}");

    // Un jugador que no aloja el mundo no puede usar ninguno.
    let guest = Client::join(&server, false);
    let (error, ok) = guest.command("seed");
    assert!(!ok && error.contains("Only the host"), "{error}");
    guest.send(ClientMsg::Command { line: "tp 0 0 0".into() });
    let (error, ok) = guest.wait("refusal", |msg| match msg {
        ServerMsg::CommandOutput { text, ok } => Some((text, ok)),
        ServerMsg::Teleport { .. } => panic!("a guest must not be teleported"),
        _ => None,
    });
    assert!(!ok && error.contains("Only the host"), "{error}");

    server.shutdown();
    let _ = std::fs::remove_dir_all(world_dir);
}

#[test]
fn the_host_places_and_fills_blocks_by_name() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let world_dir =
        std::env::temp_dir().join(format!("kuravoxel-setblock-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&world_dir);
    let server = kuravoxel_server::start(ServerConfig {
        seed: 4242,
        world_dir: world_dir.clone(),
        content_dir: workspace.join("content"),
        ..Default::default()
    })
    .expect("server starts");

    let host = Client::join(&server, true);
    let content = host.wait("content", |msg| match msg {
        ServerMsg::Content(content) => Some(content),
        _ => None,
    });
    let log_x = content.block_id("base:wood[axis=x]").expect("oriented wood exists");
    assert_ne!(log_x, content.block_id("base:wood").unwrap());

    // Los chunks del jugador tardan un poco en generarse: se reintenta hasta que el
    // cambio llega como cualquier otra edición.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let ((text, ok), effects) = host.command_with_effects("setblock ~ ~2 ~ base:wood[axis=x]");
        let changed = effects.iter().any(|msg| {
            matches!(msg, ServerMsg::BlockChange { block, .. } if Block(*block) == log_x)
        });
        if ok && changed {
            assert!(text.starts_with("Placed base:wood[axis=x] at"), "{text}");
            break;
        }
        assert!(ok || text.contains("not loaded"), "{text}");
        assert!(Instant::now() < deadline, "the player's chunks never loaded");
        std::thread::sleep(Duration::from_millis(50));
    }

    // `fill` reenvía enteros los chunks que cambia.
    let stone = content.block_id("base:stone").unwrap();
    let ((text, ok), effects) = host.command_with_effects("fill ~-1 ~2 ~-1 ~1 ~3 ~1 base:stone");
    assert!(ok && text.starts_with("Filled 18 blocks"), "{text}");
    let resent: Vec<_> = effects
        .iter()
        .filter_map(|msg| match msg {
            ServerMsg::Chunk { chunk, .. } => Some(chunk),
            _ => None,
        })
        .collect();
    assert!(!resent.is_empty());
    assert!(resent.iter().any(|chunk| chunk.to_dense().contains(&stone)));

    let (error, ok) = host.command("setblock ~ ~ ~ base:nothing");
    assert!(!ok && error.contains("Unknown block 'base:nothing'"), "{error}");
    let (error, ok) = host.command("fill 0 0 0 100 100 100 air");
    assert!(!ok && error.contains("limit"), "{error}");

    server.shutdown();
    let _ = std::fs::remove_dir_all(world_dir);
}

#[test]
fn the_host_sets_the_time_and_the_world_remembers_it() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let world_dir = std::env::temp_dir().join(format!("kuravoxel-time-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&world_dir);
    let config =
        ServerConfig { world_dir: world_dir.clone(), content_dir: workspace.join("content"), ..Default::default() };
    let time = |client: &Client| {
        client.wait("time", |msg| match msg {
            ServerMsg::Time(time) => Some(time),
            _ => None,
        })
    };

    let server = kuravoxel_server::start(config.clone()).expect("server starts");
    let host = Client::join(&server, true);
    // Al entrar llega la hora del mundo: uno nuevo empieza por la mañana y con el día en marcha.
    let initial = time(&host);
    assert!(initial.day_length > 0.0 && (initial.hours - WorldTime::default().hours).abs() < 0.1);

    let guest = Client::join(&server, false);
    time(&guest);

    // Por consola: se para el tiempo y se pone la hora; todos se enteran.
    let (text, ok) = host.command("time cycle off");
    assert!(ok && text.contains("stopped"), "{text}");
    let set = WorldTime { hours: 18.5, day_length: 0.0, day: 0 };
    let stopped_at_dusk = "Time: day 0, 18:30 (time is stopped); tonight: full moon";
    assert_eq!(host.command("time set 18:30"), (stopped_at_dusk.into(), true));
    assert_eq!(
        guest.wait("time for the guest", |msg| match msg {
            ServerMsg::Time(time) if time.hours == 18.5 => Some(time),
            _ => None,
        }),
        set
    );
    assert_eq!(host.command("time"), (stopped_at_dusk.into(), true));
    let (error, ok) = host.command("time set 99");
    assert!(!ok && error.contains("not a time"), "{error}");

    // Sin consola (los deslizadores del panel): mismo efecto, sin respuesta de texto.
    host.send(ClientMsg::SetTime { hours: Some(3.25), day_length: Some(1.0) });
    let changed = host.wait("slider", |msg| match msg {
        ServerMsg::Time(time) if time.day_length > 0.0 => Some(time),
        _ => None,
    });
    assert_eq!(changed.day_length, 10.0, "a day cannot be shorter than the minimum");
    assert!((changed.hours - 3.25).abs() < 0.2);

    // Un invitado no puede: ni por consola ni con el mensaje.
    let (error, ok) = guest.command("time set noon");
    assert!(!ok && error.contains("Only the host"), "{error}");
    guest.send(ClientMsg::SetTime { hours: Some(12.0), day_length: Some(0.0) });
    host.send(ClientMsg::SetTime { hours: Some(21.0), day_length: Some(0.0) });
    let after = time(&guest);
    assert_eq!(after, WorldTime { hours: 21.0, day_length: 0.0, day: 0 }, "only the host's change arrives");

    // La luna: saltar a la luna nueva cambia el día de esta noche, y todos lo reciben.
    let (text, ok) = host.command("time moon new");
    assert!(ok && text.contains("tonight: new moon"), "{text}");
    let moonless = time(&guest);
    assert_eq!(moonless.moon_phase(), "new moon");
    assert_eq!((moonless.day, moonless.hours), (4, 21.0), "the hour does not change");
    let (text, ok) = host.command("time day 0");
    assert!(ok && text.contains("full moon"), "{text}");
    assert!(!guest.command("time moon full").1, "a guest cannot change the moon either");
    host.command("time moon new");

    // Al reiniciar, el mundo sigue en la hora (y el día) en que se dejó.
    server.shutdown();
    let server = kuravoxel_server::start(config).expect("server restarts");
    let host = Client::join(&server, true);
    assert_eq!(time(&host), WorldTime { hours: 21.0, day_length: 0.0, day: 4 });
    server.shutdown();
    let _ = std::fs::remove_dir_all(world_dir);
}
