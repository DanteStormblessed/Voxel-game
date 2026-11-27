//! Generación procedural del terreno a partir de su definición por datos (`biomes.rs`).
//!
//! Los chunks son cúbicos, así que varios chunks apilados comparten la misma columna
//! (cx, cz). El mapa de alturas y los árboles se calculan una sola vez por columna y se
//! cachean: los chunks de cielo o de roca maciza se resuelven sin tocar un solo voxel.

use std::sync::{Arc, Mutex, OnceLock};

use glam::IVec3;
use rustc_hash::FxHashMap;

use kuravoxel_common::block::Block;
use kuravoxel_common::chunk::{Chunk, CHUNK_SIZE, CHUNK_SIZE_I, CHUNK_VOLUME};
use kuravoxel_common::lod::{LodExtra, LodTile, TileKey, TILE_AREA, TILE_CELLS as PAD};
use kuravoxel_common::protocol::WorldBounds;

pub use crate::biomes::SEA_LEVEL;
use crate::biomes::{Surface, Terrain, TreeRule, TreeShape};
use crate::noise::hash2;

/// Terreno lejano por debajo de este nivel (en celdas de 2 m y 4 m) se calcula columna a
/// columna; por encima se muestrea, que es lo que permite ver a varios kilómetros.
const EXACT_LOD_LEVEL: u8 = 2;
/// Muestras por lado en cada celda de las teselas muestreadas.
const LOD_SAMPLES: i32 = 3;
/// En celdas gruesas (16 m o más) no caben árboles sueltos: la celda se pinta de bosque
/// con esta probabilidad por unidad de densidad de árboles.
const FOREST_COVER: f32 = 1.6;
/// La pendiente de una columna se mide contra las que están a esta distancia.
const SLOPE_STEP: i32 = 2;

/// Los árboles se colocan como mucho uno por celda de esta rejilla (en voxeles).
const TREE_CELL: i32 = 7;
const LEAF_RADIUS: i32 = 2;
/// Altura de la copa redonda por encima del último bloque de tronco.
const CANOPY_HEIGHT: i32 = 3;

struct Tree {
    /// Posición del tronco en coordenadas de mundo.
    x: i32,
    z: i32,
    /// Altura del suelo bajo el tronco.
    base: i32,
    trunk: i32,
    wood: Block,
    leaves: Block,
    shape: TreeShape,
}

impl Tree {
    /// Y del bloque más alto del árbol.
    fn top(&self) -> i32 {
        match self.shape {
            TreeShape::Round => self.base + self.trunk + CANOPY_HEIGHT,
            TreeShape::Pine => self.base + self.trunk + 1,
            TreeShape::Bush => self.base + 2,
            TreeShape::Pillar => self.base + self.trunk,
        }
    }

    /// Y de la capa de hojas más baja.
    fn crown(&self) -> i32 {
        match self.shape {
            TreeShape::Round => self.base + self.trunk,
            TreeShape::Pine => self.base + (self.trunk / 3).max(2),
            TreeShape::Bush => self.base + 1,
            // Sin copa: empieza por encima del tronco.
            TreeShape::Pillar => self.base + self.trunk + 1,
        }
    }
}

struct ColumnData {
    /// Altura del bloque sólido más alto de cada (x, z), indexado como x + z * 32.
    heights: [i16; CHUNK_SIZE * CHUNK_SIZE],
    /// Bloques de cada (x, z).
    surface: Vec<Surface>,
    min: i32,
    /// Por debajo de esta Y todo es roca.
    rock: i32,
    /// Y más alta con algún bloque (terreno, agua, árboles o plantas). Por encima, aire.
    top: i32,
    /// Árboles cuya copa puede tocar esta columna (incluye los plantados en las vecinas).
    trees: Vec<Tree>,
}

type ColumnCell = Arc<OnceLock<ColumnData>>;

/// Pendiente de una columna de altura `h`, dadas las de `SLOPE_STEP` más allá en x y z.
fn slope(h: i32, hx: i32, hz: i32) -> f32 {
    // Módulo del gradiente: la misma pendiente mire hacia donde mire la ladera.
    (((hx - h).pow(2) + (hz - h).pow(2)) as f32).sqrt() / SLOPE_STEP as f32
}

pub struct WorldGen {
    seed: u32,
    terrain: Arc<Terrain>,
    columns: Mutex<FxHashMap<(i32, i32), ColumnCell>>,
}

impl WorldGen {
    pub fn new(seed: u32, terrain: Arc<Terrain>) -> Self {
        Self {
            seed,
            terrain,
            columns: Mutex::new(FxHashMap::default()),
        }
    }

    pub fn terrain(&self) -> &Arc<Terrain> {
        &self.terrain
    }

    /// Zona jugable del mundo, centrada en el origen.
    pub fn bounds(&self) -> WorldBounds {
        let (w, l) = (self.terrain.width as f64 / 2.0, self.terrain.length as f64 / 2.0);
        WorldBounds { min: [-w, -l], max: [w, l] }
    }

    /// Altura del terreno (Y del bloque sólido más alto) en una columna del mundo.
    pub fn height_at(&self, wx: i32, wz: i32) -> i32 {
        self.terrain.height(wx as f64, wz as f64).floor() as i32
    }

    /// Altura y bloques de una columna suelta (fuera de las que ya están calculadas).
    pub fn surface_at(&self, wx: i32, wz: i32) -> (i32, Surface) {
        let h = self.height_at(wx, wz);
        let steep = slope(h, self.height_at(wx + SLOPE_STEP, wz), self.height_at(wx, wz + SLOPE_STEP));
        (h, self.terrain.surface(wx, wz, h, steep))
    }

    /// Candidato a árbol de la celda (gx, gz) de la rejilla de árboles: su posición y su
    /// tipo, si la densidad del bioma lo deja. Barato: no mira la altura.
    fn tree_spot(&self, gx: i32, gz: i32) -> Option<(i32, i32, &TreeRule)> {
        let h1 = hash2(gx, gz, self.seed ^ 0x7EE5);
        let tx = gx * TREE_CELL + 1 + (h1 % 5) as i32;
        let tz = gz * TREE_CELL + 1 + ((h1 >> 8) % 5) as i32;
        let roll = (h1 >> 16) as f32 / 65536.0;
        self.terrain.tree(tx, tz, roll).map(|rule| (tx, tz, rule))
    }

    /// El árbol de un candidato sobre un suelo de altura `base`.
    fn grow_tree(&self, gx: i32, gz: i32, tx: i32, tz: i32, rule: &TreeRule, base: i32) -> Tree {
        let h2 = hash2(gx, gz, self.seed ^ 0x1EAF);
        let [low, high] = rule.trunk;
        let trunk = low + (h2 % (high - low + 1) as u32) as i32;
        let leaves = rule.leaves[((h2 >> 8) as usize) % rule.leaves.len()];
        Tree { x: tx, z: tz, base, trunk, wood: rule.wood, leaves, shape: rule.shape }
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
        // Alturas con un margen hacia +x y +z, para medir la pendiente.
        const GRID: usize = CHUNK_SIZE + SLOPE_STEP as usize;
        let mut grid = [0i32; GRID * GRID];
        for z in 0..GRID {
            for x in 0..GRID {
                grid[x + z * GRID] = self.height_at(x0 + x as i32, z0 + z as i32);
            }
        }

        let mut heights = [0i16; CHUNK_SIZE * CHUNK_SIZE];
        let mut surface = Vec::with_capacity(CHUNK_SIZE * CHUNK_SIZE);
        let (mut min, mut max, mut rock) = (i32::MAX, i32::MIN, i32::MAX);
        let mut plant_top = i32::MIN;
        let step = SLOPE_STEP as usize;
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let h = grid[x + z * GRID];
                let steep = slope(h, grid[x + step + z * GRID], grid[x + (z + step) * GRID]);
                let cell = self.terrain.surface(x0 + x as i32, z0 + z as i32, h, steep);
                heights[x + z * CHUNK_SIZE] = h as i16;
                min = min.min(h);
                max = max.max(h);
                rock = rock.min(h - cell.depth);
                if cell.plant != Block::AIR {
                    plant_top = plant_top.max(h + 1);
                }
                surface.push(cell);
            }
        }

        let mut top = if min < SEA_LEVEL { max.max(SEA_LEVEL) } else { max };
        top = top.max(plant_top);

        // Árboles: un candidato por celda de rejilla, con posición determinista por hash.
        // Se miran también las celdas de fuera cuya copa puede entrar en esta columna.
        let mut trees = Vec::new();
        let lo = |origin: i32| (origin - LEAF_RADIUS - (TREE_CELL - 1)).div_euclid(TREE_CELL);
        let hi = |origin: i32| (origin + CHUNK_SIZE_I - 1 + LEAF_RADIUS).div_euclid(TREE_CELL);
        for gz in lo(z0)..=hi(z0) {
            for gx in lo(x0)..=hi(x0) {
                let Some((tx, tz, rule)) = self.tree_spot(gx, gz) else { continue };
                let reaches = |t: i32, origin: i32| {
                    t >= origin - LEAF_RADIUS && t <= origin + CHUNK_SIZE_I - 1 + LEAF_RADIUS
                };
                if !reaches(tx, x0) || !reaches(tz, z0) {
                    continue;
                }

                let (lx, lz) = (tx - x0, tz - z0);
                let inside = (0..CHUNK_SIZE_I).contains(&lx) && (0..CHUNK_SIZE_I).contains(&lz);
                let (base, ground) = if inside {
                    let i = lx as usize + lz as usize * CHUNK_SIZE;
                    (heights[i] as i32, surface[i])
                } else {
                    self.surface_at(tx, tz)
                };
                if !rule.grows_on(ground.top, base) {
                    continue;
                }
                let tree = self.grow_tree(gx, gz, tx, tz, rule, base);
                top = top.max(tree.top());
                trees.push(tree);
            }
        }

        ColumnData { heights, surface, min, rock, top, trees }
    }

    /// Genera un chunk. Se llama desde los hilos de trabajo.
    pub fn generate(&self, pos: IVec3) -> Chunk {
        let cell = self.column(pos.x, pos.z);
        let column = cell.get().expect("column is initialized");
        let (stone, water) = (self.terrain.stone, self.terrain.water);

        let y0 = pos.y * CHUNK_SIZE_I;
        let y1 = y0 + CHUNK_SIZE_I - 1;
        if y0 > column.top {
            return Chunk::Uniform(Block::AIR);
        }
        if y1 < column.rock.min(column.min) {
            return Chunk::Uniform(stone);
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
                let cell = &column.surface[x + z * CHUNK_SIZE];
                for y in y0..=fill_top {
                    let block = if y > h {
                        water
                    } else if y == h {
                        cell.top
                    } else if y >= h - cell.depth {
                        cell.under
                    } else {
                        stone
                    };
                    data[Chunk::index(x, (y - y0) as usize, z)] = block;
                }
            }
        }

        for tree in &column.trees {
            place_tree(&mut data, tree, x0, y0, z0);
        }

        // Plantas: un bloque sobre la superficie, si nada lo ocupa ya.
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let cell = &column.surface[x + z * CHUNK_SIZE];
                let y = column.heights[x + z * CHUNK_SIZE] as i32 + 1;
                if cell.plant != Block::AIR && (y0..=y1).contains(&y) {
                    let slot = &mut data[Chunk::index(x, (y - y0) as usize, z)];
                    if *slot == Block::AIR {
                        *slot = cell.plant;
                    }
                }
            }
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

        let (stone, water) = (self.terrain.stone, self.terrain.water);
        let mut top = [0i32; TILE_AREA];
        let mut heights = [0i32; TILE_AREA];
        let mut blocks = [(Block::AIR, Block::AIR); TILE_AREA];
        for pz in 0..PAD {
            for px in 0..PAD {
                let (cx, cz) = (x0 - s + px as i32 * s, z0 - s + pz as i32 * s);
                let mut samples = [0i32; (LOD_SAMPLES * LOD_SAMPLES) as usize];
                let offset = |k: i32| (2 * k + 1) * s / (2 * LOD_SAMPLES);
                for j in 0..LOD_SAMPLES {
                    for i in 0..LOD_SAMPLES {
                        samples[(i + j * LOD_SAMPLES) as usize] = self.height_at(cx + offset(i), cz + offset(j));
                    }
                }
                let h = samples.iter().copied().min().unwrap_or(0);
                let i = px + pz * PAD;
                heights[i] = h;
                top[i] = (h.max(SEA_LEVEL) + 1).div_euclid(s) - 1;
                blocks[i] = if h < SEA_LEVEL {
                    (water, water)
                } else {
                    // La pendiente sale de las propias muestras (separadas un tercio de
                    // celda): no cuesta ninguna altura más.
                    let (mx, mz) = (cx + s / 2, cz + s / 2);
                    let gap = (offset(1) - offset(0)) as f32;
                    let (dx, dz) = ((samples[5] - samples[4]) as f32, (samples[7] - samples[4]) as f32);
                    let steep = (dx * dx + dz * dz).sqrt() / gap;
                    let cell = self.terrain.surface(mx, mz, h, steep);
                    (cell.top, cell.under)
                };
            }
        }

        let mut extras: Vec<LodExtra> = Vec::new();
        if level == 3 {
            // Con celdas de 8 m cada árbol es un bloque de copa en su celda.
            let mut crowns: FxHashMap<usize, (Block, i32)> = FxHashMap::default();
            let cells = |w0: i32| w0.div_euclid(TREE_CELL)..=(w0 + span - 1).div_euclid(TREE_CELL);
            for gz in cells(z0) {
                for gx in cells(x0) {
                    let Some((wx, wz, rule)) = self.tree_spot(gx, gz) else { continue };
                    let (lx, lz) = ((wx - x0).div_euclid(s), (wz - z0).div_euclid(s));
                    if !(0..CHUNK_SIZE_I).contains(&lx) || !(0..CHUNK_SIZE_I).contains(&lz) {
                        continue;
                    }
                    let i = (lx + 1) as usize + (lz + 1) as usize * PAD;
                    // El suelo de la celda vale por el del árbol: no se calcula su altura.
                    if !rule.has_crown() || !rule.grows_on(blocks[i].0, heights[i]) {
                        continue;
                    }
                    let tree = self.grow_tree(gx, gz, wx, wz, rule, heights[i]);
                    crowns.entry(i).or_insert((tree.leaves, tree.top()));
                }
            }
            for (i, (leaves, tree_top)) in crowns {
                let from = top[i] + 1;
                extras.push(LodExtra {
                    column: i as u16,
                    from: from as i16,
                    to: tree_top.div_euclid(s).max(from) as i16,
                    block: leaves,
                });
            }
            // Orden estable: la misma tesela sale siempre igual.
            extras.sort_by_key(|extra| extra.column);
        } else {
            // Más gruesas: la celda entera toma el color del bosque, más a menudo cuanto
            // más denso es.
            for pz in 0..PAD {
                for px in 0..PAD {
                    let i = px + pz * PAD;
                    let (mx, mz) = (x0 - s + px as i32 * s + s / 2, z0 - s + pz as i32 * s + s / 2);
                    let Some((density, rule)) = self.terrain.forest(mx, mz) else { continue };
                    if !rule.grows_on(blocks[i].0, heights[i]) {
                        continue;
                    }
                    let hash = hash2(mx, mz, self.seed ^ 0xF0E5);
                    if ((hash & 0xFFFF) as f32 / 65536.0) < density * FOREST_COVER {
                        blocks[i].0 = rule.leaves[(hash >> 16) as usize % rule.leaves.len()];
                    }
                }
            }
        }

        let surface: Vec<Block> = blocks.iter().map(|b| b.0).collect();
        let subsoil: Vec<Block> = blocks.iter().map(|b| b.1).collect();
        LodTile::encode(key, &top, &surface, &subsoil, stone, extras)
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
                    (self.terrain.water, self.terrain.water)
                } else {
                    // Pendiente en la primera columna de la celda, sin salirse de la rejilla.
                    let (gx, gz) = (px * su, pz * su);
                    let step = SLOPE_STEP as usize;
                    let own = heights[gx + gz * side];
                    let steep = slope(
                        own,
                        heights[(gx + step).min(side - 1) + gz * side],
                        heights[gx + (gz + step).min(side - 1) * side],
                    );
                    let cell =
                        self.terrain.surface(x0 - s + (px as i32) * s, z0 - s + (pz as i32) * s, h, steep);
                    (cell.top, cell.under)
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
            let crown_from = tree.crown().div_euclid(s);
            let crown_to = tree.top().div_euclid(s);
            let reach = if tree.shape == TreeShape::Pillar { -1 } else { 1 };
            for cz in cell_of(tree.z - reach, z0)..=cell_of(tree.z + reach, z0) {
                for cx in cell_of(tree.x - reach, x0)..=cell_of(tree.x + reach, x0) {
                    if reach < 0 {
                        break;
                    }
                    let i = (cx + 1) as usize + (cz + 1) as usize * PAD;
                    // Con celdas de 4 m o más no cabe un tronco: la copa baja hasta el suelo.
                    let from = if level >= 2 { top[i] + 1 } else { crown_from.max(top[i] + 1) };
                    push(i, from, crown_to.max(from), tree.leaves);
                }
            }
            if level < 2 {
                let i = (cell_of(tree.x, x0) + 1) as usize + (cell_of(tree.z, z0) + 1) as usize * PAD;
                if crown_from > top[i] + 1 {
                    push(i, top[i] + 1, crown_from - 1, tree.wood);
                }
            }
        }

        let surface: Vec<Block> = blocks.iter().map(|b| b.0).collect();
        let subsoil: Vec<Block> = blocks.iter().map(|b| b.1).collect();
        LodTile::encode(key, &top, &surface, &subsoil, self.terrain.stone, extras)
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
fn place_tree(data: &mut [Block; CHUNK_VOLUME], tree: &Tree, x0: i32, y0: i32, z0: i32) {
    if tree.top() < y0 || tree.base + 1 > y0 + CHUNK_SIZE_I - 1 {
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

    if tree.shape != TreeShape::Bush {
        for y in tree.base + 1..=tree.base + tree.trunk {
            set(lx, y - y0, lz, tree.wood, true);
        }
    }

    // Capa de hojas de un radio, con o sin esquinas. Las hojas no pisan nada que ya
    // exista (tronco, terreno).
    let mut layer = |y: i32, radius: i32, cut_corners: bool| {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if cut_corners && radius > 0 && dx.abs() == radius && dz.abs() == radius {
                    continue;
                }
                set(lx + dx, y - y0, lz + dz, tree.leaves, false);
            }
        }
    };
    match tree.shape {
        TreeShape::Round => {
            const LAYERS: [(i32, bool); 4] = [(2, true), (2, true), (1, false), (1, true)];
            for (index, &(radius, cut_corners)) in LAYERS.iter().enumerate() {
                layer(tree.base + tree.trunk + index as i32, radius, cut_corners);
            }
        }
        TreeShape::Pine => {
            // Faldas alternas que se estrechan hacia la punta.
            for y in tree.crown()..=tree.top() {
                let radius = match tree.top() - y {
                    0 => 0,
                    1 | 2 => 1,
                    from_tip => 1 + from_tip % 2,
                };
                layer(y, radius, true);
            }
        }
        TreeShape::Bush => {
            layer(tree.base + 1, 1, false);
            layer(tree.base + 2, 1, true);
        }
        TreeShape::Pillar => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biomes::{self, WorldSource};
    use kuravoxel_common::content::{self, ContentData};

    fn generator() -> WorldGen {
        WorldGen::new(1337, Arc::new(biomes::tests::terrain()))
    }

    fn content_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
    }

    /// El terreno del juego, compilado con el contenido de `content/`.
    fn game(solo: Option<&str>) -> WorldGen {
        let dir = content_dir();
        let data = content::load(&dir).and_then(|loaded| ContentData::assign(loaded, &[], &[])).expect("content");
        let source = WorldSource::load(&dir).expect("worldgen files");
        let terrain = Terrain::compile(&source, &data, 1337, solo).expect("terrain compiles");
        WorldGen::new(1337, Arc::new(terrain))
    }

    #[test]
    fn chunks_follow_the_height_map_and_the_surface_rules() {
        let generator = generator();
        let content = content::test_content(&biomes::tests::TEST_BLOCKS);
        let block = |name: &str| content.block_id(name).unwrap();
        // Columna de la llanura (norte): ~20 m, hierba sobre tierra sobre roca.
        let (cx, cz) = (2, -40);
        let chunks: Vec<Chunk> = (-1..=2).map(|cy| generator.generate(IVec3::new(cx, cy, cz))).collect();
        let at = |x: usize, y: i32, z: usize| {
            let cy = y.div_euclid(CHUNK_SIZE_I);
            chunks[(cy + 1) as usize].get(x, (y - cy * CHUNK_SIZE_I) as usize, z)
        };
        let mut trunks = 0;
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let h = generator.height_at(cx * CHUNK_SIZE_I + x as i32, cz * CHUNK_SIZE_I + z as i32);
                assert_eq!(at(x, h, z), block("base:grass"));
                assert_eq!(at(x, h - 1, z), block("base:dirt"));
                assert_eq!(at(x, h - 3, z), block("base:dirt"));
                assert_eq!(at(x, h - 4, z), block("base:stone"));
                let above = at(x, h + 1, z);
                assert!(above == Block::AIR || above == block("base:wood") || above == block("base:leaves"));
                trunks += (above == block("base:wood")) as u32;
            }
        }
        assert!(trunks > 3, "half of the tree cells have a tree ({trunks} trunks)");
        assert!(matches!(generator.generate(IVec3::new(cx, 5, cz)), Chunk::Uniform(Block::AIR)));
        assert!(matches!(generator.generate(IVec3::new(cx, -3, cz)), Chunk::Uniform(b) if b == block("base:stone")));
        // En las dunas del oeste (sur), bajo el mar: agua hasta el nivel 0 sobre arena.
        let sea = generator.generate(IVec3::new(-30, -1, 40));
        assert_eq!(sea.get(5, 31, 5), block("base:water"));
        assert_eq!(sea.get(5, 1, 5), block("base:sand"));
    }

    #[test]
    fn far_tiles_of_every_level_build() {
        let generator = generator();
        for level in kuravoxel_common::lod::MIN_LEVEL..=5 {
            let tile = generator.lod_tile((level, 0, 1));
            let columns = tile.decode().expect("valid tile");
            assert!(columns.top.iter().any(|&top| top * (1 << level) > 100), "level {level} reaches the dunes");
        }
        // Las llanuras tienen bosque también a lo lejos.
        assert!(!generator.lod_tile((1, 0, -30)).extras.is_empty());
        assert!(!generator.lod_tile((3, 0, -4)).extras.is_empty());
    }

    #[test]
    fn the_game_world_is_shaped_like_chile() {
        let generator = game(None);
        let terrain = generator.terrain().clone();
        assert_eq!(terrain.bands().len(), 9, "three zones of three biomes");
        let (width, length) = (terrain.width, terrain.length);
        // Alturas media y máxima de una franja norte-sur en x.
        let band = |x: i32| {
            let heights: Vec<i32> = (-length / 2..length / 2).step_by(97).map(|z| generator.height_at(x, z)).collect();
            (heights.iter().sum::<i32>() as f32 / heights.len() as f32, *heights.iter().max().unwrap())
        };
        let (west, west_peak) = band(-width / 2 + 40);
        let (east, east_peak) = band(width / 2 - 100);
        assert!(west < -15.0 && west_peak < SEA_LEVEL, "ocean to the west: {west} (peak {west_peak})");
        assert!(east > 180.0 && east_peak > 400, "mountains to the east: {east} (peak {east_peak})");
        // Fuera de la zona jugable el perfil sigue: más mar y más cordillera.
        assert!(band(-width).1 < SEA_LEVEL);
        assert!(band(width).0 > 180.0);
        // Cada bioma tiene tierra firme en su franja y todos generan chunks y teselas.
        for (index, biome) in terrain.bands().iter().enumerate() {
            let (z0, z1) = terrain.band_range(index);
            let z = (z0 + z1) / 2;
            assert_eq!(terrain.biome_at(0, z).id, biome.id);
            let land = (-width / 2..width / 2).step_by(64).filter(|&x| generator.height_at(x, z) > SEA_LEVEL).count();
            assert!(land > 20, "{} is mostly under water ({land} of 80 samples on land)", biome.id);
            let start = std::time::Instant::now();
            for cx in 0..4 {
                for cy in -1..3 {
                    generator.generate(IVec3::new(cx, cy, z.div_euclid(CHUNK_SIZE_I)));
                }
            }
            let chunks = start.elapsed();
            let start = std::time::Instant::now();
            generator.lod_tile((4, 0, z.div_euclid(CHUNK_SIZE_I << 4))).decode().expect("valid tile");
            println!("{:<24} 4 columns in {chunks:.1?}, far tile in {:.1?}", biome.id, start.elapsed());
        }
    }

    #[test]
    fn every_biome_works_alone() {
        let source = WorldSource::load(&content_dir()).expect("worldgen files");
        for id in source.biomes.keys() {
            let generator = game(Some(id));
            assert_eq!(generator.terrain().bands().len(), 1);
            generator.generate(IVec3::new(0, 0, 0));
        }
    }
}
