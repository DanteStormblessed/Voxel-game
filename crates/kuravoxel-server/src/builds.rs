//! Construcciones en el terreno lejano.
//!
//! El terreno lejano sale del generador, que no sabe nada de lo editado. Aquí se compara
//! cada columna de chunks modificados con lo que el generador pondría y se anota, columna
//! de voxeles a columna, dónde la superficie ya no es la natural. Esas notas se mezclan
//! después en las teselas (`apply`): lo que sobresale va como una columna de bloques sobre
//! el terreno (igual que los árboles), y lo excavado o repintado cambia la celda.
//!
//! Sigue siendo un mapa de alturas: de una construcción se ve su volumen exterior con el
//! color del bloque más alto; no hay interiores ni voladizos.

use std::sync::Arc;

use glam::IVec3;
use kuravoxel_common::block::Block;
use kuravoxel_common::chunk::{Chunk, CHUNK_SIZE, CHUNK_SIZE_I};
use kuravoxel_common::lod::{self, LodExtra, LodTile, TileKey, TILE_CELLS};
use rustc_hash::FxHashMap;

use crate::worldgen::{WorldGen, SEA_LEVEL};

/// Lo construido cuenta como edificio (y nunca desaparece a lo lejos) si sobresale del
/// suelo al menos estos metros; por debajo es suelo repintado. Lo mismo, hacia abajo, para
/// lo excavado.
const MIN_RISE: i32 = 2;

/// Una columna de voxeles cuya superficie no es la que daría el generador.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltColumn {
    /// Posición dentro de la columna de chunks.
    pub x: u8,
    pub z: u8,
    /// Voxel sólido más alto.
    pub top: i32,
    /// Altura sobre el suelo natural (negativa si se ha excavado).
    pub rise: i32,
    pub block: Block,
}

/// Columnas construidas, por columna de chunks (x, z).
pub type Builds = FxHashMap<(i32, i32), Vec<BuiltColumn>>;

/// Voxel visible más alto de una columna, mirando solo esas capas (de arriba abajo).
fn top_of<'a>(
    layers: impl Iterator<Item = (i32, &'a Chunk)>,
    x: usize,
    z: usize,
    cubes: &[bool],
) -> Option<(i32, Block)> {
    for (cy, chunk) in layers {
        if chunk.uniform() == Some(Block::AIR) {
            continue;
        }
        for y in (0..CHUNK_SIZE).rev() {
            let block = chunk.get(x, y, z);
            if cubes.get(block.0 as usize).copied().unwrap_or(block != Block::AIR) {
                return Some((cy * CHUNK_SIZE_I + y as i32, block));
            }
        }
    }
    None
}

/// Compara los chunks modificados de una columna (`layers`: altura en chunks y chunk, de
/// arriba abajo) con lo que el generador pondría en su lugar. `cubes` dice, por número de
/// bloque, cuáles son cubos visibles; los modelos pequeños (flores, faroles) no cuentan.
pub fn scan_column(
    generator: &WorldGen,
    cubes: &[bool],
    (cx, cz): (i32, i32),
    layers: &[(i32, Arc<Chunk>)],
) -> Vec<BuiltColumn> {
    let (Some(&(highest, _)), Some(&(lowest, _))) = (layers.first(), layers.last()) else {
        return Vec::new();
    };
    let natural: Vec<(i32, Chunk)> =
        layers.iter().map(|&(cy, _)| (cy, generator.generate(IVec3::new(cx, cy, cz)))).collect();

    let mut built = Vec::new();
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let actual = top_of(layers.iter().map(|(cy, chunk)| (*cy, &**chunk)), x, z, cubes);
            let expected = top_of(natural.iter().map(|(cy, chunk)| (*cy, chunk)), x, z, cubes);
            if actual == expected {
                continue;
            }
            let (wx, wz) = (cx * CHUNK_SIZE_I + x as i32, cz * CHUNK_SIZE_I + z as i32);
            let ground = generator.height_at(wx, wz).max(SEA_LEVEL);
            // La superficie está en una capa sin tocar, por encima: lo editado queda bajo
            // tierra (una mina, un sótano) y desde fuera no se ve.
            if ground >= (highest + 1) * CHUNK_SIZE_I {
                continue;
            }
            let (top, block) = match (actual, expected) {
                (Some(found), _) => found,
                // Excavado hasta más abajo de lo guardado: el fondo es terreno sin tocar.
                (None, Some((_, block))) => ((lowest * CHUNK_SIZE_I - 1).min(ground), block),
                (None, None) => continue,
            };
            built.push(BuiltColumn { x: x as u8, z: z as u8, top, rise: top - ground, block });
        }
    }
    built
}

/// Teselas de todos los niveles en las que influye una columna de chunks (contando la
/// celda de borde que cada tesela guarda de sus vecinas).
pub fn tiles_touching((cx, cz): (i32, i32)) -> Vec<TileKey> {
    let mut keys = Vec::new();
    for level in lod::MIN_LEVEL..=lod::MAX_LEVEL {
        let border = ((1 << level) / CHUNK_SIZE_I).max(1);
        for dz in [-border, 0, border] {
            for dx in [-border, 0, border] {
                let key = (level, (cx + dx) >> level, (cz + dz) >> level);
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
    }
    keys
}

/// Lo que hay construido dentro de una celda de tesela.
#[derive(Default)]
struct Cell {
    /// Lo más alto que sobresale del suelo.
    raised: Option<(i32, Block)>,
    /// Columnas excavadas y la más alta de ellas.
    dug: u32,
    dug_top: Option<(i32, Block)>,
    /// Columnas a ras de suelo con otro bloque.
    painted: u32,
    paint: Block,
}

/// Mezcla las construcciones en una tesela generada. `None` si no le afecta ninguna.
///
/// - Una celda con algo que sobresale toma su altura **máxima** (redondeada hacia arriba):
///   una casa más pequeña que la celda se sigue viendo, como un cubo grueso.
/// - Lo excavado y lo repintado solo cuentan si ocupan al menos media celda.
pub fn apply(tile: &LodTile, builds: &Builds) -> Option<LodTile> {
    if builds.is_empty() {
        return None;
    }
    let (level, tx, tz) = tile.key;
    let s = 1i32 << level;
    let cells = TILE_CELLS as i32;
    // Esquina de la celda (0, 0), que es la de borde.
    let (x0, z0) = (tx * CHUNK_SIZE_I * s - s, tz * CHUNK_SIZE_I * s - s);
    let chunks_x = x0.div_euclid(CHUNK_SIZE_I)..=(x0 + cells * s - 1).div_euclid(CHUNK_SIZE_I);
    let chunks_z = z0.div_euclid(CHUNK_SIZE_I)..=(z0 + cells * s - 1).div_euclid(CHUNK_SIZE_I);

    let mut touched: FxHashMap<usize, Cell> = FxHashMap::default();
    let mut gather = |(cx, cz): (i32, i32), columns: &[BuiltColumn]| {
        for column in columns {
            let px = (cx * CHUNK_SIZE_I + column.x as i32 - x0).div_euclid(s);
            let pz = (cz * CHUNK_SIZE_I + column.z as i32 - z0).div_euclid(s);
            if !(0..cells).contains(&px) || !(0..cells).contains(&pz) {
                continue;
            }
            let cell = touched.entry(px as usize + pz as usize * TILE_CELLS).or_default();
            if column.rise >= MIN_RISE {
                if cell.raised.is_none_or(|(top, _)| column.top > top) {
                    cell.raised = Some((column.top, column.block));
                }
            } else if column.rise <= -MIN_RISE {
                cell.dug += 1;
                if cell.dug_top.is_none_or(|(top, _)| column.top > top) {
                    cell.dug_top = Some((column.top, column.block));
                }
            } else {
                cell.painted += 1;
                cell.paint = column.block;
            }
        }
    };
    // Se recorre lo que sea menos: los chunks de la tesela o las columnas construidas.
    let span = (chunks_x.end() - chunks_x.start() + 1) as usize * (chunks_z.end() - chunks_z.start() + 1) as usize;
    if builds.len() < span {
        for (&key, columns) in builds {
            if chunks_x.contains(&key.0) && chunks_z.contains(&key.1) {
                gather(key, columns);
            }
        }
    } else {
        for cz in chunks_z.clone() {
            for cx in chunks_x.clone() {
                if let Some(columns) = builds.get(&(cx, cz)) {
                    gather((cx, cz), columns);
                }
            }
        }
    }
    if touched.is_empty() {
        return None;
    }

    let mut columns = tile.decode()?;
    let mut extras = tile.extras.clone();
    let half = ((s * s) as u32).div_ceil(2);
    let mut changed = false;
    for (&i, cell) in &touched {
        if let Some((top, block)) = cell.raised {
            let from = columns.top[i] + 1;
            let to = (top + s).div_euclid(s) - 1;
            if to >= from {
                // La construcción sustituye a los árboles que hubiera en la celda.
                extras.retain(|extra| extra.column as usize != i);
                extras.push(LodExtra {
                    column: i as u16,
                    from: from.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                    to: to.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                    block,
                });
                changed = true;
            }
        } else if let Some((top, block)) = cell.dug_top.filter(|_| cell.dug >= half) {
            let lowered = (top + 1).div_euclid(s) - 1;
            if lowered < columns.top[i] {
                columns.top[i] = lowered;
                columns.surface[i] = block;
                extras.retain(|extra| extra.column as usize != i);
                changed = true;
            }
        } else if cell.painted >= half && columns.surface[i] != cell.paint {
            columns.surface[i] = cell.paint;
            changed = true;
        }
    }
    changed.then(|| {
        LodTile::encode(tile.key, &columns.top, &columns.surface, &columns.subsoil, tile.deep, extras)
    })
}

#[cfg(test)]
mod tests {
    use kuravoxel_common::lod::TILE_AREA;

    use super::*;

    const GRASS: Block = Block(1);
    const STONE: Block = Block(2);
    const PLANKS: Block = Block(7);

    /// Tesela llana con el suelo en la celda de altura `top`.
    fn flat_tile(key: TileKey, top: i32) -> LodTile {
        LodTile::encode(key, &[top; TILE_AREA], &[GRASS; TILE_AREA], &[STONE; TILE_AREA], STONE, Vec::new())
    }

    fn column(x: u8, z: u8, top: i32, rise: i32) -> BuiltColumn {
        BuiltColumn { x, z, top, rise, block: PLANKS }
    }

    #[test]
    fn a_small_house_never_disappears() {
        // Una casa de 4×4 y 5 m de alto en (8..12, 8..12), con el suelo en y = 9.
        let mut builds = Builds::default();
        let house: Vec<BuiltColumn> =
            (8..12).flat_map(|z| (8..12).map(move |x| column(x, z, 14, 5))).collect();
        builds.insert((0, 0), house);

        // Celdas de 2 m: la casa ocupa 2×2 celdas y llega hasta y = 15.
        let tile = apply(&flat_tile((1, 0, 0), 4), &builds).expect("the tile changes");
        assert_eq!(tile.extras.len(), 4);
        assert!(tile.extras.iter().all(|e| (e.from, e.to, e.block) == (5, 7, PLANKS)));
        let cell = 1 + 4 + (1 + 4) * TILE_CELLS;
        assert!(tile.extras.iter().any(|e| e.column as usize == cell));

        // Celdas de 16 m: es mucho menor que la celda, pero se ve como un cubo.
        let tile = apply(&flat_tile((4, 0, 0), -1), &builds).expect("the tile changes");
        assert_eq!(tile.extras.len(), 1);
        assert_eq!((tile.extras[0].column as usize, tile.extras[0].from, tile.extras[0].to), (1 + TILE_CELLS, 0, 0));

        // Una tesela lejana no se entera.
        assert!(apply(&flat_tile((1, 5, 5), 4), &builds).is_none());
        // La vecina la tiene en su celda de borde solo si la casa toca el borde.
        assert!(apply(&flat_tile((1, -1, 0), 4), &builds).is_none());
        builds.insert((0, 0), vec![column(0, 0, 14, 5)]);
        let neighbour = apply(&flat_tile((1, -1, 0), 4), &builds).expect("border cell");
        assert_eq!(neighbour.extras[0].column as usize, 33 + TILE_CELLS);
    }

    #[test]
    fn digging_and_painting_need_half_a_cell() {
        let mut builds = Builds::default();
        // Un pozo de 1×1: no cambia una celda de 2 m.
        builds.insert((0, 0), vec![column(0, 0, 3, -6)]);
        assert!(apply(&flat_tile((1, 0, 0), 4), &builds).is_none());

        // Una zanja de 2×1 sí: la celda baja y toma el bloque del fondo.
        builds.insert((0, 0), vec![column(0, 0, 3, -6), column(1, 0, 3, -6)]);
        let tile = apply(&flat_tile((1, 0, 0), 4), &builds).expect("the tile changes");
        let columns = tile.decode().unwrap();
        let cell = 1 + TILE_CELLS;
        assert_eq!((columns.top[cell], columns.surface[cell]), (1, PLANKS));
        assert_eq!(columns.top[cell + 1], 4);

        // Un suelo de tablas a ras de tierra solo cambia el color.
        builds.insert((0, 0), vec![column(0, 0, 9, 0), column(1, 0, 9, 0)]);
        let tile = apply(&flat_tile((1, 0, 0), 4), &builds).expect("the tile changes");
        let columns = tile.decode().unwrap();
        assert_eq!((columns.top[cell], columns.surface[cell]), (4, PLANKS));
        assert!(tile.extras.is_empty());
    }

    #[test]
    fn every_level_has_tiles_for_a_column() {
        let keys = tiles_touching((0, 0));
        for level in lod::MIN_LEVEL..=lod::MAX_LEVEL {
            // El chunk (0, 0) está en la esquina de su tesela: toca también a tres vecinas.
            assert_eq!(keys.iter().filter(|key| key.0 == level).count(), 4, "level {level}");
            assert!(keys.contains(&(level, 0, 0)) && keys.contains(&(level, -1, -1)));
        }
        assert_eq!(tiles_touching((5, 5)).iter().filter(|key| key.0 == 4).count(), 1);
    }
}
