//! Prueba de punta a punta de las construcciones en el terreno lejano: lo que se
//! construye aparece poco después en las teselas, se quita al deshacerlo y sigue ahí
//! tras reiniciar el servidor.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kuravoxel_common::block::Block;
use kuravoxel_common::lod::LodTile;
use kuravoxel_common::protocol::{ClientMsg, ServerMsg, PROTOCOL_VERSION};
use kuravoxel_server::{ServerConfig, ServerHandle};

struct Client {
    server: ServerHandle,
    id: u32,
    incoming: Receiver<ServerMsg>,
    brick: Block,
}

impl Client {
    fn join(server: &ServerHandle) -> Self {
        let (tx, incoming) = channel();
        let id = server.connect_operator(Box::new(move |msg| tx.send(msg).is_ok()));
        let mut client = Self { server: server.clone(), id, incoming, brick: Block::AIR };
        client.send(ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            name: "Host".into(),
            view_distance: 2,
            far_distance: 12,
        });
        client.brick = client.wait("content", |msg| match msg {
            ServerMsg::Content(content) => content.block_id("base:stone_bricks"),
            _ => None,
        });
        client
    }

    fn send(&self, msg: ClientMsg) {
        self.server.message(self.id, msg);
    }

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

    /// Repite un comando hasta que el servidor lo acepta (los chunks tardan en llegar).
    fn command(&self, line: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.send(ClientMsg::Command { line: line.into() });
            let (text, ok) = self.wait(line, |msg| match msg {
                ServerMsg::CommandOutput { text, ok } => Some((text, ok)),
                _ => None,
            });
            if ok {
                return;
            }
            assert!(Instant::now() < deadline, "{line}: {text}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Espera una tesela del nivel más fino que tenga (o ya no tenga) ladrillo encima.
    fn wait_tile(&self, what: &str, with_brick: bool) -> Arc<LodTile> {
        let brick = self.brick;
        self.wait(what, |msg| match msg {
            ServerMsg::LodTile(tile)
                if tile.key == (1, 0, 0) && tile.extras.iter().any(|e| e.block == brick) == with_brick =>
            {
                Some(tile)
            }
            _ => None,
        })
    }
}

fn start(world_dir: &PathBuf) -> ServerHandle {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    kuravoxel_server::start(ServerConfig {
        seed: 4242,
        world_dir: world_dir.clone(),
        content_dir: workspace.join("content"),
        ..Default::default()
    })
    .expect("server starts")
}

#[test]
fn buildings_show_up_in_the_far_terrain() {
    let world_dir = std::env::temp_dir().join(format!("kuravoxel-builds-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&world_dir);
    let server = start(&world_dir);
    let host = Client::join(&server);

    // Una torre de 4×4 y 20 m junto al punto de aparición, que está en (0, 0). Es alta de
    // sobra: el suelo bajo ella puede no estar a la altura del punto de aparición.
    host.command("fill 8 ~0 8 11 ~19 11 base:stone_bricks");
    let tile = host.wait_tile("tile with the tower", true);
    let tower: Vec<_> = tile.extras.iter().filter(|e| e.block == host.brick).collect();
    // Con celdas de 2 m son 2×2 columnas, en las celdas (4, 4)-(5, 5) más la de borde.
    assert_eq!(tower.len(), 4, "tower cells");
    let mut cells: Vec<u16> = tower.iter().map(|e| e.column).collect();
    cells.sort_unstable();
    assert_eq!(cells, [5 + 5 * 34, 6 + 5 * 34, 5 + 6 * 34, 6 + 6 * 34]);
    assert!(tower.iter().all(|e| e.to - e.from >= 4), "the tower stands well over the ground");

    // Al quitarla, la tesela vuelve a ser la del generador.
    host.command("fill 8 ~0 8 11 ~19 11 air");
    host.wait_tile("tile without the tower", false);

    // Se vuelve a poner, se guarda y se reinicia: sigue en el terreno lejano.
    host.command("fill 8 ~0 8 11 ~19 11 base:stone_bricks");
    host.wait_tile("tile with the tower again", true);
    host.command("save");
    drop(host);
    server.shutdown();

    let server = start(&world_dir);
    let host = Client::join(&server);
    host.wait_tile("tile with the tower after a restart", true);
    server.shutdown();
    let _ = std::fs::remove_dir_all(&world_dir);
}
