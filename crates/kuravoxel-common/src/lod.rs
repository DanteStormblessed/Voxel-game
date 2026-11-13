//! Terreno lejano (LOD): datos que el servidor envía y el cliente malla.
//!
//! Las teselas forman un quadtree de dos niveles: una tesela gruesa (nivel 2, celdas de
//! 4 m, 128 m de lado) o sus cuatro hijas finas (nivel 1, celdas de 2 m, 64 m de lado).

use serde::{Deserialize, Serialize};

use crate::chunk::{rle_decode, rle_encode};

pub const FINE: u8 = 1;
pub const COARSE: u8 = 2;
/// Chunks (de 32 m) por lado de una tesela gruesa.
pub const COARSE_CHUNKS: i32 = 1 << COARSE;
/// Celdas por lado de una tesela, incluida una de borde a cada lado (32 + 2).
pub const TILE_CELLS: usize = 34;
pub const TILE_AREA: usize = TILE_CELLS * TILE_CELLS;

/// (nivel, x, z) en unidades de tesela de ese nivel.
pub type TileKey = (u8, i32, i32);

pub fn children(key: TileKey) -> [TileKey; 4] {
    let (_, x, z) = key;
    [
        (FINE, x * 2, z * 2),
        (FINE, x * 2 + 1, z * 2),
        (FINE, x * 2, z * 2 + 1),
        (FINE, x * 2 + 1, z * 2 + 1),
    ]
}

pub fn parent(key: TileKey) -> TileKey {
    (COARSE, key.1.div_euclid(2), key.2.div_euclid(2))
}

/// Teselas que con las que una tesela se solapa en el otro nivel: sus hijas o su madre.
pub fn family(key: TileKey) -> Vec<TileKey> {
    if key.0 == COARSE {
        children(key).to_vec()
    } else {
        vec![parent(key)]
    }
}

/// Teselas necesarias alrededor del chunk (cx, cz), de cerca a lejos. `radius` y
/// `fine_radius` van en chunks.
pub fn desired_tiles(cx: i32, cz: i32, radius: i32, fine_radius: i32) -> Vec<TileKey> {
    let center = (cx as f32 + 0.5, cz as f32 + 0.5);
    let dist = |x: f32, z: f32| ((x - center.0).powi(2) + (z - center.1).powi(2)).sqrt();
    let size = COARSE_CHUNKS as f32;
    let range =
        |c: i32| (c - radius).div_euclid(COARSE_CHUNKS)..=(c + radius).div_euclid(COARSE_CHUNKS);

    let mut tiles: Vec<(f32, TileKey)> = Vec::new();
    for tz in range(cz) {
        for tx in range(cx) {
            let (x0, z0) = (tx as f32 * size, tz as f32 * size);
            let nearest = dist(center.0.clamp(x0, x0 + size), center.1.clamp(z0, z0 + size));
            if nearest > radius as f32 {
                continue;
            }
            let center_dist = dist(x0 + size * 0.5, z0 + size * 0.5);
            let coarse = (COARSE, tx, tz);
            if center_dist < fine_radius as f32 {
                tiles.extend(children(coarse).map(|child| (center_dist, child)));
            } else {
                tiles.push((center_dist, coarse));
            }
        }
    }
    tiles.sort_by(|a, b| a.0.total_cmp(&b.0));
    tiles.into_iter().map(|(_, key)| key).collect()
}

/// Columna de bloques extra sobre el terreno (árboles simplificados).
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct LodExtra {
    /// Índice de celda: x + z * TILE_CELLS.
    pub column: u16,
    /// Primera y última celda en vertical, en unidades del nivel.
    pub from: i16,
    pub to: i16,
    pub block: u8,
}

/// Una tesela de terreno lejano: 34×34 celdas con su altura y sus bloques.
#[derive(Serialize, Deserialize)]
pub struct LodTile {
    pub key: TileKey,
    /// Bloque de las celdas que quedan por debajo del subsuelo.
    pub deep: u8,
    /// Alturas como diferencias respecto a la celda anterior (casi siempre 1 byte).
    top_delta: Vec<i16>,
    surface: Vec<(u16, u8)>,
    subsoil: Vec<(u16, u8)>,
    pub extras: Vec<LodExtra>,
}

/// Contenido descomprimido de una tesela.
pub struct LodColumns {
    /// Celda sólida más alta de cada columna, en unidades del nivel.
    pub top: Vec<i32>,
    pub surface: Vec<u8>,
    pub subsoil: Vec<u8>,
}

impl LodTile {
    pub fn encode(
        key: TileKey,
        top: &[i32],
        surface: &[u8],
        subsoil: &[u8],
        deep: u8,
        extras: Vec<LodExtra>,
    ) -> Self {
        assert!(top.len() == TILE_AREA && surface.len() == TILE_AREA && subsoil.len() == TILE_AREA);
        let mut previous = 0;
        let top_delta = top
            .iter()
            .map(|&value| {
                let delta = (value - previous) as i16;
                previous = value;
                delta
            })
            .collect();
        Self {
            key,
            deep,
            top_delta,
            surface: rle_encode(surface),
            subsoil: rle_encode(subsoil),
            extras,
        }
    }

    /// `None` si los datos recibidos no tienen el tamaño esperado.
    pub fn decode(&self) -> Option<LodColumns> {
        if self.top_delta.len() != TILE_AREA {
            return None;
        }
        let mut value = 0i32;
        let top = self
            .top_delta
            .iter()
            .map(|&delta| {
                value += delta as i32;
                value
            })
            .collect();
        let mut surface = vec![0u8; TILE_AREA];
        let mut subsoil = vec![0u8; TILE_AREA];
        if !rle_decode(&self.surface, &mut surface) || !rle_decode(&self.subsoil, &mut subsoil) {
            return None;
        }
        if self.extras.iter().any(|extra| extra.column as usize >= TILE_AREA) {
            return None;
        }
        Some(LodColumns { top, surface, subsoil })
    }

    /// Altura de una columna sin descomprimir la tesela entera.
    pub fn top_at(&self, column: usize) -> Option<i32> {
        (column < self.top_delta.len())
            .then(|| self.top_delta[..=column].iter().map(|&d| d as i32).sum())
    }
}
