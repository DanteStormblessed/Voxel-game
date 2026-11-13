//! Generación procedural del terreno.
//!
//! Los chunks son cúbicos, así que varios chunks apilados comparten la misma columna
//! (cx, cz). El mapa de alturas y los árboles se calculan una sola vez por columna y se
//! cachean: los chunks de cielo o de roca maciza se resuelven sin tocar un solo voxel.

use std::sync::{Arc, Mutex, OnceLock};

use glam::IVec3;
use rustc_hash::FxHashMap;

use voxel_common::block::Block;
use anyhow::Context;
use voxel_common::chunk::{Chunk, CHUNK_SIZE, CHUNK_SIZE_I, CHUNK_VOLUME};
use voxel_common::content::ContentData;
use voxel_common::lod::{LodExtra, LodTile, TileKey, TILE_AREA, TILE_CELLS as PAD};

use crate::noise::{fbm2, hash2, perlin2, smoothstep};

pub const SEA_LEVEL: i32 = 0;
/// Capas de subsuelo (tierra/arena) entre la superficie y la roca.
const SUBSOIL_DEPTH: i32 = 3;
const ROCK_LINE: i32 = 70;
const SNOW_LINE: i32 = 104;

/// Los árboles se colocan como mucho uno por celda de esta rejilla (en voxeles).
const TREE_CELL: i32 = 7;
const LEAF_RADIUS: i32 = 2;
/// Altura de la copa por encima del último bloque de tronco.
const CANOPY_HEIGHT: i32 = 3;
/// Bloques que usa el generador, resueltos por nombre en el contenido cargado.
#[derive(Clone, Copy)]
pub struct TerrainBlocks {
    pub grass: Block,
    pub dirt: Block,
    pub stone: Block,
    pub sand: Block,
    pub water: Block,
    pub snow: Block,
    pub wood: Block,
    pub leaves: [Block; 4],
}

impl TerrainBlocks {
    pub fn resolve(content: &ContentData) -> anyhow::Result<Self> {
        let find = |id: &str| {
            content.block_id(id).with_context(|| {
                format!("the terrain generator needs block \"{id}\", which is missing from the content")
            })
        };
        Ok(Self {
            grass: find("base:grass")?,
            dirt: find("base:dirt")?,
            stone: find("base:stone")?,
            sand: find("base:sand")?,
            water: find("base:water")?,
            snow: find("base:snow")?,
            wood: find("base:wood")?,
            leaves: [
                find("base:leaves_yellow")?,
                find("base:leaves_lime")?,
                find("base:leaves_orange")?,
                find("base:leaves_green")?,
            ],
        })
    }
}

struct Tree {
    /// Posición del tronco en coordenadas de mundo.
    x: i32,
    z: i32,
    /// Altura del suelo bajo el tronco.
    base: i32,
    trunk: i32,
    leaves: Block,
}

struct ColumnData {
    /// Altura del bloque sólido más alto de cada (x, z), indexado como x + z * 32.
    heights: [i16; CHUNK_SIZE * CHUNK_SIZE],
    min: i32,
    /// Y más alta con algún bloque (terreno, agua o árboles). Por encima, todo es aire.
    top: i32,
    /// Árboles cuya copa puede tocar esta columna (incluye los plantados en las vecinas).
    trees: Vec<Tree>,
}

type ColumnCell = Arc<OnceLock<ColumnData>>;

pub struct WorldGen {
    seed: u32,
    blocks: TerrainBlocks,
    columns: Mutex<FxHashMap<(i32, i32), ColumnCell>>,
}

impl WorldGen {
    pub fn new(seed: u32, blocks: TerrainBlocks) -> Self {
        Self {
            seed,
            blocks,
            columns: Mutex::new(FxHashMap::default()),
        }
    }

    /// Altura del terreno (Y del bloque sólido más alto) en una columna del mundo.
    pub fn height_at(&self, wx: i32, wz: i32) -> i32 {
        let (x, z) = (wx as f64, wz as f64);
        let seed = self.seed;

        let continent = fbm2(x * 0.0011, z * 0.0011, 4, seed);
        let hills = fbm2(x * 0.0085, z * 0.0085, 4, seed ^ 0x9E37);
        let mountains = smoothstep(0.0, 0.4, fbm2(x * 0.0007, z * 0.0007, 3, seed ^ 0x51ED));
        let ridge = (1.0 - fbm2(x * 0.0035, z * 0.0035, 4, seed ^ 0xC0FE).abs() * 2.2).clamp(0.0, 1.0);

        let height = 8.0
            + continent * 55.0
            + hills * (5.0 + 12.0 * mountains)
            + mountains * ridge * ridge * 120.0;
        height.floor() as i32
    }

    /// Bloque de superficie y de subsuelo para una columna de altura `h`.
    fn surface_blocks(&self, wx: i32, wz: i32, h: i32) -> (Block, Block) {
        // Pequeño tramado para que las transiciones por altitud no sean líneas rectas.
        let jitter = (hash2(wx, wz, self.seed ^ 0xA5A5) % 5) as i32 - 2;
        if h <= SEA_LEVEL + 1 {
            (self.blocks.sand, self.blocks.sand)
        } else if h + jitter >= SNOW_LINE {
            (self.blocks.snow, self.blocks.stone)
        } else if h + jitter >= ROCK_LINE {
            (self.blocks.stone, self.blocks.stone)
        } else {
            (self.blocks.grass, self.blocks.dirt)
        }
    }

    fn column(&self, cx: i32, cz: i32) -> ColumnCell {
        // El mutex solo protege el mapa; el cálculo (caro) va fuera, dentro del OnceLock,
        // de modo que dos hilos nunca calculan la misma columna dos veces.
        let cell = self
            .columns
            .lock()
            .unwrap()
            .entry((cx, cz))
            .or_default()
            .clone();
        cell.get_or_init(|| self.build_column(cx, cz));
        cell
    }

    fn build_column(&self, cx: i32, cz: i32) -> ColumnData {
        let (x0, z0) = (cx * CHUNK_SIZE_I, cz * CHUNK_SIZE_I);
        let mut heights = [0i16; CHUNK_SIZE * CHUNK_SIZE];
        let (mut min, mut max) = (i32::MAX, i32::MIN);
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let h = self.height_at(x0 + x as i32, z0 + z as i32);
                heights[x + z * CHUNK_SIZE] = h as i16;
                min = min.min(h);
                max = max.max(h);
            }
        }

        let mut top = if min < SEA_LEVEL { max.max(SEA_LEVEL) } else { max };

        // Árboles: un candidato por celda de rejilla, con posición determinista por hash.
        // Se miran también las celdas de fuera cuya copa puede entrar en esta columna.
        let mut trees = Vec::new();
        let lo = |origin: i32| (origin - LEAF_RADIUS - (TREE_CELL - 1)).div_euclid(TREE_CELL);
        let hi = |origin: i32| (origin + CHUNK_SIZE_I - 1 + LEAF_RADIUS).div_euclid(TREE_CELL);
        for gz in lo(z0)..=hi(z0) {
            for gx in lo(x0)..=hi(x0) {
                let h1 = hash2(gx, gz, self.seed ^ 0x7EE5);
                let tx = gx * TREE_CELL + 1 + (h1 % 5) as i32;
                let tz = gz * TREE_CELL + 1 + ((h1 >> 8) % 5) as i32;
                let reaches = |t: i32, origin: i32| {
                    t >= origin - LEAF_RADIUS && t <= origin + CHUNK_SIZE_I - 1 + LEAF_RADIUS
                };
                if !reaches(tx, x0) || !reaches(tz, z0) {
                    continue;
                }

                let forest = perlin2(tx as f64 * 0.008, tz as f64 * 0.008, self.seed ^ 0xF0E5);
                let density = 0.04 + 0.5 * smoothstep(-0.05, 0.35, forest);
                if ((h1 >> 16) & 0xFF) as f32 > density * 255.0 {
                    continue;
                }

                let (lx, lz) = (tx - x0, tz - z0);
                let inside = (0..CHUNK_SIZE_I).contains(&lx) && (0..CHUNK_SIZE_I).contains(&lz);
                let base = if inside {
                    heights[lx as usize + lz as usize * CHUNK_SIZE] as i32
                } else {
                    self.height_at(tx, tz)
                };
                if self.surface_blocks(tx, tz, base).0 != self.blocks.grass {
                    continue;
                }

                let h2 = hash2(gx, gz, self.seed ^ 0x1EAF);
                let trunk = 4 + (h2 % 3) as i32;
                let leaves = self.blocks.leaves[((h2 >> 8) % 4) as usize];
                top = top.max(base + trunk + CANOPY_HEIGHT);
                trees.push(Tree { x: tx, z: tz, base, trunk, leaves });
            }
        }

        ColumnData { heights, min, top, trees }
    }

    /// Genera un chunk. Se llama desde los hilos de trabajo.
    pub fn generate(&self, pos: IVec3) -> Chunk {
        let cell = self.column(pos.x, pos.z);
        let column = cell.get().expect("column is initialized");

        let y0 = pos.y * CHUNK_SIZE_I;
        let y1 = y0 + CHUNK_SIZE_I - 1;
        if y0 > column.top {
            return Chunk::Uniform(Block::AIR);
        }
        if y1 < column.min - SUBSOIL_DEPTH {
            return Chunk::Uniform(self.blocks.stone);
        }

        let (x0, z0) = (pos.x * CHUNK_SIZE_I, pos.z * CHUNK_SIZE_I);
        let mut data = Chunk::new_dense_buffer();
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let h = column.heights[x + z * CHUNK_SIZE] as i32;
                let fill_top = h.max(SEA_LEVEL).min(y1);
                if fill_top < y0 {
                    continue;
                }
                let (surface, subsoil) = self.surface_blocks(x0 + x as i32, z0 + z as i32, h);
                for y in y0..=fill_top {
                    let block = if y > h {
                        self.blocks.water
                    } else if y == h {
                        surface
                    } else if y >= h - SUBSOIL_DEPTH {
                        subsoil
                    } else {
                        self.blocks.stone
                    };
                    data[Chunk::index(x, (y - y0) as usize, z)] = block.0;
                }
            }
        }

        for tree in &column.trees {
            place_tree(&mut data, tree, self.blocks.wood, x0, y0, z0);
        }

        Chunk::from_dense(data)
    }

    /// Genera los datos de una tesela de terreno lejano: 32×32 celdas de `2^level` metros
    /// más una de borde. El cliente los malla. La altura de cada celda es la **mínima** de las columnas que cubre:
    /// así el terreno lejano nunca sobresale por encima del detallado ni de un nivel más
    /// fino, y las uniones entre niveles no dejan huecos vistos desde la cámara.
    pub fn lod_tile(&self, key: TileKey) -> LodTile {
        let (level, tx, tz) = (key.0 as u32, key.1, key.2);
        let s = 1i32 << level;
        let span = CHUNK_SIZE_I * s;
        let (x0, z0) = (tx * span, tz * span);

        // Alturas a resolución completa, con una celda de borde alrededor.
        let side = (span + 2 * s) as usize;
        let su = s as usize;
        let mut heights = vec![0i32; side * side];
        let mut trees: Vec<&Tree> = Vec::new();
        let cells: Vec<ColumnCell> = (0..s * s)
            .map(|i| self.column(tx * s + i % s, tz * s + i / s))
            .collect();
        for (i, cell) in cells.iter().enumerate() {
            let column = cell.get().expect("column is initialized");
            let (ox, oz) = (su + (i % su) * CHUNK_SIZE, su + (i / su) * CHUNK_SIZE);
            for z in 0..CHUNK_SIZE {
                let row = (oz + z) * side + ox;
                for x in 0..CHUNK_SIZE {
                    heights[row + x] = column.heights[x + z * CHUNK_SIZE] as i32;
                }
            }
            // Solo los árboles plantados dentro de esta columna (la lista incluye vecinos).
            let (cx0, cz0) = (x0 + (i % su) as i32 * CHUNK_SIZE_I, z0 + (i / su) as i32 * CHUNK_SIZE_I);
            trees.extend(column.trees.iter().filter(|t| {
                (cx0..cx0 + CHUNK_SIZE_I).contains(&t.x) && (cz0..cz0 + CHUNK_SIZE_I).contains(&t.z)
            }));
        }
        for j in 0..side {
            for i in 0..side {
                if i < su || i >= side - su || j < su || j >= side - su {
                    heights[i + j * side] = self.height_at(x0 - s + i as i32, z0 - s + j as i32);
                }
            }
        }

        // Reducción a 34×34 celdas: celda sólida más alta y bloques de cada una.
        let mut top = [0i32; TILE_AREA];
        let mut blocks = [(Block::AIR, Block::AIR); TILE_AREA];
        for pz in 0..PAD {
            for px in 0..PAD {
                let mut h = i32::MAX;
                for dz in 0..su {
                    let row = (pz * su + dz) * side + px * su;
                    for dx in 0..su {
                        h = h.min(heights[row + dx]);
                    }
                }
                let i = px + pz * PAD;
                top[i] = (h.max(SEA_LEVEL) + 1).div_euclid(s) - 1;
                blocks[i] = if h < SEA_LEVEL {
                    (self.blocks.water, self.blocks.water)
                } else {
                    self.surface_blocks(x0 - s + (px as i32) * s, z0 - s + (pz as i32) * s, h)
                };
            }
        }

        // Árboles simplificados: columnas de bloques sobre el terreno.
        let mut extras: Vec<LodExtra> = Vec::new();
        let mut push = |column: usize, from: i32, to: i32, block: Block| {
            extras.push(LodExtra { column: column as u16, from: from as i16, to: to as i16, block: block.0 });
        };
        for tree in trees {
            let cell_of = |w: i32, origin: i32| ((w - origin).div_euclid(s)).clamp(0, CHUNK_SIZE_I - 1);
            let crown_from = (tree.base + tree.trunk).div_euclid(s);
            let crown_to = (tree.base + tree.trunk + CANOPY_HEIGHT).div_euclid(s);
            for cz in cell_of(tree.z - 1, z0)..=cell_of(tree.z + 1, z0) {
                for cx in cell_of(tree.x - 1, x0)..=cell_of(tree.x + 1, x0) {
                    let i = (cx + 1) as usize + (cz + 1) as usize * PAD;
                    // Con celdas de 4 m o más no cabe un tronco: la copa baja hasta el suelo.
                    let from = if level >= 2 { top[i] + 1 } else { crown_from.max(top[i] + 1) };
                    push(i, from, crown_to.max(from), tree.leaves);
                }
            }
            if level < 2 {
                let i = (cell_of(tree.x, x0) + 1) as usize + (cell_of(tree.z, z0) + 1) as usize * PAD;
                if crown_from > top[i] + 1 {
                    push(i, top[i] + 1, crown_from - 1, self.blocks.wood);
                }
            }
        }

        let surface: Vec<u8> = blocks.iter().map(|b| b.0 .0).collect();
        let subsoil: Vec<u8> = blocks.iter().map(|b| b.1 .0).collect();
        LodTile::encode(key, &top, &surface, &subsoil, self.blocks.stone.0, extras)
    }

    /// Descarta las columnas cacheadas lejos del jugador.
    pub fn trim_cache(&self, cx: i32, cz: i32, radius: i32) {
        self.columns
            .lock()
            .unwrap()
            .retain(|&(x, z), _| (x - cx).abs() <= radius && (z - cz).abs() <= radius);
    }
}

/// Escribe en el chunk la parte del árbol que cae dentro de él.
fn place_tree(
    data: &mut [u8; CHUNK_VOLUME],
    tree: &Tree,
    wood: Block,
    x0: i32,
    y0: i32,
    z0: i32,
) {
    let tree_top = tree.base + tree.trunk + CANOPY_HEIGHT;
    if tree_top < y0 || tree.base + 1 > y0 + CHUNK_SIZE_I - 1 {
        return;
    }
    let (lx, lz) = (tree.x - x0, tree.z - z0);
    let range = 0..CHUNK_SIZE_I;
    let mut set = |x: i32, y: i32, z: i32, block: Block, replace: bool| {
        if range.contains(&x) && range.contains(&y) && range.contains(&z) {
            let i = Chunk::index(x as usize, y as usize, z as usize);
            if replace || data[i] == 0 {
                data[i] = block.0;
            }
        }
    };

    for y in tree.base + 1..=tree.base + tree.trunk {
        set(lx, y - y0, lz, wood, true);
    }

    // Copa: (radio, ¿recortar esquinas?) por capa, de abajo arriba. Las hojas no pisan
    // nada que ya exista (tronco, terreno).
    const LAYERS: [(i32, bool); 4] = [(2, true), (2, true), (1, false), (1, true)];
    for (layer, &(radius, cut_corners)) in LAYERS.iter().enumerate() {
        let y = tree.base + tree.trunk + layer as i32 - y0;
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if cut_corners && dx.abs() == radius && dz.abs() == radius {
                    continue;
                }
                set(lx + dx, y, lz + dz, tree.leaves, false);
            }
        }
    }
}
