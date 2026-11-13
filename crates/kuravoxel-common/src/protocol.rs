//! Protocolo cliente ↔ servidor.
//!
//! El servidor es la autoridad: decide qué chunks ve cada jugador, dónde está cada
//! entidad y qué movimientos acepta. El cliente pide, predice y dibuja.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::chunk::Chunk;
use crate::content::ContentData;
use crate::lod::{LodTile, TileKey};

pub const PROTOCOL_VERSION: u32 = 3;
/// Distancia máxima, en voxeles, a la que un jugador puede poner o romper un bloque.
pub const REACH: f64 = 8.0;
pub const DEFAULT_PORT: u16 = 28700;
/// Ticks de simulación por segundo del servidor.
pub const TICK_RATE: u32 = 20;

/// Chunks que el servidor envía más allá de la distancia de voxeles pedida: dan vecinos
/// para mallar y margen para que al andar el borde ya esté listo.
pub const LOAD_MARGIN: i32 = 4;
/// Radio vertical de chunks alrededor del jugador.
pub const VERTICAL_RADIUS: i32 = 4;
/// Chunks extra que se conservan antes de descargar, para no recargar al cruzar un borde.
pub const UNLOAD_MARGIN: i32 = 2;

pub type PlayerId = u32;

/// Estado de un jugador tal como lo ven los demás.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PlayerState {
    pub id: PlayerId,
    /// Posición de los ojos.
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Serialize, Deserialize)]
pub enum ClientMsg {
    /// Primer mensaje de toda conexión.
    Hello {
        version: u32,
        name: String,
        /// Distancia de voxeles reales pedida, en chunks. El servidor puede recortarla.
        view_distance: u8,
        /// Distancia total con terreno lejano, en chunks.
        far_distance: u8,
    },
    /// Posición propuesta por el cliente. El servidor la valida. No fiable: si se pierde
    /// una, la siguiente la sustituye.
    Move { pos: [f64; 3], yaw: f32, pitch: f32 },
    /// Petición de cambiar un voxel (0 = romper). El cliente ya lo muestra cambiado; si
    /// el servidor no lo acepta, responde con el bloque real.
    SetBlock { pos: [i32; 3], block: u8 },
}

#[derive(Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        id: PlayerId,
        spawn: [f64; 3],
        /// Distancias concedidas, ya recortadas a los límites del servidor.
        view_distance: u8,
        far_distance: u8,
    },
    /// La conexión se rechaza o se ha cerrado.
    Disconnect { reason: String },
    /// Contenido activo (bloques...). Llega tras el `Welcome` y cada vez que el servidor
    /// lo recarga; sustituye por completo al anterior.
    Content(Arc<ContentData>),
    Chunk { pos: [i32; 3], chunk: Arc<Chunk> },
    UnloadChunks(Vec<[i32; 3]>),
    /// Un voxel ha cambiado. También sirve para corregir una edición rechazada.
    BlockChange { pos: [i32; 3], block: u8 },
    LodTile(Arc<LodTile>),
    UnloadLod(Vec<TileKey>),
    PlayerJoined { id: PlayerId, name: String },
    PlayerLeft { id: PlayerId },
    /// Foto de las demás entidades en un tick. No fiable.
    Entities { tick: u32, players: Vec<PlayerState> },
    /// El servidor no acepta el movimiento del cliente y lo recoloca.
    Teleport { pos: [f64; 3] },
}

impl ClientMsg {
    /// Los mensajes no fiables viajan como datagramas: sin reenvío ni orden.
    pub fn is_unreliable(&self) -> bool {
        matches!(self, ClientMsg::Move { .. })
    }
}

impl ServerMsg {
    pub fn is_unreliable(&self) -> bool {
        matches!(self, ServerMsg::Entities { .. })
    }
}
