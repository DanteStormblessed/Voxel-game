//! Prueba de punta a punta de los ítems sueltos: un jugador tira un ítem, todos lo ven
//! aparecer, cae, y al cabo de un momento lo recoge quien está encima.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use kuravoxel_common::protocol::{ClientMsg, ServerMsg, PROTOCOL_VERSION};
use kuravoxel_server::ServerConfig;

fn wait<T>(incoming: &Receiver<ServerMsg>, what: &str, mut pick: impl FnMut(ServerMsg) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let msg = incoming.recv_timeout(left).unwrap_or_else(|_| panic!("timed out: {what}"));
        if let Some(found) = pick(msg) {
            return found;
        }
    }
}

#[test]
fn dropped_items_fall_and_are_picked_up() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let world_dir = std::env::temp_dir().join(format!("kuravoxel-drops-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&world_dir);
    let server = kuravoxel_server::start(ServerConfig {
        seed: 4242,
        world_dir: world_dir.clone(),
        content_dir: workspace.join("content"),
        trust_movement: true,
        ..Default::default()
    })
    .expect("server starts");

    let (tx, incoming) = channel();
    let id = server.connect(Box::new(move |msg| tx.send(msg).is_ok()));
    server.message(id, ClientMsg::Hello {
        version: PROTOCOL_VERSION,
        name: "Thrower".into(),
        view_distance: 3,
        far_distance: 3,
    });
    let spawn = wait(&incoming, "welcome", |msg| match msg {
        ServerMsg::Welcome { spawn, .. } => Some(spawn),
        _ => None,
    });
    let content = wait(&incoming, "content", |msg| match msg {
        ServerMsg::Content(content) => Some(content),
        _ => None,
    });
    let lamp = content.item_id("base:lamp").expect("lamp item").0;

    // Se tira hacia arriba: vuelve a caer junto al jugador, que lo recoge.
    server.message(id, ClientMsg::DropItem { item: lamp, dir: [0.0, 1.0, 0.0] });
    let (drop, item, pos) = wait(&incoming, "drop spawned", |msg| match msg {
        ServerMsg::DropSpawned { id, item, pos } => Some((id, item, pos)),
        _ => None,
    });
    assert_eq!(item, lamp);
    assert!((pos[1] - spawn[1]).abs() < 1.0, "it leaves from the player's hand: {pos:?}");

    let started = Instant::now();
    let picked = wait(&incoming, "picked up", |msg| match msg {
        ServerMsg::PickedUp { item } => Some(item),
        _ => None,
    });
    assert_eq!(picked, lamp);
    assert!(started.elapsed() >= Duration::from_millis(1000), "not before the pickup delay");
    wait(&incoming, "drop removed", |msg| match msg {
        ServerMsg::DropRemoved { id } if id == drop => Some(()),
        _ => None,
    });

    // Un ítem que no existe no aparece.
    server.message(id, ClientMsg::DropItem { item: 60_000, dir: [0.0, 0.0, -1.0] });
    server.message(id, ClientMsg::Command { line: "seed".into() });
    wait(&incoming, "nothing else", |msg| match msg {
        ServerMsg::DropSpawned { .. } => panic!("an unknown item was dropped"),
        ServerMsg::CommandOutput { .. } => Some(()),
        _ => None,
    });

    server.shutdown();
    let _ = std::fs::remove_dir_all(world_dir);
}
