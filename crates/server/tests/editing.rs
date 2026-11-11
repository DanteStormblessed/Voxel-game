//! Prueba de punta a punta del servidor: un cliente en memoria edita bloques, el servidor
//! valida, difunde y guarda, y tras reiniciar el mundo conserva los cambios.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use voxel_common::chunk::Chunk;
use voxel_common::content::ContentData;
use voxel_common::protocol::{ClientMsg, ServerMsg, PROTOCOL_VERSION};
use voxel_server::{ServerConfig, ServerHandle};

struct Client {
    server: ServerHandle,
    id: u32,
    incoming: Receiver<ServerMsg>,
    content: Arc<ContentData>,
    chunks: HashMap<[i32; 3], Arc<Chunk>>,
    changes: Vec<([i32; 3], u8)>,
}

impl Client {
    fn join(config: ServerConfig) -> Self {
        let server = voxel_server::start(config).expect("server starts");
        let (tx, incoming) = channel();
        let id = server.connect(Box::new(move |msg| tx.send(msg).is_ok()));
        let client = Self {
            server,
            id,
            incoming,
            content: Arc::default(),
            chunks: HashMap::new(),
            changes: Vec::new(),
        };
        client.send(ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            name: "Tester".into(),
            view_distance: 3,
            far_distance: 3,
        });
        client
    }

    fn send(&self, msg: ClientMsg) {
        self.server.message(self.id, msg);
    }

    /// Procesa mensajes hasta que `done` se cumple (o falla por tiempo).
    fn pump_until(&mut self, what: &str, mut done: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(self) {
            let wait = deadline.saturating_duration_since(Instant::now());
            match self.incoming.recv_timeout(wait).unwrap_or_else(|_| panic!("timed out: {what}")) {
                ServerMsg::Content(content) => self.content = content,
                ServerMsg::Chunk { pos, chunk } => {
                    self.chunks.insert(pos, chunk);
                }
                ServerMsg::BlockChange { pos, block } => self.changes.push((pos, block)),
                _ => {}
            }
        }
    }

    fn block(&self, x: i32, y: i32, z: i32) -> Option<u8> {
        let chunk = self.chunks.get(&[x >> 5, y >> 5, z >> 5])?;
        Some(chunk.get((x & 31) as usize, (y & 31) as usize, (z & 31) as usize).0)
    }

    /// Altura del bloque sólido más alto de una columna, según los chunks recibidos.
    fn ground(&self, x: i32, z: i32) -> Option<i32> {
        (-64..160).rev().find(|&y| self.block(x, y, z).is_some_and(|block| block != 0))
    }
}

#[test]
fn edits_are_validated_broadcast_and_saved() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    // VOXEL_TEST_WORLD permite conservar el mundo resultante para inspeccionarlo.
    let world_dir = std::env::var_os("VOXEL_TEST_WORLD")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("voxel-test-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&world_dir);
    let config = ServerConfig {
        seed: 1337,
        world_dir: world_dir.clone(),
        content_dir: workspace.join("content"),
        trust_movement: true,
        ..Default::default()
    };

    let mut client = Client::join(config.clone());
    client.pump_until("initial chunks", |c| (-1..=4).all(|y| c.chunks.contains_key(&[0, y, 0])));
    let ground = client.ground(0, -3).expect("ground under the build site");
    let brick = client.content.block_id("base:leaves_orange").expect("block exists").0;

    // El jugador baja junto al suelo y levanta una torre de 4 bloques.
    client.send(ClientMsg::Move { pos: [0.5, ground as f64 + 3.0, 0.5], yaw: 0.0, pitch: 0.0 });
    let tower: Vec<[i32; 3]> = (1..=4).map(|dy| [0, ground + dy, -3]).collect();
    for &pos in &tower {
        client.send(ClientMsg::SetBlock { pos, block: brick });
    }
    client.pump_until("tower confirmed", |c| c.changes.len() >= tower.len());
    assert_eq!(client.changes, tower.iter().map(|&pos| (pos, brick)).collect::<Vec<_>>());

    // Fuera de alcance: el servidor no lo aplica y devuelve el bloque real (aire).
    client.changes.clear();
    let far = [40, ground + 30, 0];
    client.send(ClientMsg::SetBlock { pos: far, block: brick });
    // Un número de bloque que no existe tampoco se acepta.
    client.send(ClientMsg::SetBlock { pos: [0, ground + 6, -3], block: 250 });
    client.pump_until("rejections", |c| c.changes.len() >= 2);
    assert_eq!(client.changes, vec![(far, 0), ([0, ground + 6, -3], 0)]);

    // Romper el bloque de arriba de la torre.
    client.changes.clear();
    client.send(ClientMsg::SetBlock { pos: tower[3], block: 0 });
    client.pump_until("break confirmed", |c| !c.changes.is_empty());
    assert_eq!(client.changes, vec![(tower[3], 0)]);

    client.server.shutdown();
    assert!(world_dir.join("world.toml").exists());

    // Segunda sesión sobre el mismo mundo: los cambios siguen ahí.
    let mut client = Client::join(config);
    client.pump_until("chunks after restart", |c| {
        (-1..=4).all(|y| c.chunks.contains_key(&[0, y, -1]))
    });
    for pos in &tower[..3] {
        assert_eq!(client.block(pos[0], pos[1], pos[2]), Some(brick), "saved block at {pos:?}");
    }
    assert_eq!(client.block(tower[3][0], tower[3][1], tower[3][2]), Some(0));
    assert_eq!(client.ground(0, -3), Some(ground + 3));
    client.server.shutdown();

    if std::env::var_os("VOXEL_TEST_WORLD").is_none() {
        let _ = std::fs::remove_dir_all(&world_dir);
    }
}
