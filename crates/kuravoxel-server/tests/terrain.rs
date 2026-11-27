//! Prueba de punta a punta del terreno por datos: el comando `biome`, la recarga en
//! caliente al cambiar un bioma (el mundo se regenera y se reenvía, lo editado se
//! conserva) y que un archivo con errores no tumba nada; la recarga a mano y los perfiles
//! de cada mundo.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::IVec3;
use kuravoxel_common::block::Block;
use kuravoxel_common::chunk::Chunk;
use kuravoxel_common::protocol::{ClientMsg, ServerMsg, TerrainOp, TerrainStatus, PROTOCOL_VERSION};
use kuravoxel_server::{ServerConfig, ServerHandle};

struct Client {
    server: ServerHandle,
    id: u32,
    incoming: Receiver<ServerMsg>,
}

impl Client {
    fn wait<T>(&self, what: &str, mut pick: impl FnMut(ServerMsg) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            let msg = self.incoming.recv_timeout(wait).unwrap_or_else(|_| panic!("timed out: {what}"));
            if let Some(found) = pick(msg) {
                return found;
            }
        }
    }

    fn chunk(&self, what: &str, pos: IVec3, mut accept: impl FnMut(&Chunk) -> bool) -> Arc<Chunk> {
        self.wait(what, |msg| match msg {
            ServerMsg::Chunk { pos: p, chunk } if IVec3::from_array(p) == pos && accept(&chunk) => Some(chunk),
            _ => None,
        })
    }

    fn command(&self, line: &str) -> (String, bool) {
        self.server.message(self.id, ClientMsg::Command { line: line.into() });
        self.wait(line, |msg| match msg {
            ServerMsg::CommandOutput { text, ok } => Some((text, ok)),
            _ => None,
        })
    }
}

/// Voxeles que no son aire.
fn solid(chunk: &Chunk) -> usize {
    let mut count = 0;
    for z in 0..32 {
        for y in 0..32 {
            for x in 0..32 {
                count += (chunk.get(x, y, z) != Block::AIR) as usize;
            }
        }
    }
    count
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_dir(&source, &target);
        } else {
            std::fs::copy(&source, &target).unwrap();
        }
    }
}

#[test]
fn editing_a_biome_file_regenerates_the_world() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!("kuravoxel-terrain-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let content_dir = dir.join("content");
    copy_dir(&workspace.join("content"), &content_dir);
    let server = kuravoxel_server::start(ServerConfig {
        seed: 4242,
        world_dir: dir.join("world"),
        content_dir: content_dir.clone(),
        ..Default::default()
    })
    .expect("server starts");

    let (tx, incoming) = channel();
    let id = server.connect_operator(Box::new(move |msg| tx.send(msg).is_ok()));
    let client = Client { server: server.clone(), id, incoming };
    server.message(id, ClientMsg::Hello {
        version: PROTOCOL_VERSION,
        name: "Geologist".into(),
        view_distance: 3,
        far_distance: 3,
    });
    let spawn = client.wait("welcome", |msg| match msg {
        ServerMsg::Welcome { spawn, .. } => Some(spawn),
        _ => None,
    });
    let brick = client
        .wait("content", |msg| match msg {
            ServerMsg::Content(content) => Some(content),
            _ => None,
        })
        .block_id("base:stone_bricks")
        .expect("brick block");

    // El punto de aparición está en las planicies, en tierra firme.
    let (text, ok) = client.command("biome");
    assert!(ok && text.contains("base:plains") && text.contains("Planicies"), "{text}");
    assert_eq!(text.lines().count(), 10, "the current biome and the nine bands:\n{text}");

    // Voxel 12 m por encima de los pies: aire. Se pone un ladrillo más arriba, en el chunk
    // siguiente (un chunk editado ya no se regenera).
    let feet = IVec3::new(spawn[0].floor() as i32, (spawn[1] - 1.62).round() as i32, spawn[2].floor() as i32);
    let locate = |voxel: IVec3| {
        let chunk = IVec3::new(voxel.x >> 5, voxel.y >> 5, voxel.z >> 5);
        let local = voxel - chunk * 32;
        (chunk, [local.x as usize, local.y as usize, local.z as usize])
    };
    let (air_chunk, [ax, ay, az]) = locate(feet + IVec3::Y * 12);
    let brick_up = (air_chunk.y + 1) * 32 + 8 - feet.y;
    let (brick_chunk, [bx, by, bz]) = locate(feet + IVec3::Y * brick_up);
    let place_brick = format!("setblock ~0 ~{brick_up} ~0 base:stone_bricks");
    let before = client.chunk("the chunk above the spawn point", air_chunk, |_| true);
    assert_eq!(before.get(ax, ay, az), Block::AIR);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !client.command(&place_brick).1 {
        assert!(Instant::now() < deadline, "the chunk of the brick never loaded");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Se levantan las planicies 40 m: el servidor lo nota solo y reenvía los chunks.
    let plains = content_dir.join("base/worldgen/biomes/plains.toml");
    let original = std::fs::read_to_string(&plains).unwrap();
    let raised = original.replace("inputs = [22, \"flat_part\"", "inputs = [62, \"flat_part\"");
    assert_ne!(raised, original, "the plains file has the expected base height");
    std::fs::write(&plains, &raised).unwrap();
    // (Junto a un río el suelo sube menos de 40 m, pero el chunk cambia igualmente.)
    let raised_chunk = client.chunk("the regenerated chunk", air_chunk, |chunk| solid(chunk) != solid(&before));
    assert!(solid(&raised_chunk) > solid(&before), "the ground is higher now");

    // Lo editado no se regenera: el ladrillo sigue donde estaba.
    std::fs::write(&plains, &original).unwrap();
    // Todo se reenvía, en cualquier orden: el terreno vuelve a ser el de antes y el chunk
    // editado llega tal como estaba, con su ladrillo.
    let (mut restored, mut kept) = (false, None);
    client.wait("the original terrain and the edited chunk", |msg| {
        if let ServerMsg::Chunk { pos, chunk } = msg {
            let pos = IVec3::from_array(pos);
            restored |= pos == air_chunk && solid(&chunk) == solid(&before);
            if pos == brick_chunk {
                kept = Some(chunk);
            }
        }
        (restored && kept.is_some()).then_some(())
    });
    assert_eq!(kept.expect("edited chunk").get(bx, by, bz), brick);

    // Un archivo con errores no rompe nada: sigue el terreno anterior.
    std::fs::write(&plains, original.replace("type = \"noise\"", "type = \"nose\"")).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    let (text, ok) = client.command("biome");
    assert!(ok && text.contains("Planicies"), "{text}");
    // Arreglado, vuelve a ser el terreno vigente: tampoco hay nada que regenerar.
    std::fs::write(&plains, &original).unwrap();
    std::thread::sleep(Duration::from_millis(1000));

    // Mover nodos en el lienzo del editor solo cambia `[editor.layout]`: no regenera nada.
    while client.incoming.try_recv().is_ok() {}
    std::fs::write(&plains, format!("{original}\n[editor.layout]\nheight = [400, 20]\n")).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    let resent = client.incoming.try_iter().filter(|msg| matches!(msg, ServerMsg::Chunk { .. })).count();
    assert_eq!(resent, 0, "a layout change does not regenerate the world");

    server.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Servidor sobre `content_dir` y un anfitrión ya dentro, con el chunk que hay justo
/// encima de sus pies (sin recibir aún).
fn join(dir: &Path, content_dir: &Path) -> (ServerHandle, Client, IVec3, TerrainStatus) {
    let server = kuravoxel_server::start(ServerConfig {
        seed: 4242,
        world_dir: dir.join("world"),
        content_dir: content_dir.to_path_buf(),
        ..Default::default()
    })
    .expect("server starts");
    let (tx, incoming) = channel();
    let id = server.connect_operator(Box::new(move |msg| tx.send(msg).is_ok()));
    let client = Client { server: server.clone(), id, incoming };
    server.message(id, ClientMsg::Hello {
        version: PROTOCOL_VERSION,
        name: "Geologist".into(),
        view_distance: 3,
        far_distance: 3,
    });
    let spawn = client.wait("welcome", |msg| match msg {
        ServerMsg::Welcome { spawn, .. } => Some(spawn),
        _ => None,
    });
    let status = client.wait("the terrain status", |msg| match msg {
        ServerMsg::TerrainStatus(status) => Some(status),
        _ => None,
    });
    let feet = IVec3::new(spawn[0].floor() as i32, (spawn[1] - 1.62).round() as i32, spawn[2].floor() as i32);
    let above = IVec3::new(feet.x >> 5, (feet.y + 12) >> 5, feet.z >> 5);
    (server, client, above, status)
}

fn status(client: &Client, what: &str, mut accept: impl FnMut(&TerrainStatus) -> bool) -> TerrainStatus {
    client.wait(what, |msg| match msg {
        ServerMsg::TerrainStatus(status) if accept(&status) => Some(status),
        _ => None,
    })
}

#[test]
fn terrain_changes_can_wait_and_each_world_picks_its_profiles() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!("kuravoxel-profiles-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let content_dir = dir.join("content");
    copy_dir(&workspace.join("content"), &content_dir);
    let (server, client, above, start) = join(&dir, &content_dir);
    let before = client.chunk("the chunk above the spawn point", above, |_| true);
    assert!(start.auto && !start.pending && start.profiles.is_empty(), "{start:?}");

    // Sin recarga automática, un cambio en los archivos espera.
    let (text, ok) = client.command("terrain auto off");
    assert!(ok && text.contains("live reload off"), "{text}");
    let plains = content_dir.join("base/worldgen/biomes/plains.toml");
    let original = std::fs::read_to_string(&plains).unwrap();
    let raised = original.replace("inputs = [22, \"flat_part\"", "inputs = [62, \"flat_part\"");
    assert_ne!(raised, original);
    std::fs::write(&plains, &raised).unwrap();
    status(&client, "the change waiting", |status| status.pending);
    std::thread::sleep(Duration::from_millis(800));
    let resent = client.incoming.try_iter().filter(|msg| matches!(msg, ServerMsg::Chunk { .. })).count();
    assert_eq!(resent, 0, "nothing regenerates until it is applied");
    let (text, ok) = client.command("terrain");
    assert!(ok && text.contains("waiting"), "{text}");

    // Aplicado, el mundo se regenera.
    client.server.message(client.id, ClientMsg::Terrain(TerrainOp::Apply));
    let raised_chunk = client.chunk("the regenerated chunk", above, |chunk| solid(chunk) != solid(&before));
    assert!(solid(&raised_chunk) > solid(&before));

    // El terreno alto pasa a ser un perfil y el archivo por defecto vuelve a ser el de
    // siempre: hay un cambio esperando...
    let profile = content_dir.join("base/worldgen/profiles/plains/high.toml");
    std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
    std::fs::write(&profile, &raised).unwrap();
    std::fs::write(&plains, &original).unwrap();
    status(&client, "the default file waiting", |status| status.pending);
    // ...hasta que este mundo elige el perfil: es justo el terreno que ya tiene.
    let (text, ok) = client.command("terrain profile plains high");
    assert!(ok && text.contains("base:plains") && text.contains("high"), "{text}");
    assert!(!text.contains("waiting"), "{text}");
    client.server.message(client.id, ClientMsg::Terrain(TerrainOp::Status));
    let chosen = status(&client, "the profile chosen", |status| !status.pending);
    assert_eq!(chosen.profiles.get("base:plains").map(String::as_str), Some("high"));
    let (text, ok) = client.command("terrain profiles");
    assert!(ok && text.lines().any(|line| line.contains("base:plains") && line.contains("high")), "{text}");
    let (text, ok) = client.command("terrain profile plains nowhere");
    assert!(!ok && text.contains("no profile"), "{text}");

    // Al reiniciar, el mundo recuerda su perfil (y la recarga vuelve a estar activada).
    server.shutdown();
    let meta = std::fs::read_to_string(dir.join("world/world.toml")).unwrap();
    assert!(meta.contains("[profiles]") && meta.contains("\"base:plains\" = \"high\""), "{meta}");
    let (server, client, _, start) = join(&dir, &content_dir);
    // (El punto de aparición sube con el terreno: se mira el mismo chunk que antes.)
    let again = client.chunk("the same chunk after restarting", above, |_| true);
    assert!(start.auto && start.profiles.get("base:plains").is_some_and(|p| p == "high"), "{start:?}");
    assert_eq!(solid(&again), solid(&raised_chunk), "the world starts with its profile");

    // Volver al archivo por defecto lo regenera al momento (la recarga está activada).
    client.server.message(client.id, ClientMsg::Terrain(TerrainOp::Profile { biome: "base:plains".into(), profile: None }));
    client.chunk("the default terrain again", above, |chunk| solid(chunk) == solid(&before));
    server.shutdown();
    let meta = std::fs::read_to_string(dir.join("world/world.toml")).unwrap();
    assert!(!meta.contains("profiles"), "{meta}");
    let _ = std::fs::remove_dir_all(&dir);
}
