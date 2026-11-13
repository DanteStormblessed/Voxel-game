//! Mallador greedy.
//!
//! Cada cara visible se fusiona con sus vecinas coplanares del mismo bloque en un único
//! quad, que ocupa 8 bytes y se dibuja como instancia (los 4 vértices los genera el vertex
//! shader). Junto a cada quad viajan los datos que el fragment shader necesita:
//!
//! * **Aristas** (4 bits): qué lados del quad son aristas convexas reales del volumen.
//!   El shader inclina la normal solo junto a esas aristas, así el borde se ve redondeado
//!   sin un solo triángulo extra y las superficies grandes siguen planas.
//! * **Oclusión ambiental** (4 × 2 bits): una por esquina, interpolada en el shader.
//! * **Luz** (4 bits): los bloques que emiten luz brillan sin sombra.
//!
//! Hay tres clases de bloque visible (ver `MeshTable`):
//!
//! * **Cubos opacos**: tapan las caras vecinas y hacen sombra.
//! * **Cubos translúcidos** (agua, cristal): sus quads van aparte, al final de la malla,
//!   para el segundo pase con mezcla. Entre dos iguales no hay cara.
//! * **Modelos** de mini-voxeles (flores, losas, escaleras): cada uno añade sus quads de
//!   1/8 de voxel, ya fusionados al cargar el contenido. No tapan a sus vecinos.
//!
//! El mallado trabaja sobre una copia acolchada de 34³ (el chunk más una capa de sus 26
//! vecinos) para que caras, aristas y oclusión sean correctas en los bordes del chunk.

use std::cell::RefCell;
use std::sync::Arc;

use kuravoxel_common::block::Block;
use kuravoxel_common::chunk::{Chunk, CHUNK_SIZE, CHUNK_SIZE_I};
use kuravoxel_common::content::ContentData;
use kuravoxel_common::lod::{LodTile, TILE_CELLS};
use kuravoxel_common::model::{cell_index, CellVoxels, CELL};
use kuravoxel_common::shape::BlockTable;

const PAD: usize = CHUNK_SIZE + 2;
const PAD_VOLUME: usize = PAD * PAD * PAD;
/// Paso en el buffer acolchado al avanzar un voxel en X, Y o Z.
const STRIDE: [usize; 3] = [1, PAD, PAD * PAD];

/// El chunk y sus 26 vecinos, indexados como (dx+1) + (dy+1)*3 + (dz+1)*9.
pub type Neighborhood = [Arc<Chunk>; 27];
const CENTER: usize = 13;
/// Índices de los 6 vecinos que comparten cara con el centro.
const FACE_NEIGHBORS: [usize; 6] = [12, 14, 10, 16, 4, 22];

/// (eje normal, ¿sentido positivo?, eje U, eje V). U × V = normal, de modo que los quads
/// salen en sentido antihorario vistos desde fuera. El shader usa las mismas tablas.
const FACES: [(usize, bool, usize, usize); 6] = [
    (0, true, 1, 2),  // +X
    (0, false, 2, 1), // -X
    (1, true, 2, 0),  // +Y
    (1, false, 0, 2), // -Y
    (2, true, 0, 1),  // +Z
    (2, false, 1, 0), // -Z
];

const EDGE_U_NEG: u32 = 1;
const EDGE_U_POS: u32 = 2;
const EDGE_V_NEG: u32 = 4;
const EDGE_V_POS: u32 = 8;
const EDGE_V_MASK: u32 = EDGE_V_NEG | EDGE_V_POS;
/// Parte de una celda de la máscara que debe coincidir para fusionar: bloque y oclusión.
const KEY_MASK: u32 = 0xFF_FFFF;

/// Entradas de la paleta de GPU por número de bloque: las 6 caras y 2 libres.
pub const PALETTE_STRIDE: usize = 8;
/// Bit de `quad[1]` que marca un quad de mini-voxeles.
const MINI: u32 = 1 << 31;

/// Malla de un chunk: un `[u32; 2]` por quad.
///
/// ```text
/// cubo  [0]  x:5 | y:5 | z:5 | ancho-1:5 | alto-1:5 | cara:3 | aristas:4
///       [1]  bloque:16 | ao:8 | luz:4 | libre:3 | mini=0:1
/// mini  [0]  celda x:5 y:5 z:5 | desplazamiento x:3 y:3 z:3 | ancho-1:3 | alto-1:3 | libre:2
///       [1]  color:20 | cara:3 | libre:1 | luz:4 | libre:3 | mini=1:1
/// ```
///
/// El color de un cubo está en la paleta en `bloque · 8 + cara`; el de un mini-voxel es
/// directamente su entrada de paleta. Los quads opacos van agrupados por cara para poder
/// descartar en CPU las caras que miran en dirección contraria a la cámara; los
/// translúcidos van al final, todos juntos.
#[derive(Default)]
pub struct ChunkMesh {
    pub quads: Vec<[u32; 2]>,
    /// Rango `(inicio, fin)` de quads opacos de cada una de las 6 caras.
    pub face_ranges: [(u32, u32); 6],
    /// Rango de quads translúcidos.
    pub transparent: (u32, u32),
}

// --- Tabla del mallador ---

const AIR: u8 = 0;
const OPAQUE: u8 = 1;
const TRANSPARENT: u8 = 2;
const MODEL: u8 = 3;

/// Un quad de mini-voxeles de un modelo, a falta de la celda y la luz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MiniQuad {
    word0: u32,
    word1: u32,
    /// Si está en el borde de la celda, qué casillas de ese lado ocupa (ver `plane_bit`);
    /// 0 si está dentro. Se omite si el vecino tapa todas: un cubo opaco o un modelo que
    /// ocupa esas casillas en su lado opuesto (dos losas juntas no tienen cara entre ellas).
    cover: u64,
}

/// Quads de una celda de modelo y qué ocupa en cada uno de sus 6 lados.
#[derive(Debug, PartialEq, Eq)]
struct ModelMesh {
    quads: [Vec<MiniQuad>; 6],
    /// Casillas ocupadas en la capa de borde de cada cara.
    cover: [u64; 6],
}

/// Bit de la casilla `p` en el plano perpendicular a `normal_axis`. Los dos ejes del
/// plano van siempre en orden creciente, así que las caras opuestas (+X y −X) usan las
/// mismas casillas.
fn plane_bit(normal_axis: usize, p: [usize; 3]) -> u64 {
    let (a, b) = match normal_axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    1 << (p[a] + p[b] * CELL)
}

/// Lo que el mallador necesita de cada número de bloque, en tablas planas.
pub struct MeshTable {
    kind: Box<[u8]>,
    light: Box<[u8]>,
    /// Índice en `models` de los bloques con modelo.
    model_of: Box<[u32]>,
    models: Vec<ModelMesh>,
    /// Primera entrada de la paleta de GPU de los colores de cada modelo.
    model_base: Vec<u32>,
}

impl Default for MeshTable {
    /// Sin contenido: todo lo que no es aire es un cubo opaco.
    fn default() -> Self {
        let mut kind = vec![OPAQUE; 1 << 16].into_boxed_slice();
        kind[0] = AIR;
        Self {
            kind,
            light: vec![0; 1 << 16].into_boxed_slice(),
            model_of: vec![u32::MAX; 1 << 16].into_boxed_slice(),
            models: Vec::new(),
            model_base: Vec::new(),
        }
    }
}

/// Paleta de GPU y tabla del mallador, construidas juntas para que los índices de color
/// de los modelos coincidan.
pub fn build_tables(content: &ContentData, blocks: &BlockTable) -> (MeshTable, Vec<[u8; 4]>) {
    let states = content.states.len().max(1);
    let mut palette = vec![[255, 0, 255, 255]; states * PALETTE_STRIDE];
    let mut table = MeshTable::default();

    // Colores de los modelos, detrás de los de los bloques.
    let mut model_base = Vec::with_capacity(content.models.len());
    for model in &content.models {
        model_base.push(palette.len() as u32);
        palette.extend(model.model.palette.iter().map(|&[r, g, b]| [r, g, b, 255]));
    }

    table.model_base = model_base.clone();
    for number in 0..states {
        let props = blocks.get(Block(number as u16));
        for (face, &[r, g, b]) in props.faces.iter().enumerate() {
            palette[number * PALETTE_STRIDE + face] = [r, g, b, props.opacity];
        }
        table.kind[number] = match props {
            p if !p.visible => AIR,
            p if p.model.is_some() => MODEL,
            p if p.transparent => TRANSPARENT,
            _ => OPAQUE,
        };
        table.light[number] = props.light;
        if let Some(cell) = &props.model {
            table.model_of[number] = table.models.len() as u32;
            table.models.push(mini_quads(&cell.voxels, model_base[cell.model as usize]));
        }
    }
    (table, palette)
}

/// Fusiona las caras visibles de una celda de modelo en quads, por cara.
fn mini_quads(cell: &CellVoxels, color_base: u32) -> ModelMesh {
    let mut out: [Vec<MiniQuad>; 6] = Default::default();
    let mut cover = [0u64; 6];
    let color_at = |p: [isize; 3]| {
        if p.iter().all(|&c| (0..CELL as isize).contains(&c)) {
            cell[cell_index(p[0] as usize, p[1] as usize, p[2] as usize)]
        } else {
            0
        }
    };
    for (face, &(normal_axis, positive, u_axis, v_axis)) in FACES.iter().enumerate() {
        let dir = if positive { 1 } else { -1 };
        for depth in 0..CELL {
            // Máscara de la rebanada: índice de color (0 = sin cara).
            let mut mask = [0u8; CELL * CELL];
            for v in 0..CELL {
                for u in 0..CELL {
                    let mut p = [0isize; 3];
                    p[normal_axis] = depth as isize;
                    p[u_axis] = u as isize;
                    p[v_axis] = v as isize;
                    let mut above = p;
                    above[normal_axis] += dir;
                    if color_at(above) == 0 {
                        mask[u + v * CELL] = color_at(p);
                    }
                }
            }
            let boundary = if positive { depth == CELL - 1 } else { depth == 0 };
            if boundary {
                for v in 0..CELL {
                    for u in 0..CELL {
                        let mut p = [0usize; 3];
                        p[normal_axis] = depth;
                        p[u_axis] = u;
                        p[v_axis] = v;
                        if cell[cell_index(p[0], p[1], p[2])] != 0 {
                            cover[face] |= plane_bit(normal_axis, p);
                        }
                    }
                }
            }
            for v0 in 0..CELL {
                let mut u0 = 0;
                while u0 < CELL {
                    let color = mask[u0 + v0 * CELL];
                    if color == 0 {
                        u0 += 1;
                        continue;
                    }
                    let mut width = 1;
                    while u0 + width < CELL && mask[u0 + width + v0 * CELL] == color {
                        width += 1;
                    }
                    let mut height = 1;
                    while v0 + height < CELL
                        && (0..width).all(|k| mask[u0 + k + (v0 + height) * CELL] == color)
                    {
                        height += 1;
                    }
                    for v in v0..v0 + height {
                        mask[u0 + v * CELL..u0 + width + v * CELL].fill(0);
                    }
                    let mut origin = [0u32; 3];
                    origin[normal_axis] = depth as u32;
                    origin[u_axis] = u0 as u32;
                    origin[v_axis] = v0 as u32;
                    let mut quad_cover = 0;
                    if boundary {
                        for v in v0..v0 + height {
                            for u in u0..u0 + width {
                                let mut p = [depth; 3];
                                p[u_axis] = u;
                                p[v_axis] = v;
                                quad_cover |= plane_bit(normal_axis, p);
                            }
                        }
                    }
                    out[face].push(MiniQuad {
                        word0: origin[0] << 15
                            | origin[1] << 18
                            | origin[2] << 21
                            | (width as u32 - 1) << 24
                            | (height as u32 - 1) << 27,
                        word1: (color_base + color as u32 - 1) | (face as u32) << 20 | MINI,
                        cover: quad_cover,
                    });
                    u0 += width;
                }
            }
        }
    }
    ModelMesh { quads: out, cover }
}

impl MeshTable {
    #[inline(always)]
    fn kind(&self, block: u16) -> u8 {
        self.kind[block as usize]
    }

    #[inline(always)]
    fn opaque(&self, block: u16) -> bool {
        self.kind[block as usize] == OPAQUE
    }

    /// Casillas que tapa el bloque `block` en su cara `face` (todas si es un cubo opaco).
    #[inline]
    fn cover(&self, block: u16, face: usize) -> u64 {
        match self.kind[block as usize] {
            OPAQUE => u64::MAX,
            MODEL => self.models[self.model_of[block as usize] as usize].cover[face],
            _ => 0,
        }
    }

    /// Entrada de la paleta de GPU del primer color del modelo `index` de
    /// `ContentData::models`.
    pub fn model_palette_base(&self, index: usize) -> u32 {
        self.model_base.get(index).copied().unwrap_or(0)
    }

    /// Las dos tablas dan la misma geometría (si no, hay que volver a mallar).
    pub fn same_geometry(&self, other: &MeshTable) -> bool {
        self.kind == other.kind && self.model_of == other.model_of && self.models == other.models
    }
}

struct Scratch {
    /// Números de bloque del chunk y su capa de vecinos.
    padded: Box<[u16; PAD_VOLUME]>,
    mask: [u32; CHUNK_SIZE * CHUNK_SIZE],
}

thread_local! {
    /// Buffers de trabajo reutilizados por cada hilo: mallar no reserva más memoria que
    /// la de la propia malla.
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch {
        padded: vec![0u16; PAD_VOLUME].into_boxed_slice().try_into().expect("padded size"),
        mask: [0; CHUNK_SIZE * CHUNK_SIZE],
    });
}

/// Casos que no producen geometría y no merece la pena mandar a un hilo: chunk de aire,
/// o chunk macizo y opaco rodeado de chunks macizos y opacos.
pub fn is_trivially_empty(hood: &Neighborhood, table: &MeshTable) -> bool {
    match hood[CENTER].uniform() {
        Some(Block::AIR) => true,
        Some(block) => {
            table.opaque(block.0)
                && FACE_NEIGHBORS
                    .iter()
                    .all(|&i| matches!(hood[i].uniform(), Some(block) if table.opaque(block.0)))
        }
        None => false,
    }
}

pub fn mesh_chunk(hood: &Neighborhood, table: &MeshTable) -> ChunkMesh {
    if is_trivially_empty(hood, table) {
        return ChunkMesh::default();
    }
    SCRATCH.with(|scratch| {
        let scratch = &mut *scratch.borrow_mut();
        fill_padded(&mut scratch.padded, hood);
        build(&scratch.padded, &mut scratch.mask, table)
    })
}

// El terreno lejano llega ya con una celda de borde: mismo tamaño que el buffer acolchado.
const _: () = assert!(TILE_CELLS == PAD);

thread_local! {
    /// A lo lejos todo es un cubo opaco (sin agua translúcida ni modelos): solo cuentan
    /// el relieve y sus colores.
    static LOD_TABLE: MeshTable = MeshTable::default();
}

/// Malla una tesela de terreno lejano recibida del servidor. Devuelve una malla por cada
/// chunk vertical (en unidades del nivel) que contiene superficie.
pub fn mesh_lod_tile(tile: &LodTile) -> Vec<(i32, ChunkMesh)> {
    let Some(columns) = tile.decode() else {
        log::warn!("far terrain tile {:?} has invalid data", tile.key);
        return Vec::new();
    };
    let lowest = columns.top.iter().copied().min().unwrap_or(0);
    let highest = tile
        .extras
        .iter()
        .map(|extra| extra.to as i32)
        .chain(columns.top.iter().copied())
        .max()
        .unwrap_or(0);

    let mut meshes = Vec::new();
    for chunk_y in lowest.div_euclid(CHUNK_SIZE_I)..=highest.div_euclid(CHUNK_SIZE_I) {
        // Fila del buffer acolchado que corresponde a la celda de altura `cy`.
        let to_row = |cy: i32| cy - chunk_y * CHUNK_SIZE_I + 1;
        let mesh = SCRATCH.with(|scratch| {
            let scratch = &mut *scratch.borrow_mut();
            let buf = &mut scratch.padded;
            for column in 0..PAD * PAD {
                let top_row = to_row(columns.top[column]);
                let base = column % PAD + (column / PAD) * PAD * PAD;
                for py in 0..PAD as i32 {
                    buf[base + py as usize * PAD] = if py > top_row {
                        0
                    } else if py == top_row {
                        columns.surface[column].0
                    } else if py == top_row - 1 {
                        columns.subsoil[column].0
                    } else {
                        tile.deep.0
                    };
                }
            }
            for extra in &tile.extras {
                let column = extra.column as usize;
                let base = column % PAD + (column / PAD) * PAD * PAD;
                let rows = to_row(extra.from as i32).max(0)..=to_row(extra.to as i32).min(PAD as i32 - 1);
                for py in rows {
                    let cell = &mut buf[base + py as usize * PAD];
                    if *cell == 0 {
                        *cell = extra.block.0;
                    }
                }
            }
            LOD_TABLE.with(|table| build(&scratch.padded, &mut scratch.mask, table))
        });
        if !mesh.quads.is_empty() {
            meshes.push((chunk_y, mesh));
        }
    }
    meshes
}

/// Copia el chunk central y la capa exterior de sus vecinos al buffer de 34³.
fn fill_padded(buf: &mut [u16; PAD_VOLUME], hood: &Neighborhood) {
    // Por eje: (inicio en el buffer acolchado, inicio en el chunk origen, longitud).
    const SPAN: [(usize, usize, usize); 3] = [
        (0, CHUNK_SIZE - 1, 1),
        (1, 0, CHUNK_SIZE),
        (PAD - 1, 0, 1),
    ];
    for nz in 0..3 {
        for ny in 0..3 {
            for nx in 0..3 {
                let (px, lx, cx) = SPAN[nx];
                let (py, ly, cy) = SPAN[ny];
                let (pz, lz, cz) = SPAN[nz];
                let chunk = &*hood[nx + ny * 3 + nz * 9];
                for z in 0..cz {
                    for y in 0..cy {
                        let dst = px + (py + y) * STRIDE[1] + (pz + z) * STRIDE[2];
                        let row = &mut buf[dst..dst + cx];
                        // `Block` es un u16 transparente: se copia directamente en el buffer.
                        chunk.copy_row(lx, ly + y, lz + z, bytemuck::cast_slice_mut(row));
                    }
                }
            }
        }
    }
}

/// Oclusión de una esquina (0 = muy ocluida, 3 = despejada) a partir de los dos voxeles
/// laterales y el diagonal de la capa situada sobre la cara.
#[inline(always)]
fn vertex_ao(side1: u32, side2: u32, corner: u32) -> u32 {
    if side1 & side2 == 1 {
        0
    } else {
        3 - (side1 + side2 + corner)
    }
}

fn build(buf: &[u16; PAD_VOLUME], mask: &mut [u32; CHUNK_SIZE * CHUNK_SIZE], table: &MeshTable) -> ChunkMesh {
    let mut mesh = ChunkMesh::default();
    let mut transparent: Vec<[u32; 2]> = Vec::new();

    for (face, &(normal_axis, positive, u_axis, v_axis)) in FACES.iter().enumerate() {
        let n_step = STRIDE[normal_axis] as isize * if positive { 1 } else { -1 };
        let u_step = STRIDE[u_axis] as isize;
        let v_step = STRIDE[v_axis] as isize;
        let range_start = mesh.quads.len() as u32;

        for depth in 0..CHUNK_SIZE {
            // Máscara de la rebanada. Cada celda con cara visible guarda:
            // bloque (bits 0-15) | ao (16-23) | aristas (24-27). Cero = sin cara.
            let base = (depth + 1) * STRIDE[normal_axis] + STRIDE[u_axis] + STRIDE[v_axis];
            let mut any = false;
            for v in 0..CHUNK_SIZE {
                for u in 0..CHUNK_SIZE {
                    let i = (base + u * STRIDE[u_axis] + v * STRIDE[v_axis]) as isize;
                    let block = buf[i as usize];
                    let above = i + n_step;
                    let neighbor = buf[above as usize];
                    // Un modelo que llena entera la cara que da a este bloque lo tapa
                    // igual que un cubo (la base de una losa sobre la piedra).
                    let hidden = table.cover(neighbor, face ^ 1) == u64::MAX;
                    let visible = match table.kind(block) {
                        OPAQUE => !hidden,
                        TRANSPARENT => neighbor != block && !hidden,
                        MODEL => {
                            // Los modelos se añaden tal cual, sin pasar por la fusión.
                            let model = &table.models[table.model_of[block as usize] as usize];
                            let light = (table.light[block as usize] as u32) << 24;
                            let mut cell = [0u32; 3];
                            cell[normal_axis] = depth as u32;
                            cell[u_axis] = u as u32;
                            cell[v_axis] = v as u32;
                            let cell = cell[0] | cell[1] << 5 | cell[2] << 10;
                            // Las caras opuestas son pares en `FACES`: +X/−X, +Y/−Y, +Z/−Z.
                            let covered = table.cover(neighbor, face ^ 1);
                            let shown = |q: &&MiniQuad| q.cover == 0 || q.cover & !covered != 0;
                            for quad in model.quads[face].iter().filter(shown) {
                                mesh.quads.push([quad.word0 | cell, quad.word1 | light]);
                            }
                            false
                        }
                        _ => false,
                    };
                    let cell = if visible {
                        any = true;
                        let solid = |offset: isize| table.opaque(buf[(above + offset) as usize]) as u32;
                        let (u_neg, u_pos) = (solid(-u_step), solid(u_step));
                        let (v_neg, v_pos) = (solid(-v_step), solid(v_step));
                        let ao = vertex_ao(u_neg, v_neg, solid(-u_step - v_step))
                            | vertex_ao(u_pos, v_neg, solid(u_step - v_step)) << 2
                            | vertex_ao(u_neg, v_pos, solid(v_step - u_step)) << 4
                            | vertex_ao(u_pos, v_pos, solid(u_step + v_step)) << 6;

                        // Arista convexa: al lado, en el mismo plano, no sigue el volumen.
                        let open = |offset: isize| {
                            let side = buf[(i + offset) as usize];
                            (side != block && !table.opaque(side)) as u32
                        };
                        let edges = open(-u_step) * EDGE_U_NEG
                            | open(u_step) * EDGE_U_POS
                            | open(-v_step) * EDGE_V_NEG
                            | open(v_step) * EDGE_V_POS;

                        block as u32 | ao << 16 | edges << 24
                    } else {
                        0
                    };
                    mask[u + v * CHUNK_SIZE] = cell;
                }
            }
            if !any {
                continue;
            }

            // Fusión greedy. Dos celdas vecinas con cara visible nunca tienen arista
            // convexa entre ellas, así que solo hay que vigilar las aristas del contorno:
            // deben ser uniformes en cada lado del rectángulo final.
            for v0 in 0..CHUNK_SIZE {
                let mut u0 = 0;
                while u0 < CHUNK_SIZE {
                    let first = mask[u0 + v0 * CHUNK_SIZE];
                    if first == 0 {
                        u0 += 1;
                        continue;
                    }
                    let key = first & KEY_MASK;
                    let first_edges = first >> 24;

                    // Ancho: mismas celdas y mismas aristas en los lados -V / +V.
                    let mut width = 1;
                    let mut last_edges = first_edges;
                    while u0 + width < CHUNK_SIZE {
                        let cell = mask[u0 + width + v0 * CHUNK_SIZE];
                        if cell == 0
                            || cell & KEY_MASK != key
                            || (cell >> 24) & EDGE_V_MASK != first_edges & EDGE_V_MASK
                        {
                            break;
                        }
                        last_edges = cell >> 24;
                        width += 1;
                    }
                    let left = first_edges & EDGE_U_NEG;
                    let right = last_edges & EDGE_U_POS;

                    // Alto: filas completas que conserven las aristas de los extremos.
                    let mut height = 1;
                    let mut top = first_edges & EDGE_V_POS;
                    'grow: while v0 + height < CHUNK_SIZE {
                        let row = (v0 + height) * CHUNK_SIZE;
                        let row_top = (mask[u0 + row] >> 24) & EDGE_V_POS;
                        for k in 0..width {
                            let cell = mask[u0 + k + row];
                            let edges = cell >> 24;
                            if cell == 0
                                || cell & KEY_MASK != key
                                || edges & EDGE_V_POS != row_top
                                || (k == 0 && edges & EDGE_U_NEG != left)
                                || (k == width - 1 && edges & EDGE_U_POS != right)
                            {
                                break 'grow;
                            }
                        }
                        top = row_top;
                        height += 1;
                    }

                    for v in v0..v0 + height {
                        let row = v * CHUNK_SIZE;
                        mask[u0 + row..u0 + width + row].fill(0);
                    }

                    let mut origin = [0usize; 3];
                    origin[normal_axis] = depth;
                    origin[u_axis] = u0;
                    origin[v_axis] = v0;
                    let edges = left | right | (first_edges & EDGE_V_NEG) | top;
                    let block = (key & 0xFFFF) as u16;
                    let quad = [
                        origin[0] as u32
                            | (origin[1] as u32) << 5
                            | (origin[2] as u32) << 10
                            | (width as u32 - 1) << 15
                            | (height as u32 - 1) << 20
                            | (face as u32) << 25
                            | edges << 28,
                        key | (table.light[block as usize] as u32) << 24,
                    ];
                    if table.kind(block) == TRANSPARENT {
                        transparent.push(quad);
                    } else {
                        mesh.quads.push(quad);
                    }

                    u0 += width;
                }
            }
        }

        mesh.face_ranges[face] = (range_start, mesh.quads.len() as u32);
    }

    let start = mesh.quads.len() as u32;
    mesh.quads.extend(transparent);
    mesh.transparent = (start, mesh.quads.len() as u32);
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuravoxel_common::content::{BlockDef, Loaded, ModelDef};
    use kuravoxel_common::model::Model;

    const GRASS: Block = Block(1);
    const STONE: Block = Block(3);

    fn hood(center: Chunk, around: Block) -> Neighborhood {
        let filler = Arc::new(Chunk::Uniform(around));
        let mut hood: Neighborhood = std::array::from_fn(|_| filler.clone());
        hood[CENTER] = Arc::new(center);
        hood
    }

    fn mesh(hood: &Neighborhood) -> ChunkMesh {
        mesh_chunk(hood, &MeshTable::default())
    }

    fn size(quad: [u32; 2]) -> (u32, u32) {
        (((quad[0] >> 15) & 31) + 1, ((quad[0] >> 20) & 31) + 1)
    }

    fn edges(quad: [u32; 2]) -> u32 {
        quad[0] >> 28
    }

    /// Contenido de prueba: piedra, cristal, lámpara y losa con modelo.
    fn content_tables() -> (ContentData, MeshTable, Vec<[u8; 4]>) {
        let slab = Model { size: [8, 4, 8], voxels: vec![1; 256], palette: vec![[10, 20, 30]] };
        let blocks = vec![
            BlockDef::simple("a:a_stone", "stone", [100, 100, 100]),
            BlockDef { transparent: true, opacity: 90, ..BlockDef::simple("a:b_glass", "glass", [200, 220, 255]) },
            BlockDef { light: 15, ..BlockDef::simple("a:c_lamp", "lamp", [255, 200, 0]) },
            BlockDef { model: Some("a:slab".into()), ..BlockDef::simple("a:d_slab", "slab", [1, 1, 1]) },
        ];
        let models = vec![ModelDef { id: "a:slab".into(), model: slab }];
        let content = ContentData::assign(Loaded { blocks, items: vec![], models }, &[], &[]).unwrap();
        let (table, palette) = build_tables(&content, &BlockTable::new(&content));
        (content, table, palette)
    }

    #[test]
    fn single_voxel_is_six_fully_beveled_quads() {
        let mut data = Chunk::new_dense_buffer();
        data[Chunk::index(5, 6, 7)] = STONE;
        let mesh = mesh(&hood(Chunk::from_dense(data), Block::AIR));

        assert_eq!(mesh.quads.len(), 6);
        for (face, &(start, end)) in mesh.face_ranges.iter().enumerate() {
            assert_eq!(end - start, 1, "face {face}");
            let quad = mesh.quads[start as usize];
            assert_eq!(quad[0] & 0x7FFF, 5 | 6 << 5 | 7 << 10);
            assert_eq!(size(quad), (1, 1));
            assert_eq!((quad[0] >> 25) & 7, face as u32);
            assert_eq!(edges(quad), 0b1111);
            assert_eq!(quad[1], STONE.0 as u32 | 0xFF << 16);
        }
    }

    #[test]
    fn slab_merges_into_one_quad_per_side() {
        let mut data = Chunk::new_dense_buffer();
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                data[Chunk::index(x, 0, z)] = GRASS;
            }
        }
        let mesh = mesh(&hood(Chunk::from_dense(data), Block::AIR));

        assert_eq!(mesh.quads.len(), 6);
        let (start, end) = mesh.face_ranges[2];
        assert_eq!(end - start, 1);
        let top = mesh.quads[start as usize];
        assert_eq!(size(top), (32, 32));
        assert_eq!(edges(top), 0b1111);
    }

    #[test]
    fn coplanar_neighbors_remove_border_bevels() {
        // Una losa que continúa en los chunks vecinos: la cara superior no tiene aristas.
        let slab = || {
            let mut data = Chunk::new_dense_buffer();
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    data[Chunk::index(x, 0, z)] = GRASS;
                }
            }
            Chunk::from_dense(data)
        };
        let air = Arc::new(Chunk::Uniform(Block::AIR));
        let mut hood: Neighborhood = std::array::from_fn(|_| air.clone());
        // Misma altura (dy = 0): los 9 chunks del plano horizontal son losas.
        for dz in 0..3 {
            for dx in 0..3 {
                hood[dx + 3 + dz * 9] = Arc::new(slab());
            }
        }
        let mesh = mesh(&hood);

        // Solo quedan la cara superior y la inferior, sin biseles.
        assert_eq!(mesh.quads.len(), 2);
        for quad in &mesh.quads {
            assert_eq!(size(*quad), (32, 32));
            assert_eq!(edges(*quad), 0);
        }
    }

    #[test]
    fn step_splits_top_face_by_occlusion() {
        // Un escalón: el suelo junto a la pared queda oscurecido y no se fusiona con el resto.
        let mut data = Chunk::new_dense_buffer();
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                data[Chunk::index(x, 0, z)] = GRASS;
                if x >= 16 {
                    data[Chunk::index(x, 1, z)] = GRASS;
                }
            }
        }
        let mesh = mesh(&hood(Chunk::from_dense(data), Block::AIR));
        let (start, end) = mesh.face_ranges[2];
        // Suelo bajo despejado, suelo alto y la franja ocluida junto al escalón, que se
        // parte en 3 porque la oclusión se aclara en los dos extremos de la pared.
        assert_eq!(end - start, 5);
    }

    #[test]
    fn buried_and_empty_chunks_have_no_geometry() {
        let table = MeshTable::default();
        let solid = hood(Chunk::Uniform(STONE), STONE);
        assert!(is_trivially_empty(&solid, &table));
        assert!(mesh(&solid).quads.is_empty());

        let air = hood(Chunk::Uniform(Block::AIR), STONE);
        assert!(mesh(&air).quads.is_empty());

        let exposed = hood(Chunk::Uniform(STONE), Block::AIR);
        assert!(!is_trivially_empty(&exposed, &table));
        assert_eq!(mesh(&exposed).quads.len(), 6);
    }

    #[test]
    fn palette_has_a_color_per_face_and_model_colors_after_the_blocks() {
        let (content, _, palette) = content_tables();
        let glass = content.block_id("a:b_glass").unwrap().0 as usize;
        assert_eq!(palette[glass * PALETTE_STRIDE + 2], [200, 220, 255, 90]);
        assert_eq!(palette[content.states.len() * PALETTE_STRIDE], [10, 20, 30, 255]);
    }

    #[test]
    fn glass_goes_to_the_transparent_range_and_lets_the_stone_show() {
        let (content, table, _) = content_tables();
        let (stone, glass) = (content.block_id("a:a_stone").unwrap(), content.block_id("a:b_glass").unwrap());
        let mut data = Chunk::new_dense_buffer();
        // Dos de cristal juntos sobre una piedra.
        data[Chunk::index(5, 5, 5)] = stone;
        data[Chunk::index(5, 6, 5)] = glass;
        data[Chunk::index(6, 6, 5)] = glass;
        let mesh = mesh_chunk(&hood(Chunk::from_dense(data), Block::AIR), &table);

        // La piedra conserva sus 6 caras (el cristal no tapa). El cristal no tiene cara
        // entre los dos ni sobre la piedra: arriba y a los lados se funde, pero las caras
        // ±Z se parten porque solo una de las dos celdas tiene arista abajo (bajo la otra
        // está la piedra). 8 quads translúcidos.
        let (start, end) = mesh.transparent;
        assert_eq!((start, end - start), (6, 8));
        // Sin sombra sobre la piedra: el cristal no hace oclusión.
        let top = mesh.quads[mesh.face_ranges[2].0 as usize];
        assert_eq!((top[1] >> 16) & 0xFF, 0xFF);
    }

    #[test]
    fn lamps_carry_their_light_and_models_add_mini_quads() {
        let (content, table, _) = content_tables();
        let (lamp, slab, stone) = (
            content.block_id("a:c_lamp").unwrap(),
            content.block_id("a:d_slab").unwrap(),
            content.block_id("a:a_stone").unwrap(),
        );
        let mut data = Chunk::new_dense_buffer();
        data[Chunk::index(1, 1, 1)] = lamp;
        data[Chunk::index(9, 9, 9)] = slab;
        data[Chunk::index(9, 8, 9)] = stone;
        let mesh = mesh_chunk(&hood(Chunk::from_dense(data), Block::AIR), &table);
        let cube = |q: &&[u32; 2], block: Block| q[1] & MINI == 0 && q[1] & 0xFFFF == block.0 as u32;

        let lamp_quad = mesh.quads.iter().find(|q| cube(q, lamp)).unwrap();
        assert_eq!((lamp_quad[1] >> 24) & 15, 15);

        // La losa: 5 caras de mini-voxeles (la de abajo la tapa la piedra), cada una en su
        // celda.
        let minis: Vec<_> = mesh.quads.iter().filter(|q| q[1] & MINI != 0).collect();
        assert_eq!(minis.len(), 5);
        for quad in &minis {
            assert_eq!(quad[0] & 0x7FFF, 9 | 9 << 5 | 9 << 10);
        }
        // Cara superior a media altura (fila y = 3, la última llena) y de 8×8.
        let top = minis.iter().find(|q| (q[1] >> 20) & 7 == 2).unwrap();
        assert_eq!(((top[0] >> 18) & 7, (top[0] >> 24) & 7, (top[0] >> 27) & 7), (3, 7, 7));
        // La base de la losa llena entera la cara de arriba de la piedra: esa cara no se
        // dibuja, como si la losa fuera opaca por ese lado.
        assert_eq!(mesh.quads.iter().filter(|q| cube(q, stone)).count(), 5);
    }

    #[test]
    fn neighboring_slabs_have_no_faces_between_them() {
        let (content, table, _) = content_tables();
        let slab = content.block_id("a:d_slab").unwrap();
        let mut data = Chunk::new_dense_buffer();
        data[Chunk::index(4, 4, 4)] = slab;
        data[Chunk::index(5, 4, 4)] = slab;
        let mesh = mesh_chunk(&hood(Chunk::from_dense(data), Block::AIR), &table);
        // 6 caras cada una menos las dos que se tocan.
        assert_eq!(mesh.quads.iter().filter(|q| q[1] & MINI != 0).count(), 10);

        // Una encima de otra no se tapan: entre ellas queda medio bloque de aire.
        let mut data = Chunk::new_dense_buffer();
        data[Chunk::index(4, 4, 4)] = slab;
        data[Chunk::index(4, 5, 4)] = slab;
        let mesh = mesh_chunk(&hood(Chunk::from_dense(data), Block::AIR), &table);
        assert_eq!(mesh.quads.iter().filter(|q| q[1] & MINI != 0).count(), 12);
    }
}
