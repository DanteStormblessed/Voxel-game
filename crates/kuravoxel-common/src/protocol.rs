//! Protocolo cliente ↔ servidor.
//!
//! El servidor es la autoridad: decide qué chunks ve cada jugador, dónde está cada
//! entidad y qué movimientos acepta. El cliente pide, predice y dibuja.

use std::sync::Arc;

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::chunk::Chunk;
use crate::content::ContentData;
use crate::lod::{LodTile, TileKey};

pub const PROTOCOL_VERSION: u32 = 7;
/// Longitud máxima, en bytes, de una línea de consola enviada al servidor.
pub const MAX_COMMAND_LENGTH: usize = 256;
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
/// Identificador de un ítem suelto en el mundo.
pub type DropId = u32;

/// Posición de un ítem suelto que se está moviendo.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct DropState {
    pub id: DropId,
    /// Centro de la base del ítem.
    pub pos: [f64; 3],
}

/// Zona jugable del mundo (en X y Z; la altura no tiene límite). Fuera de ella el terreno
/// se sigue generando para que el horizonte no acabe en un corte, pero nadie puede ir.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldBounds {
    /// Esquinas mínima y máxima, como (x, z) en metros.
    pub min: [f64; 2],
    pub max: [f64; 2],
}

impl WorldBounds {
    /// Sin límites (para pruebas y herramientas).
    pub const UNLIMITED: WorldBounds =
        WorldBounds { min: [f64::NEG_INFINITY; 2], max: [f64::INFINITY; 2] };

    /// `pos` está dentro, con `margin` metros de tolerancia.
    pub fn contains(&self, pos: DVec3, margin: f64) -> bool {
        pos.x >= self.min[0] - margin
            && pos.x <= self.max[0] + margin
            && pos.z >= self.min[1] - margin
            && pos.z <= self.max[1] + margin
    }

    /// La posición más cercana dentro, dejando `inset` metros hasta el borde.
    pub fn clamp(&self, pos: DVec3, inset: f64) -> DVec3 {
        let axis = |v: f64, lo: f64, hi: f64| {
            if lo + inset > hi - inset { (lo + hi) * 0.5 } else { v.clamp(lo + inset, hi - inset) }
        };
        DVec3::new(
            axis(pos.x, self.min[0], self.max[0]),
            pos.y,
            axis(pos.z, self.min[1], self.max[1]),
        )
    }
}

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
    SetBlock { pos: [i32; 3], block: u16 },
    /// Línea de consola para que la ejecute el servidor (`tp`, `seed`...). El servidor
    /// decide si quien la envía tiene permiso y responde con `CommandOutput`.
    Command { line: String },
    /// Tirar un ítem (índice en `ContentData::items`) hacia donde se mira.
    DropItem { item: u16, dir: [f32; 3] },
}

#[derive(Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        id: PlayerId,
        spawn: [f64; 3],
        /// Zona jugable: el cliente no deja salir de ella a su jugador.
        bounds: WorldBounds,
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
    BlockChange { pos: [i32; 3], block: u16 },
    LodTile(Arc<LodTile>),
    UnloadLod(Vec<TileKey>),
    PlayerJoined { id: PlayerId, name: String },
    PlayerLeft { id: PlayerId },
    /// Foto de las demás entidades en un tick: jugadores y los ítems sueltos que se
    /// están moviendo. No fiable.
    Entities { tick: u32, players: Vec<PlayerState>, drops: Vec<DropState> },
    /// Ha aparecido un ítem suelto (o el cliente acaba de llegar y ya estaba ahí).
    DropSpawned { id: DropId, item: u16, pos: [f64; 3] },
    /// Un ítem suelto desaparece (recogido o caducado).
    DropRemoved { id: DropId },
    /// Este jugador ha recogido un ítem.
    PickedUp { item: u16 },
    /// El servidor no acepta el movimiento del cliente y lo recoloca.
    Teleport { pos: [f64; 3] },
    /// Respuesta a un `Command`: texto para la consola (puede tener varias líneas) y si
    /// el comando se ejecutó.
    CommandOutput { text: String, ok: bool },
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
