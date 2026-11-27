//! Prueba de punta a punta de las herramientas de construcción: copiar una caja, pegarla
//! girada, deshacer, cortar, y guardar, listar, cargar y borrar prefabs en disco.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::IVec3;
use kuravoxel_common::prefab::{Prefab, PrefabInfo};
use kuravoxel_common::protocol::{ClientMsg, PrefabOp, ServerMsg, PROTOCOL_VERSION};
use kuravoxel_server::{ServerConfig, ServerHandle};

struct Client {
    server: ServerHandle,
    id: u32,
    incoming: Receiver<ServerMsg>,
    spawn: [f64; 3],
}

impl Client {
    fn join(server: &ServerHandle, operator: bool) -> Self {
        let (tx, incoming) = channel();
        let sink = Box::new(move |msg| tx.send(msg).is_ok());
        let id = if operator { server.connect_operator(sink) } else { server.connect(sink) };
        let mut client = Self { server: server.clone(), id, incoming, spawn: [0.0; 3] };
        client.send(ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            name: "Builder".into(),
            view_distance: 3,
            far_distance: 3,
        });
        client.spawn = client.wait("welcome", |msg| match msg {
            ServerMsg::Welcome { spawn, .. } => Some(spawn),
            _ => None,
        });
        client
    }

    fn send(&self, msg: ClientMsg) {
        self.server.message(self.id, msg);
    }

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

    /// Ejecuta una orden y devuelve su resultado junto con lo que llegó antes de él.
    fn op(&self, op: PrefabOp) -> ((String, bool), Vec<ServerMsg>) {
        let what = format!("{op:?}");
        self.send(ClientMsg::Prefab(op));
        let mut effects = Vec::new();
        let result = self.wait(&what, |msg| match msg {
            ServerMsg::PrefabResult { text, ok } => Some((text, ok)),
            other => {
                effects.push(other);
                None
            }
        });
        (result, effects)
    }

    /// Copia una caja y devuelve lo que queda en el portapapeles.
    fn copy(&self, min: IVec3, max: IVec3) -> Arc<Prefab> {
        let ((text, ok), effects) = self.op(PrefabOp::Copy { min: min.to_array(), max: max.to_array() });
        assert!(ok, "{text}");
        clipboard(effects).expect("copying fills the clipboard").0
    }

    fn command(&self, line: &str) {
        self.send(ClientMsg::Command { line: line.into() });
        let (text, ok) = self.wait(line, |msg| match msg {
            ServerMsg::CommandOutput { text, ok } => Some((text, ok)),
            _ => None,
        });
        assert!(ok, "{line}: {text}");
    }
}

fn clipboard(effects: Vec<ServerMsg>) -> Option<(Arc<Prefab>, String)> {
    effects.into_iter().find_map(|msg| match msg {
        ServerMsg::Clipboard { prefab, name } => Some((prefab, name)),
        _ => None,
    })
}

/// Nombre del bloque de cada voxel no vacío, por posición.
fn blocks(prefab: &Prefab) -> Vec<([i32; 3], String)> {
    let size = prefab.size();
    let mut out = Vec::new();
    for y in 0..size.y {
        for z in 0..size.z {
            for x in 0..size.x {
                let index = prefab.get(IVec3::new(x, y, z));
                if index != 0 {
                    out.push(([x, y, z], prefab.palette[index as usize].clone()));
                }
            }
        }
    }
    out
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
fn selections_are_copied_pasted_saved_and_undone() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!("kuravoxel-prefabs-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Copia del contenido: los prefabs de la prueba no deben acabar en el del proyecto.
    let content_dir = dir.join("content");
    copy_dir(&workspace.join("content"), &content_dir);
    let server = kuravoxel_server::start(ServerConfig {
        seed: 4242,
        world_dir: dir.join("world"),
        content_dir: content_dir.clone(),
        ..Default::default()
    })
    .expect("server starts");

    let host = Client::join(&server, true);
    let content = host.wait("content", |msg| match msg {
        ServerMsg::Content(content) => Some(content),
        _ => None,
    });

    // Se trabaja en el aire, sobre el punto de aparición, en una zona vaciada antes.
    let base = IVec3::new(host.spawn[0] as i32 - 8, host.spawn[1] as i32 + 6, host.spawn[2] as i32 - 8);
    let (area_min, area_max) = (base - IVec3::ONE, base + IVec3::new(16, 6, 16));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let fill = PrefabOp::Fill { min: area_min.to_array(), max: area_max.to_array(), block: 0 };
        let ((text, ok), _) = host.op(fill);
        if ok {
            break;
        }
        assert!(text.contains("not loaded"), "{text}");
        assert!(Instant::now() < deadline, "the player's chunks never loaded");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Una pieza de 3×2×2 con bloques orientables.
    let at = |offset: [i32; 3]| base + IVec3::from_array(offset);
    for (offset, block) in [
        ([0, 0, 0], "base:stone"),
        ([2, 0, 0], "base:wood[axis=x]"),
        ([0, 1, 1], "base:kiln[facing=north]"),
    ] {
        let pos = at(offset);
        host.command(&format!("setblock {} {} {} {block}", pos.x, pos.y, pos.z));
    }
    let (min, max) = (base, at([2, 1, 1]));

    // Copiar: el portapapeles llega al cliente con nombres de estado.
    let ((text, ok), effects) = host.op(PrefabOp::Copy { min: max.to_array(), max: min.to_array() });
    assert!(ok && text == "Copied 3x2x2, 3 blocks.", "{text}");
    let (copied, name) = clipboard(effects).expect("the clipboard reaches the client");
    assert_eq!(name, "");
    assert_eq!(
        blocks(&copied),
        [
            ([0, 0, 0], "base:stone".to_string()),
            ([2, 0, 0], "base:wood[axis=x]".to_string()),
            ([0, 1, 1], "base:kiln[facing=north]".to_string()),
        ]
    );

    // Pegar girado un cuarto de vuelta: el anclaje (centro de la base) cae en `target`.
    let target = at([10, 0, 4]);
    let turned = copied.rotated(1, &content);
    let ((text, ok), effects) = host.op(PrefabOp::Paste { pos: target.to_array(), turns: 1, air: false });
    assert!(ok && text.starts_with("Pasted 2x2x3"), "{text}");
    assert!(effects.iter().any(|msg| matches!(msg, ServerMsg::Chunk { .. })), "the changed chunks are resent");
    let origin = target - turned.anchor();
    let pasted = host.copy(origin, origin + turned.size() - IVec3::ONE);
    assert_eq!(blocks(&pasted), blocks(&turned));
    assert!(blocks(&pasted).contains(&([1, 0, 2], "base:wood[axis=z]".to_string())));
    assert!(blocks(&pasted).contains(&([0, 1, 0], "base:kiln[facing=east]".to_string())));

    // Sin `air` los huecos del portapapeles respetan lo que hay; con `air` lo vacían.
    let marker = origin + IVec3::new(0, 0, 1);
    host.command(&format!("setblock {} {} {} base:planks", marker.x, marker.y, marker.z));
    // (Cada comprobación copia, así que antes de pegar se vuelve a copiar la pieza.)
    host.copy(min, max);
    host.op(PrefabOp::Paste { pos: target.to_array(), turns: 1, air: false });
    let kept = host.copy(origin, origin + turned.size() - IVec3::ONE);
    assert!(blocks(&kept).contains(&([0, 0, 1], "base:planks".to_string())));
    host.copy(min, max);
    let ((_, ok), _) = host.op(PrefabOp::Paste { pos: target.to_array(), turns: 1, air: true });
    assert!(ok);
    let cleared = host.copy(origin, origin + turned.size() - IVec3::ONE);
    assert_eq!(blocks(&cleared), blocks(&turned));

    // Deshacer devuelve el mundo paso a paso.
    let ((text, ok), _) = host.op(PrefabOp::Undo);
    assert!(ok && text == "Undone.", "{text}");
    let undone = host.copy(origin, origin + turned.size() - IVec3::ONE);
    assert!(blocks(&undone).contains(&([0, 0, 1], "base:planks".to_string())));

    // Guardar, listar, cargar y borrar un prefab en disco.
    let save = PrefabOp::Save { name: "test/piece".into(), min: min.to_array(), max: max.to_array() };
    let ((text, ok), effects) = host.op(save);
    assert!(ok && text == "Saved base:test/piece (3x2x2, 3 blocks).", "{text}");
    assert!(content_dir.join("base/prefabs/test/piece.toml").is_file());
    assert!(content_dir.join("base/prefabs/test/piece.bin").is_file());
    let listed = effects.iter().find_map(|msg| match msg {
        ServerMsg::PrefabList(list) => Some(list.clone()),
        _ => None,
    });
    // (El contenido del proyecto puede traer ya otros prefabs.)
    let listed = listed.expect("saving sends the new list");
    assert!(listed.contains(&PrefabInfo { id: "base:test/piece".into(), size: [3, 2, 2] }), "{listed:?}");

    let ((text, ok), effects) = host.op(PrefabOp::Load { name: "base:test/piece".into() });
    assert!(ok && text.starts_with("Loaded base:test/piece"), "{text}");
    let (loaded, name) = clipboard(effects).expect("loading fills the clipboard");
    assert_eq!((blocks(&loaded), name.as_str()), (blocks(&copied), "base:test/piece"));

    for bad in ["../outside", "Trees/Oak", ""] {
        let ((text, ok), _) = host.op(PrefabOp::Load { name: bad.into() });
        assert!(!ok && text.contains("Prefab names"), "{bad}: {text}");
    }
    let ((text, ok), _) = host.op(PrefabOp::Load { name: "missing".into() });
    assert!(!ok && text.contains("no prefab called base:missing"), "{text}");

    let ((text, ok), _) = host.op(PrefabOp::Delete { name: "test/piece".into() });
    assert!(ok && text == "Deleted base:test/piece.", "{text}");
    assert!(!content_dir.join("base/prefabs/test/piece.toml").exists());

    // Cortar copia y vacía; deshacerlo lo devuelve.
    let ((text, ok), _) = host.op(PrefabOp::Cut { min: min.to_array(), max: max.to_array() });
    assert!(ok && text == "Cut 3x2x2, 3 blocks.", "{text}");
    assert!(blocks(&host.copy(min, max)).is_empty());
    host.op(PrefabOp::Undo);
    assert_eq!(blocks(&host.copy(min, max)), blocks(&copied));

    // Límites y permisos.
    let huge = PrefabOp::Copy { min: [0, 0, 0], max: [400, 10, 10] };
    let ((text, ok), _) = host.op(huge);
    assert!(!ok && text.contains("limit"), "{text}");
    let guest = Client::join(&server, false);
    let ((text, ok), _) = guest.op(PrefabOp::Copy { min: min.to_array(), max: max.to_array() });
    assert!(!ok && text.contains("Only the host"), "{text}");
    let ((text, ok), _) = guest.op(PrefabOp::Paste { pos: target.to_array(), turns: 0, air: false });
    assert!(!ok && text.contains("Only the host"), "{text}");

    server.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
