//! Terreno lejano (LOD): datos que el servidor envía y el cliente malla.
//!
//! Las teselas forman un quadtree: una tesela de nivel `n` tiene 32×32 celdas de `2^n`
//! metros (`32·2^n` m de lado) y se divide en cuatro hijas de nivel `n - 1`. Cerca del
//! jugador se usan celdas de 2 m y, a cada salto de distancia, celdas el doble de grandes.

use serde::{Deserialize, Serialize};

use crate::block::Block;
use crate::chunk::{rle_decode, rle_encode};

/// Nivel más fino (celdas de 2 m) y raíz del quadtree (celdas de 64 m, 2 km de lado).
pub const MIN_LEVEL: u8 = 1;
pub const MAX_LEVEL: u8 = 6;
/// Una tesela se divide en sus hijas mientras su punto más cercano al jugador esté a
/// menos de estos lados de tesela. Más alto = más detalle lejos (y más quads).
pub const SPLIT_DISTANCE: i32 = 5;
/// Celdas por lado de una tesela, incluida una de borde a cada lado (32 + 2).
pub const TILE_CELLS: usize = 34;
pub const TILE_AREA: usize = TILE_CELLS * TILE_CELLS;

/// (nivel, x, z) en unidades de tesela de ese nivel.
pub type TileKey = (u8, i32, i32);

/// Chunks (de 32 m) por lado de una tesela de ese nivel.
pub const fn tile_chunks(level: u8) -> i32 {
    1 << level
}

pub fn is_valid_level(level: u8) -> bool {
    (MIN_LEVEL..=MAX_LEVEL).contains(&level)
}

pub fn children(key: TileKey) -> [TileKey; 4] {
    let (level, x, z) = key;
    let child = level - 1;
    [
        (child, x * 2, z * 2),
        (child, x * 2 + 1, z * 2),
        (child, x * 2, z * 2 + 1),
        (child, x * 2 + 1, z * 2 + 1),
    ]
}

pub fn parent(key: TileKey) -> TileKey {
    (key.0 + 1, key.1.div_euclid(2), key.2.div_euclid(2))
}

/// Dos teselas cubren parte del mismo terreno (una contiene a la otra, o son la misma).
pub fn overlaps(a: TileKey, b: TileKey) -> bool {
    let (fine, coarse) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let shift = (coarse.0 - fine.0) as u32;
    // El desplazamiento aritmético redondea hacia abajo también con negativos.
    (fine.1 >> shift, fine.2 >> shift) == (coarse.1, coarse.2)
}

/// Teselas necesarias alrededor del chunk (cx, cz), de cerca a lejos. `radius` es el
/// alcance total y `fine_radius` hasta dónde llega como mínimo el nivel más fino (para
/// que siempre cubra el borde de los voxeles reales), ambos en chunks.
pub fn desired_tiles(cx: i32, cz: i32, radius: i32, fine_radius: i32) -> Vec<TileKey> {
    let center = (cx as f32 + 0.5, cz as f32 + 0.5);
    let root = tile_chunks(MAX_LEVEL);
    let range = |c: i32| (c - radius).div_euclid(root)..=(c + radius).div_euclid(root);

    let mut tiles: Vec<(f32, TileKey)> = Vec::new();
    let mut stack: Vec<TileKey> = Vec::new();
    for tz in range(cz) {
        for tx in range(cx) {
            stack.push((MAX_LEVEL, tx, tz));
        }
    }
    while let Some(key) = stack.pop() {
        let (level, tx, tz) = key;
        let size = tile_chunks(level) as f32;
        let (x0, z0) = (tx as f32 * size, tz as f32 * size);
        let dist = |x: f32, z: f32| ((x - center.0).powi(2) + (z - center.1).powi(2)).sqrt();
        let nearest = dist(center.0.clamp(x0, x0 + size), center.1.clamp(z0, z0 + size));
        if nearest > radius as f32 {
            continue;
        }
        let mut split = SPLIT_DISTANCE as f32 * size;
        if level == MIN_LEVEL + 1 {
            split = split.max(fine_radius as f32);
        }
        if level > MIN_LEVEL && nearest < split {
            stack.extend(children(key));
        } else {
            tiles.push((dist(x0 + size * 0.5, z0 + size * 0.5), key));
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
    pub block: Block,
}

/// Una tesela de terreno lejano: 34×34 celdas con su altura y sus bloques.
#[derive(Serialize, Deserialize)]
pub struct LodTile {
    pub key: TileKey,
    /// Bloque de las celdas que quedan por debajo del subsuelo.
    pub deep: Block,
    /// Alturas como diferencias respecto a la celda anterior (casi siempre 1 byte).
    top_delta: Vec<i16>,
    surface: Vec<(u16, Block)>,
    subsoil: Vec<(u16, Block)>,
    pub extras: Vec<LodExtra>,
}

/// Contenido descomprimido de una tesela.
pub struct LodColumns {
    /// Celda sólida más alta de cada columna, en unidades del nivel.
    pub top: Vec<i32>,
    pub surface: Vec<Block>,
    pub subsoil: Vec<Block>,
}

impl LodTile {
    pub fn encode(
        key: TileKey,
        top: &[i32],
        surface: &[Block],
        subsoil: &[Block],
        deep: Block,
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
        let mut surface = vec![Block::AIR; TILE_AREA];
        let mut subsoil = vec![Block::AIR; TILE_AREA];
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlaps_follows_the_quadtree() {
        let tile = (3, -1, 2);
        assert!(overlaps(tile, tile));
        for child in children(tile) {
            assert!(overlaps(child, tile) && overlaps(tile, child));
            assert_eq!(parent(child), tile);
            for grandchild in children(child) {
                assert!(overlaps(grandchild, tile));
            }
        }
        assert!(!overlaps((2, -3, 4), tile));
        assert!(!overlaps((3, 0, 2), tile));
    }

    #[test]
    fn desired_tiles_cover_the_area_once_and_coarsen_with_distance() {
        let (cx, cz, radius) = (7, -3, 160);
        let tiles = desired_tiles(cx, cz, radius, 16);
        // Ninguna se solapa con otra.
        let set: std::collections::HashSet<_> = tiles.iter().copied().collect();
        assert_eq!(set.len(), tiles.len());
        for &tile in &tiles {
            let mut up = tile;
            while up.0 < MAX_LEVEL {
                up = parent(up);
                assert!(!set.contains(&up), "{tile:?} and its ancestor {up:?}");
            }
        }
        // Cada chunk dentro del radio está cubierto por exactamente una tesela, y el
        // nivel nunca baja al alejarse.
        let level_at = |x: i32, z: i32| {
            let covering: Vec<_> = tiles
                .iter()
                .filter(|&&(level, tx, tz)| {
                    let size = tile_chunks(level);
                    x.div_euclid(size) == tx && z.div_euclid(size) == tz
                })
                .collect();
            assert_eq!(covering.len(), 1, "chunk {x} {z}");
            covering[0].0
        };
        assert_eq!(level_at(cx, cz), MIN_LEVEL);
        assert_eq!(level_at(cx + 15, cz), MIN_LEVEL, "the finest level reaches the voxels");
        let mut previous = MIN_LEVEL;
        for step in 0..radius - 8 {
            let level = level_at(cx + step, cz);
            assert!(level >= previous);
            previous = level;
        }
        assert!(previous >= 4, "5 km away the cells are coarse");
        // De cerca a lejos.
        assert_eq!(tiles[0].0, MIN_LEVEL);
    }
}
