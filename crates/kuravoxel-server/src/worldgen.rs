//! Generación procedural del terreno.
//!
//! Los chunks son cúbicos, así que varios chunks apilados comparten la misma columna
//! (cx, cz). El mapa de alturas y los árboles se calculan una sola vez por columna y se
//! cachean: los chunks de cielo o de roca maciza se resuelven sin tocar un solo voxel.

use std::sync::{Arc, Mutex, OnceLock};

use glam::IVec3;
use rustc_hash::FxHashMap;

use kuravoxel_common::block::Block;
use anyhow::Context;
use kuravoxel_common::chunk::{Chunk, CHUNK_SIZE, CHUNK_SIZE_I, CHUNK_VOLUME};
use kuravoxel_common::content::ContentData;
use kuravoxel_common::lod::{LodExtra, LodTile, TileKey, TILE_AREA, TILE_CELLS as PAD};
use kuravoxel_common::protocol::WorldBounds;

use crate::noise::{fbm2, hash2, perlin2, smoothstep};

pub const SEA_LEVEL: i32 = 0;

/// Forma del mundo, inspirada en Chile: una franja larga de norte (-Z) a sur (+Z), con
/// el océano al este (+X) y una cordillera muy alta al oeste (-X). Medidas de la zona
/// jugable, en metros: 5,12 km de este a oeste y ~32,8 km de norte a sur.
pub const WORLD_WIDTH: i32 = 5120;
pub const WORLD_LENGTH: i32 = 32768;
/// Terreno lejano por debajo de este nivel (en celdas de 2 m y 4 m) se calcula columna a
/// columna; por encima se muestrea, que es lo que permite ver a varios kilómetros.
const EXACT_LOD_LEVEL: u8 = 2;
/// Muestras por lado en cada celda de las teselas muestreadas.
const LOD_SAMPLES: i32 = 3;
/// Fracción de copas a partir de la que una celda gruesa (16 m o más) se pinta de bosque.
const FOREST_COVER: f32 = 0.25;
/// Capas de subsuelo (tierra/arena) entre la superficie y la roca.
const SUBSOIL_DEPTH: i32 = 3;
const ROCK_LINE: i32 = 150;
const SNOW_LINE: i32 = 320;

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

    /// Zona jugable del mundo, centrada en el origen.
    pub fn bounds(&self) -> WorldBounds {
        let (w, l) = (WORLD_WIDTH as f64 / 2.0, WORLD_LENGTH as f64 / 2.0);
        WorldBounds { min: [-w, -l], max: [w, l] }
    }

    /// Posición de oeste (0) a este (1) dentro de la franja jugable. La costa y la
    /// cordillera no son rectas: el perfil se desplaza a lo largo de z.
    fn across(&self, x: f64, z: f64) -> f32 {
        let wobble = fbm2(z * 0.0006, 0.5, 3, self.seed ^ 0xC0A5) as f64 * 420.0
            + fbm2(x * 0.003, z * 0.003, 2, self.seed ^ 0x3131) as f64 * 80.0;
        ((x + wobble) / WORLD_WIDTH as f64 + 0.5) as f32
    }

    /// Altura del terreno (Y del bloque sólido más alto) en una columna del mundo.
    ///
    /// Perfil de oeste a este: cordillera (hasta ~600 m), valle central (~25 m),
    /// cordillera de la costa (~60-90 m), costa y océano. Fuera de la zona jugable el
    /// perfil sigue: más cordillera al oeste y mar abierto al este.
    pub fn height_at(&self, wx: i32, wz: i32) -> i32 {
        let (x, z) = (wx as f64, wz as f64);
        let seed = self.seed;
        let u = self.across(x, z);

        let hills = fbm2(x * 0.008, z * 0.008, 4, seed ^ 0x9E37);

        // Cordillera: crece hacia el oeste, con crestas y macizos que varían de norte a sur.
        let range = 1.0 - smoothstep(0.04, 0.42, u);
        let ridge = (1.0 - fbm2(x * 0.0022, z * 0.0022, 5, seed ^ 0xC0FE).abs() * 2.0).clamp(0.0, 1.0);
        let massif = (0.7 + fbm2(x * 0.0008, z * 0.0008, 3, seed ^ 0x51ED) * 0.8).clamp(0.25, 1.2);
        let mountains = range.powf(1.5) * (140.0 + 460.0 * ridge * ridge * massif) + range * 30.0 * ridge;

        // Cordillera de la costa: lomas entre el valle y el mar.
        let coastal = smoothstep(0.56, 0.66, u) * (1.0 - smoothstep(0.74, 0.84, u));
        let coastal_hills =
            coastal * (35.0 + 70.0 * fbm2(x * 0.004, z * 0.004, 3, seed ^ 0x7A11).abs());

        // Océano: el fondo baja hasta ~45 m bajo el nivel del mar.
        let ocean = smoothstep(0.84, 0.93, u);
        let base = 24.0 - ocean * 70.0;

        let height = base
            + mountains
            + coastal_hills
            + hills * (6.0 + 14.0 * range + 10.0 * coastal) * (1.0 - 0.8 * ocean);
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

    /// Candidato a árbol de la celda (gx, gz) de la rejilla de árboles: su posición, si la
    /// densidad de bosque de la zona lo deja. Barato: no mira la altura.
    fn tree_spot(&self, gx: i32, gz: i32) -> Option<(i32, i32)> {
        let h1 = hash2(gx, gz, self.seed ^ 0x7EE5);
        let tx = gx * TREE_CELL + 1 + (h1 % 5) as i32;
        let tz = gz * TREE_CELL + 1 + ((h1 >> 8) % 5) as i32;
        let forest = perlin2(tx as f64 * 0.008, tz as f64 * 0.008, self.seed ^ 0xF0E5);
        let density = 0.04 + 0.5 * smoothstep(-0.05, 0.35, forest);
        (((h1 >> 16) & 0xFF) as f32 <= density * 255.0).then_some((tx, tz))
    }

    /// El árbol de un candidato, si el suelo (`base`) es de hierba.
    fn grow_tree(&self, gx: i32, gz: i32, tx: i32, tz: i32, base: i32) -> Option<Tree> {
        if self.surface_blocks(tx, tz, base).0 != self.blocks.grass {
            return None;
        }
        let h2 = hash2(gx, gz, self.seed ^ 0x1EAF);
        let trunk = 4 + (h2 % 3) as i32;
        let leaves = self.blocks.leaves[((h2 >> 8) % 4) as usize];
        Some(Tree { x: tx, z: tz, base, trunk, leaves })
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
                let Some((tx, tz)) = self.tree_spot(gx, gz) else { continue };
                let reaches = |t: i32, origin: i32| {
                    t >= origin - LEAF_RADIUS && t <= origin + CHUNK_SIZE_I - 1 + LEAF_RADIUS
                };
                if !reaches(tx, x0) || !reaches(tz, z0) {
                    continue;
                }

                let (lx, lz) = (tx - x0, tz - z0);
                let inside = (0..CHUNK_SIZE_I).contains(&lx) && (0..CHUNK_SIZE_I).contains(&lz);
                let base = if inside {
                    heights[lx as usize + lz as usize * CHUNK_SIZE] as i32
                } else {
                    self.height_at(tx, tz)
                };
                let Some(tree) = self.grow_tree(gx, gz, tx, tz, base) else { continue };
                top = top.max(base + tree.trunk + CANOPY_HEIGHT);
                trees.push(tree);
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
                    data[Chunk::index(x, (y - y0) as usize, z)] = block;
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
        if key.0 <= EXACT_LOD_LEVEL {
            self.lod_tile_exact(key)
        } else {
            self.lod_tile_sampled(key)
        }
    }

    /// Tesela gruesa (celdas de 8 m o más): la altura de cada celda es la mínima de unas
    /// pocas muestras, no de todas sus columnas. Cuesta lo mismo en cualquier nivel, que
    /// es lo que permite ver a kilómetros. A estas distancias un posible saliente de un
    /// par de metros entre niveles no se aprecia.
    fn lod_tile_sampled(&self, key: TileKey) -> LodTile {
        let (level, tx, tz) = (key.0 as u32, key.1, key.2);
        let s = 1i32 << level;
        let span = CHUNK_SIZE_I * s;
        let (x0, z0) = (tx * span, tz * span);

        let mut top = [0i32; TILE_AREA];
        let mut heights = [0i32; TILE_AREA];
        let mut blocks = [(Block::AIR, Block::AIR); TILE_AREA];
        for pz in 0..PAD {
            for px in 0..PAD {
                let (cx, cz) = (x0 - s + px as i32 * s, z0 - s + pz as i32 * s);
                let mut h = i32::MAX;
                for j in 0..LOD_SAMPLES {
                    for i in 0..LOD_SAMPLES {
                        let offset = |k: i32| (2 * k + 1) * s / (2 * LOD_SAMPLES);
                        h = h.min(self.height_at(cx + offset(i), cz + offset(j)));
                    }
                }
                let i = px + pz * PAD;
                heights[i] = h;
                top[i] = (h.max(SEA_LEVEL) + 1).div_euclid(s) - 1;
                blocks[i] = if h < SEA_LEVEL {
                    (self.blocks.water, self.blocks.water)
                } else {
                    self.surface_blocks(cx + s / 2, cz + s / 2, h)
                };
            }
        }

        // Árboles: se recorren los candidatos de la tesela y se cuentan por celda. Con
        // celdas de 8 m cada árbol es un bloque de copa; más gruesas, la celda se pinta
        // del color del bosque si tiene suficientes.
        let mut counts: FxHashMap<usize, (u32, [u32; 4])> = FxHashMap::default();
        let cells = |w0: i32| w0.div_euclid(TREE_CELL)..=(w0 + span - 1).div_euclid(TREE_CELL);
        for gz in cells(z0) {
            for gx in cells(x0) {
                let Some((wx, wz)) = self.tree_spot(gx, gz) else { continue };
                let (lx, lz) = ((wx - x0).div_euclid(s), (wz - z0).div_euclid(s));
                if !(0..CHUNK_SIZE_I).contains(&lx) || !(0..CHUNK_SIZE_I).contains(&lz) {
                    continue;
                }
                let i = (lx + 1) as usize + (lz + 1) as usize * PAD;
                // Una celda bajo el agua o en roca no tiene árboles: ni se calcula la altura.
                if blocks[i].0 != self.blocks.grass {
                    continue;
                }
                let Some(tree) = self.grow_tree(gx, gz, wx, wz, self.height_at(wx, wz)) else {
                    continue;
                };
                let kind = self.blocks.leaves.iter().position(|&b| b == tree.leaves).unwrap_or(0);
                let entry = counts.entry(i).or_default();
                entry.0 += 1;
                entry.1[kind] += 1;
            }
        }
        let mut extras: Vec<LodExtra> = Vec::new();
        let per_cell = (s * s) as f32 / (TREE_CELL * TREE_CELL) as f32;
        let needed = ((per_cell * FOREST_COVER).ceil() as u32).max(1);
        for (&i, &(count, kinds)) in &counts {
            let kind = (0..4).max_by_key(|&k| kinds[k]).unwrap_or(0);
            let leaves = self.blocks.leaves[kind];
            if level == 3 {
                // Copa de un árbol medio (tronco de 4-6 m) sobre el suelo de la celda.
                let from = top[i] + 1;
                let crown = (heights[i] + 5 + CANOPY_HEIGHT).div_euclid(s);
                extras.push(LodExtra {
                    column: i as u16,
                    from: from as i16,
                    to: crown.max(from) as i16,
                    block: leaves,
                });
            } else if count >= needed {
                blocks[i].0 = leaves;
            }
        }

        let surface: Vec<Block> = blocks.iter().map(|b| b.0).collect();
        let subsoil: Vec<Block> = blocks.iter().map(|b| b.1).collect();
        LodTile::encode(key, &top, &surface, &subsoil, self.blocks.stone, extras)
    }

    /// Tesela fina (celdas de 2 m y 4 m), calculada columna a columna.
    fn lod_tile_exact(&self, key: TileKey) -> LodTile {
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
            extras.push(LodExtra { column: column as u16, from: from as i16, to: to as i16, block });
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

        let surface: Vec<Block> = blocks.iter().map(|b| b.0).collect();
        let subsoil: Vec<Block> = blocks.iter().map(|b| b.1).collect();
        LodTile::encode(key, &top, &surface, &subsoil, self.blocks.stone, extras)
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
    data: &mut [Block; CHUNK_VOLUME],
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
            if replace || data[i] == Block::AIR {
                data[i] = block;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn generator() -> WorldGen {
        // Números de bloque cualesquiera: aquí solo importa que sean distintos.
        let b = Block;
        WorldGen::new(
            1337,
            TerrainBlocks {
                grass: b(1),
                dirt: b(2),
                stone: b(3),
                sand: b(4),
                water: b(5),
                snow: b(6),
                wood: b(7),
                leaves: [b(8), b(9), b(10), b(11)],
            },
        )
    }

    /// Alturas media y máxima de una franja norte-sur en x.
    fn band(generator: &WorldGen, x: i32) -> (f32, i32) {
        let heights: Vec<i32> =
            (-WORLD_LENGTH / 2..WORLD_LENGTH / 2).step_by(97).map(|z| generator.height_at(x, z)).collect();
        let mean = heights.iter().sum::<i32>() as f32 / heights.len() as f32;
        (mean, *heights.iter().max().unwrap())
    }

    #[test]
    fn mountains_to_the_west_valley_in_the_middle_sea_to_the_east() {
        let generator = generator();
        let (west, west_peak) = band(&generator, -WORLD_WIDTH / 2 + 100);
        let (valley, _) = band(&generator, 0);
        let (east, east_peak) = band(&generator, WORLD_WIDTH / 2 - 50);
        assert!(west > 200.0 && west_peak > 400, "west {west} (peak {west_peak})");
        assert!((5.0..80.0).contains(&valley), "valley {valley}");
        assert!(east < -20.0 && east_peak < SEA_LEVEL, "east {east} (peak {east_peak})");
        // Fuera de la zona jugable el perfil sigue: más cordillera y más mar.
        assert!(band(&generator, -WORLD_WIDTH).0 > 200.0);
        assert!(band(&generator, WORLD_WIDTH).1 < SEA_LEVEL);
    }

    #[test]
    fn far_tiles_of_every_level_build() {
        let generator = generator();
        for level in kuravoxel_common::lod::MIN_LEVEL..=5 {
            // Una tesela en la cordillera, que es lo más caro.
            let size = CHUNK_SIZE_I << level;
            let key = (level, (-WORLD_WIDTH / 2 + 300).div_euclid(size), 0);
            let start = std::time::Instant::now();
            let tile = generator.lod_tile(key);
            let columns = tile.decode().expect("valid tile");
            println!("level {level}: {:?}, {} extras", start.elapsed(), tile.extras.len());
            assert!(columns.top.iter().any(|&top| top * (1 << level) > 100), "level {level} is mountainous");
        }
    }
}
