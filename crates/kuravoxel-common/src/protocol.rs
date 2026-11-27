//! Protocolo cliente ↔ servidor.
//!
//! El servidor es la autoridad: decide qué chunks ve cada jugador, dónde está cada
//! entidad y qué movimientos acepta. El cliente pide, predice y dibuja.

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::chunk::Chunk;
use crate::content::ContentData;
use crate::lod::{LodTile, TileKey};
use crate::prefab::{Prefab, PrefabInfo};

pub const PROTOCOL_VERSION: u32 = 11;
/// Segundos reales que dura un día del mundo si nadie lo cambia (20 minutos).
pub const DEFAULT_DAY_LENGTH: f32 = 1200.0;
/// Día más corto y más largo que se aceptan, en segundos reales.
pub const DAY_LENGTH_RANGE: (f32, f32) = (10.0, 86_400.0);
/// Noches que tarda la luna en dar una vuelta entera de fases (llena → nueva → llena).
pub const MOON_CYCLE_NIGHTS: u32 = 8;
/// Nombre de cada fase de la luna, de luna nueva en adelante (octavos del ciclo).
pub const MOON_PHASES: [&str; 8] = [
    "new moon",
    "waxing crescent",
    "first quarter",
    "waxing gibbous",
    "full moon",
    "waning gibbous",
    "last quarter",
    "waning crescent",
];

/// Hora del mundo. La lleva el servidor; el cliente la recibe de vez en cuando y entre
/// medias la avanza por su cuenta.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldTime {
    /// Hora del día, de 0 a 24 (6 = amanecer, 12 = mediodía, 18 = atardecer).
    pub hours: f32,
    /// Segundos reales que dura un día entero; 0 = el tiempo está parado.
    pub day_length: f32,
    /// Días enteros desde que empezó el mundo: sube al pasar la medianoche. De él sale la
    /// fase de la luna, así que todos ven la misma.
    pub day: u32,
}

impl Default for WorldTime {
    fn default() -> Self {
        Self { hours: 9.0, day_length: DEFAULT_DAY_LENGTH, day: 0 }
    }
}

impl WorldTime {
    /// La hora `seconds` segundos reales después.
    pub fn advanced(self, seconds: f32) -> WorldTime {
        if self.day_length <= 0.0 {
            return self;
        }
        let total = self.hours as f64 + seconds as f64 * 24.0 / self.day_length as f64;
        let days = (total / 24.0).floor();
        let hours = total.rem_euclid(24.0) as f32;
        // `rem_euclid` en f32 puede devolver justo 24.0.
        let (hours, days) = if hours >= 24.0 { (0.0, days + 1.0) } else { (hours, days) };
        let day = (self.day as i64 + days as i64).clamp(0, u32::MAX as i64) as u32;
        WorldTime { hours, day, ..self }
    }

    /// Noche en curso o la última: la noche `n` va de la tarde del día `n` a la mañana
    /// del `n + 1`. La fase de la luna no cambia a mitad de una noche.
    pub fn night(self) -> u32 {
        if self.hours >= 12.0 { self.day } else { self.day.saturating_sub(1) }
    }

    /// Edad de la luna, de 0 a 1: 0 = luna nueva, 0,5 = llena. El mundo empieza con
    /// luna llena; cada noche avanza `1 / MOON_CYCLE_NIGHTS`.
    pub fn moon_age(self) -> f32 {
        let nights = self.night() % MOON_CYCLE_NIGHTS;
        (nights as f32 / MOON_CYCLE_NIGHTS as f32 + 0.5).fract()
    }

    /// Parte iluminada de la luna, de 0 (nueva) a 1 (llena).
    pub fn moon_light(self) -> f32 {
        (1.0 - (self.moon_age() * std::f32::consts::TAU).cos()) * 0.5
    }

    /// Nombre de la fase de la luna de esta noche.
    pub fn moon_phase(self) -> &'static str {
        MOON_PHASES[((self.moon_age() * 8.0).round() as usize) % 8]
    }

    /// Deja los valores dentro de sus límites (vienen de un archivo o de un cliente).
    pub fn sanitized(self) -> WorldTime {
        let hours = if self.hours.is_finite() { self.hours.rem_euclid(24.0) } else { 9.0 };
        let day_length = if !self.day_length.is_finite() || self.day_length <= 0.0 {
            0.0
        } else {
            self.day_length.clamp(DAY_LENGTH_RANGE.0, DAY_LENGTH_RANGE.1)
        };
        WorldTime { hours, day_length, day: self.day }
    }

    /// La hora como texto, `HH:MM`.
    pub fn clock(self) -> String {
        let minutes = (self.hours.rem_euclid(24.0) * 60.0).floor() as u32 % 1440;
        format!("{:02}:{:02}", minutes / 60, minutes % 60)
    }
}
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

/// Orden de las herramientas de construcción (selección, portapapeles y prefabs). Solo
/// las acepta el servidor a quien puede usar sus comandos de consola. Las cajas van de
/// `min` a `max`, ambos incluidos.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PrefabOp {
    /// Copiar una caja al portapapeles del jugador (en el servidor).
    Copy { min: [i32; 3], max: [i32; 3] },
    /// Como `Copy`, y además vaciar la caja.
    Cut { min: [i32; 3], max: [i32; 3] },
    /// Guardar una caja como prefab; también queda en el portapapeles.
    Save { name: String, min: [i32; 3], max: [i32; 3] },
    /// Cargar un prefab guardado en el portapapeles.
    Load { name: String },
    Delete { name: String },
    /// Pedir la lista de prefabs guardados.
    List,
    /// Colocar el portapapeles con su anclaje en `pos`, girado `turns` cuartos de vuelta.
    /// Con `air`, sus huecos también vacían lo que hubiera.
    Paste { pos: [i32; 3], turns: u8, air: bool },
    /// Rellenar una caja con un bloque (0 = vaciarla).
    Fill { min: [i32; 3], max: [i32; 3], block: u16 },
    /// Deshacer el último cambio hecho con estas herramientas.
    Undo,
}

/// Orden sobre el generador de terreno. Solo la acepta el servidor a quien puede usar
/// sus comandos de consola; responde con `TerrainStatus`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TerrainOp {
    /// Recarga en caliente: con `true`, el mundo se regenera en cuanto cambian los
    /// archivos de `worldgen/`; con `false`, los cambios esperan a `Apply`.
    Auto(bool),
    /// Usar ya el terreno tal como está en los archivos (y los perfiles elegidos).
    Apply,
    /// Elegir el perfil de un bioma en este mundo (`None` = el archivo por defecto).
    Profile { biome: String, profile: Option<String> },
    /// Pedir el estado sin cambiar nada.
    Status,
}

/// Estado del generador de terreno, tal como lo ve el anfitrión.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TerrainStatus {
    /// Recarga en caliente activada.
    pub auto: bool,
    /// Los archivos (o los perfiles elegidos) dan un terreno distinto del que se usa:
    /// espera a `TerrainOp::Apply`.
    pub pending: bool,
    /// Perfil elegido en este mundo para cada bioma que no usa el suyo por defecto
    /// (identificador del bioma → nombre del perfil).
    pub profiles: BTreeMap<String, String>,
    /// Los archivos tienen un error: el mundo sigue con el terreno anterior.
    pub error: Option<String>,
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
    /// Herramientas de construcción. El servidor responde con `PrefabResult` y, según
    /// la orden, con `Clipboard` o `PrefabList`.
    Prefab(PrefabOp),
    /// Cambiar la hora del mundo o la duración del día (0 = parar el tiempo); lo que va
    /// en `None` se queda como está. Solo se acepta a quien puede usar los comandos del
    /// servidor. No tiene respuesta propia: llega un `Time` a todos.
    SetTime { hours: Option<f32>, day_length: Option<f32> },
    /// Recarga del terreno y perfiles de los biomas. Sin respuesta de texto: llega un
    /// `TerrainStatus`.
    Terrain(TerrainOp),
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
    /// Resultado de una orden `Prefab`: texto para el jugador y si se hizo.
    PrefabResult { text: String, ok: bool },
    /// Lo que hay ahora en el portapapeles del jugador (para enseñar dónde caerá) y, si
    /// viene de un prefab guardado, su identificador.
    Clipboard { prefab: Arc<Prefab>, name: String },
    /// Prefabs guardados en el servidor.
    PrefabList(Vec<PrefabInfo>),
    /// Hora del mundo: al entrar, cuando alguien la cambia y cada pocos segundos.
    Time(WorldTime),
    /// Estado del generador de terreno. Solo a quien puede cambiarlo: al entrar, al
    /// cambiar algo y en respuesta a `ClientMsg::Terrain`.
    TerrainStatus(TerrainStatus),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_day_counts_midnights() {
        let time = WorldTime { hours: 23.0, day_length: 240.0, day: 3 };
        // 240 s por día: 10 s = 1 hora.
        let later = time.advanced(20.0);
        assert_eq!((later.day, later.hours.round()), (4, 1.0));
        assert_eq!(time.advanced(240.0 * 2.0 + 10.0).day, 6);
        let stopped = WorldTime { day_length: 0.0, ..time };
        assert_eq!(stopped.advanced(1000.0), stopped);
    }

    #[test]
    fn the_moon_goes_through_its_phases_one_night_at_a_time() {
        let night = |day: u32, hours: f32| WorldTime { hours, day_length: 0.0, day };
        assert_eq!(night(0, 22.0).moon_phase(), "full moon", "a new world starts with a full moon");
        assert!(night(0, 22.0).moon_light() > 0.99);
        // La misma noche antes y después de medianoche.
        assert_eq!(night(2, 23.0).moon_age(), night(3, 2.0).moon_age());
        let new = night(MOON_CYCLE_NIGHTS / 2, 22.0);
        assert_eq!(new.moon_phase(), "new moon");
        assert!(new.moon_light() < 0.01);
        assert_eq!(night(MOON_CYCLE_NIGHTS, 22.0).moon_phase(), "full moon", "and back");
        let quarters = [night(2, 22.0).moon_light(), night(6, 22.0).moon_light()];
        assert!(quarters.iter().all(|light| (light - 0.5).abs() < 0.01), "{quarters:?}");
    }
}
