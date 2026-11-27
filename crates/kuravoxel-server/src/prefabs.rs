//! Herramientas de construcción en el servidor: portapapeles, prefabs y deshacer.
//!
//! El cliente solo dice qué caja y dónde; aquí se copia del mundo con autoridad, se
//! leen y escriben los archivos de `content/<paquete>/prefabs/` y se cambia el mundo.
//! Cada cambio guarda cómo estaban sus chunks (son `Arc` inmutables: no cuesta nada) para
//! poder deshacerlo.

use std::sync::Arc;

use glam::IVec3;
use kuravoxel_common::block::Block;
use kuravoxel_common::chunk::Chunk;
use kuravoxel_common::prefab::{self, Prefab};
use kuravoxel_common::protocol::{PlayerId, PrefabOp, ServerMsg};

use crate::Server;

/// Cambios que cada jugador puede deshacer.
const MAX_UNDO: usize = 32;
/// Distancia máxima, en voxeles, entre el jugador y lo que copia o coloca.
const MAX_DISTANCE: f64 = 512.0;

/// Esquinas ordenadas y tamaño de una caja enviada por un cliente.
fn region(a: [i32; 3], b: [i32; 3]) -> Result<(IVec3, IVec3), String> {
    let (a, b) = (IVec3::from_array(a), IVec3::from_array(b));
    let (min, max) = (a.min(b), a.max(b));
    // Antes de restar: unas esquinas absurdas no deben desbordar.
    if (max.as_i64vec3() - min.as_i64vec3()).max_element() >= prefab::MAX_PREFAB_SIDE as i64 {
        return Err(format!("That box is too big: the limit is {} blocks per side.", prefab::MAX_PREFAB_SIDE));
    }
    prefab::check_size(max - min + IVec3::ONE)?;
    Ok((min, max))
}

fn describe(prefab: &Prefab) -> String {
    let [x, y, z] = prefab.size;
    format!("{x}x{y}x{z}, {} blocks", prefab.block_count())
}

impl Server {
    /// Ejecuta una orden de construcción de un jugador. Devuelve el texto que verá;
    /// `Err` si no se hizo.
    pub(crate) fn prefab_op(&mut self, id: PlayerId, op: PrefabOp) -> Result<String, String> {
        let player = self.players.get(&id).filter(|player| player.session.is_some());
        let Some(player) = player else { return Err("Not in the world yet.".into()) };
        if !player.operator {
            return Err("Only the host of this world can use the building tools.".into());
        }
        let eye = player.session.as_ref().map(|session| session.pos).unwrap_or_default();
        let near = |pos: IVec3| pos.as_dvec3().distance(eye) <= MAX_DISTANCE;
        let content_dir = self.config.content_dir.clone();
        let file = |name: &str| {
            let (pack, name) = prefab::parse_id(name).ok_or(
                "Prefab names use lowercase letters, numbers, _ and -, with / for folders (trees/oak).",
            )?;
            Ok::<_, String>((format!("{pack}:{name}"), prefab::path_of(&content_dir, &pack, &name)))
        };

        match op {
            PrefabOp::Copy { min, max } => {
                let (min, max) = region(min, max)?;
                let prefab = self.capture(min, max)?;
                let text = format!("Copied {}.", describe(&prefab));
                self.set_clipboard(id, prefab, String::new());
                Ok(text)
            }
            PrefabOp::Cut { min, max } => {
                let (min, max) = region(min, max)?;
                let prefab = self.capture(min, max)?;
                let text = format!("Cut {}.", describe(&prefab));
                self.set_clipboard(id, prefab, String::new());
                let edited = self.world.fill(min, max, Block::AIR);
                self.finish_edit(id, edited.changed, edited.before);
                Ok(text)
            }
            PrefabOp::Save { name, min, max } => {
                let (id_text, base) = file(&name)?;
                let (min, max) = region(min, max)?;
                let prefab = self.capture(min, max)?;
                prefab.save(&base).map_err(|error| format!("Could not save the prefab: {error:#}"))?;
                log::info!("prefab saved: {id_text} ({})", describe(&prefab));
                let text = format!("Saved {id_text} ({}).", describe(&prefab));
                self.set_clipboard(id, prefab, id_text);
                self.send_prefab_list(id);
                Ok(text)
            }
            PrefabOp::Load { name } => {
                let (id_text, base) = file(&name)?;
                if !base.with_extension("toml").is_file() {
                    return Err(format!("There is no prefab called {id_text}."));
                }
                let prefab = Prefab::load(&base).map_err(|error| format!("Could not load {id_text}: {error:#}"))?;
                let mut text = format!("Loaded {id_text} ({}).", describe(&prefab));
                let unknown = prefab.unknown_blocks(&self.content);
                if !unknown.is_empty() {
                    text += &format!(" Unknown blocks will be skipped: {}.", unknown.join(", "));
                }
                self.set_clipboard(id, prefab, id_text);
                Ok(text)
            }
            PrefabOp::Delete { name } => {
                let (id_text, base) = file(&name)?;
                match prefab::delete(&base) {
                    Ok(true) => {
                        log::info!("prefab deleted: {id_text}");
                        self.send_prefab_list(id);
                        Ok(format!("Deleted {id_text}."))
                    }
                    Ok(false) => Err(format!("There is no prefab called {id_text}.")),
                    Err(error) => Err(format!("Could not delete {id_text}: {error:#}")),
                }
            }
            PrefabOp::List => {
                // La lista es la respuesta: no hace falta decir nada más.
                self.send_prefab_list(id);
                Ok(String::new())
            }
            PrefabOp::Paste { pos, turns, air } => {
                let clipboard = self.players[&id].session.as_ref().and_then(|session| session.clipboard.clone());
                let Some(clipboard) = clipboard else {
                    return Err("The clipboard is empty: copy a selection or load a prefab first.".into());
                };
                let prefab = match turns % 4 {
                    0 => clipboard,
                    turns => Arc::new(clipboard.rotated(turns, &self.content)),
                };
                let pos = IVec3::from_array(pos);
                if !near(pos) {
                    return Err("That is too far away.".into());
                }
                let origin = pos - prefab.anchor();
                let max = origin + prefab.size() - IVec3::ONE;
                if !self.world.region_loaded(origin, max) {
                    return Err("Part of that area is not loaded: get closer to it.".into());
                }
                let blocks = prefab.resolve(&self.content);
                let edited = self.world.edit_region(origin, max, |at, current| {
                    match blocks[prefab.get(at - origin) as usize] {
                        Some(Block::AIR) if !air => current,
                        Some(block) => block,
                        // Un bloque que este contenido no tiene: se deja lo que hubiera.
                        None => current,
                    }
                });
                self.finish_edit(id, edited.changed, edited.before);
                Ok(format!("Pasted {}.", describe(&prefab)))
            }
            PrefabOp::Fill { min, max, block } => {
                let block = Block(block);
                if !self.content.is_known(block) {
                    return Err("That block does not exist.".into());
                }
                let (min, max) = region(min, max)?;
                if !near(min) || !self.world.region_loaded(min, max) {
                    return Err("Part of that area is not loaded: get closer to it.".into());
                }
                let size = max - min + IVec3::ONE;
                let edited = self.world.fill(min, max, block);
                self.finish_edit(id, edited.changed, edited.before);
                let name = self.content.state_name(block).unwrap_or_else(|| "air".into());
                Ok(format!("Filled {} blocks with {name}.", size.x * size.y * size.z))
            }
            PrefabOp::Undo => {
                let session = self.players.get_mut(&id).and_then(|player| player.session.as_mut());
                let Some(before) = session.and_then(|session| session.undo.pop()) else {
                    return Err("Nothing to undo.".into());
                };
                let changed = self.world.restore(before);
                self.resend_chunks(&changed);
                Ok("Undone.".into())
            }
        }
    }

    /// Copia del mundo la caja `min..=max`.
    fn capture(&self, min: IVec3, max: IVec3) -> Result<Prefab, String> {
        if !self.world.region_loaded(min, max) {
            return Err("Part of that area is not loaded: get closer to it.".into());
        }
        let size = max - min + IVec3::ONE;
        Ok(Prefab::capture(size, &self.content, |local| {
            self.world.block_at(min + local).unwrap_or(Block::AIR)
        }))
    }

    /// Deja un prefab en el portapapeles del jugador y se lo envía para la vista previa.
    fn set_clipboard(&mut self, id: PlayerId, prefab: Prefab, name: String) {
        let prefab = Arc::new(prefab);
        if let Some(session) = self.players.get_mut(&id).and_then(|player| player.session.as_mut()) {
            session.clipboard = Some(prefab.clone());
        }
        self.send(id, ServerMsg::Clipboard { prefab, name });
    }

    fn send_prefab_list(&mut self, id: PlayerId) {
        let list = prefab::list(&self.config.content_dir);
        self.send(id, ServerMsg::PrefabList(list));
    }

    /// Tras cambiar el mundo: lo reenvía a quien lo vea y lo apunta para deshacer.
    fn finish_edit(&mut self, id: PlayerId, changed: Vec<IVec3>, before: Vec<(IVec3, Arc<Chunk>)>) {
        self.resend_chunks(&changed);
        if before.is_empty() {
            return;
        }
        if let Some(session) = self.players.get_mut(&id).and_then(|player| player.session.as_mut()) {
            session.undo.push(before);
            if session.undo.len() > MAX_UNDO {
                session.undo.remove(0);
            }
        }
    }
}
