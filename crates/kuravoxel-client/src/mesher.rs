//! Mallador greedy.
//!
//! Cada cara visible se fusiona con sus vecinas coplanares del mismo bloque en un único
//! quad, que ocupa 8 bytes y se dibuja como instancia (los 4 vértices los genera el vertex
//! shader). Junto a cada quad viajan dos datos que el fragment shader necesita:
//!
//! * **Aristas** (4 bits): qué lados del quad son aristas convexas reales del volumen.
//!   El shader inclina la normal solo junto a esas aristas, así el borde se ve redondeado
//!   sin un solo triángulo extra y las superficies grandes siguen planas.
//! * **Oclusión ambiental** (4 × 2 bits): una por esquina, interpolada en el shader.
//!
//! El mallado trabaja sobre una copia acolchada de 34³ (el chunk más una capa de sus 26
//! vecinos) para que caras, aristas y oclusión sean correctas en los bordes del chunk.

use std::cell::RefCell;
use std::sync::Arc;

use voxel_common::block::Block;
use voxel_common::chunk::{Chunk, CHUNK_SIZE, CHUNK_SIZE_I};
use voxel_common::lod::{LodTile, TILE_CELLS};

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

/// Malla de un chunk: un `[u32; 2]` por quad.
///
/// ```text
/// [0]  x:5 | y:5 | z:5 | ancho-1:5 | alto-1:5 | cara:3 | aristas:4
/// [1]  bloque:8 | ao:8 | libre:16
/// ```
///
/// Los quads van agrupados por cara para poder descartar en CPU las caras que miran
/// en dirección contraria a la cámara.
#[derive(Default)]
pub struct ChunkMesh {
    pub quads: Vec<[u32; 2]>,
    /// Rango `(inicio, fin)` de quads de cada una de las 6 caras.
    pub face_ranges: [(u32, u32); 6],
}

struct Scratch {
    padded: Box<[u8; PAD_VOLUME]>,
    mask: [u32; CHUNK_SIZE * CHUNK_SIZE],
}

thread_local! {
    /// Buffers de trabajo reutilizados por cada hilo: mallar no reserva más memoria que
    /// la de la propia malla.
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch {
        padded: vec![0u8; PAD_VOLUME].into_boxed_slice().try_into().expect("padded size"),
        mask: [0; CHUNK_SIZE * CHUNK_SIZE],
    });
}

/// Casos que no producen geometría y no merece la pena mandar a un hilo: chunk de aire,
/// o chunk macizo rodeado de chunks macizos.
pub fn is_trivially_empty(hood: &Neighborhood) -> bool {
    match hood[CENTER].uniform() {
        Some(Block::AIR) => true,
        Some(_) => FACE_NEIGHBORS
            .iter()
            .all(|&i| matches!(hood[i].uniform(), Some(block) if block.is_solid())),
        None => false,
    }
}

pub fn mesh_chunk(hood: &Neighborhood) -> ChunkMesh {
    if is_trivially_empty(hood) {
        return ChunkMesh::default();
    }
    SCRATCH.with(|scratch| {
        let scratch = &mut *scratch.borrow_mut();
        fill_padded(&mut scratch.padded, hood);
        build(&scratch.padded, &mut scratch.mask)
    })
}

/// Malla un volumen acolchado de 34³ que rellena el propio llamador (índice
/// `x + y * PAD + z * PAD * PAD`, con el chunk en 1..=32).
fn mesh_padded(fill: impl FnOnce(&mut [u8; PAD_VOLUME])) -> ChunkMesh {
    SCRATCH.with(|scratch| {
        let scratch = &mut *scratch.borrow_mut();
        fill(&mut scratch.padded);
        build(&scratch.padded, &mut scratch.mask)
    })
}

// El terreno lejano llega ya con una celda de borde: mismo tamaño que el buffer acolchado.
const _: () = assert!(TILE_CELLS == PAD);

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
        let mesh = mesh_padded(|buf| {
            for column in 0..PAD * PAD {
                let top_row = to_row(columns.top[column]);
                let base = column % PAD + (column / PAD) * PAD * PAD;
                for py in 0..PAD as i32 {
                    buf[base + py as usize * PAD] = if py > top_row {
                        0
                    } else if py == top_row {
                        columns.surface[column]
                    } else if py == top_row - 1 {
                        columns.subsoil[column]
                    } else {
                        tile.deep
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
                        *cell = extra.block;
                    }
                }
            }
        });
        if !mesh.quads.is_empty() {
            meshes.push((chunk_y, mesh));
        }
    }
    meshes
}

/// Copia el chunk central y la capa exterior de sus vecinos al buffer de 34³.
fn fill_padded(buf: &mut [u8; PAD_VOLUME], hood: &Neighborhood) {
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
                        match chunk {
                            Chunk::Uniform(block) => row.fill(block.0),
                            Chunk::Dense(data) => {
                                let src = Chunk::index(lx, ly + y, lz + z);
                                row.copy_from_slice(&data[src..src + cx]);
                            }
                        }
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

fn build(buf: &[u8; PAD_VOLUME], mask: &mut [u32; CHUNK_SIZE * CHUNK_SIZE]) -> ChunkMesh {
    let mut mesh = ChunkMesh::default();

    for (face, &(normal_axis, positive, u_axis, v_axis)) in FACES.iter().enumerate() {
        let n_step = STRIDE[normal_axis] as isize * if positive { 1 } else { -1 };
        let u_step = STRIDE[u_axis] as isize;
        let v_step = STRIDE[v_axis] as isize;
        let range_start = mesh.quads.len() as u32;

        for depth in 0..CHUNK_SIZE {
            // Máscara de la rebanada. Cada celda con cara visible guarda:
            // bloque (bits 0-7) | ao (8-15) | aristas (16-19). Cero = sin cara.
            let base = (depth + 1) * STRIDE[normal_axis] + STRIDE[u_axis] + STRIDE[v_axis];
            let mut any = false;
            for v in 0..CHUNK_SIZE {
                for u in 0..CHUNK_SIZE {
                    let i = (base + u * STRIDE[u_axis] + v * STRIDE[v_axis]) as isize;
                    let block = buf[i as usize];
                    let above = i + n_step;
                    let cell = if block != 0 && buf[above as usize] == 0 {
                        any = true;
                        let solid = |offset: isize| (buf[(above + offset) as usize] != 0) as u32;
                        let (u_neg, u_pos) = (solid(-u_step), solid(u_step));
                        let (v_neg, v_pos) = (solid(-v_step), solid(v_step));
                        let ao = vertex_ao(u_neg, v_neg, solid(-u_step - v_step))
                            | vertex_ao(u_pos, v_neg, solid(u_step - v_step)) << 2
                            | vertex_ao(u_neg, v_pos, solid(v_step - u_step)) << 4
                            | vertex_ao(u_pos, v_pos, solid(u_step + v_step)) << 6;

                        // Arista convexa: no hay voxel al lado en el mismo plano.
                        let open = |offset: isize| (buf[(i + offset) as usize] == 0) as u32;
                        let edges = open(-u_step) * EDGE_U_NEG
                            | open(u_step) * EDGE_U_POS
                            | open(-v_step) * EDGE_V_NEG
                            | open(v_step) * EDGE_V_POS;

                        block as u32 | ao << 8 | edges << 16
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
                    let key = first & 0xFFFF;
                    let first_edges = first >> 16;

                    // Ancho: mismas celdas y mismas aristas en los lados -V / +V.
                    let mut width = 1;
                    let mut last_edges = first_edges;
                    while u0 + width < CHUNK_SIZE {
                        let cell = mask[u0 + width + v0 * CHUNK_SIZE];
                        if cell == 0
                            || cell & 0xFFFF != key
                            || (cell >> 16) & EDGE_V_MASK != first_edges & EDGE_V_MASK
                        {
                            break;
                        }
                        last_edges = cell >> 16;
                        width += 1;
                    }
                    let left = first_edges & EDGE_U_NEG;
                    let right = last_edges & EDGE_U_POS;

                    // Alto: filas completas que conserven las aristas de los extremos.
                    let mut height = 1;
                    let mut top = first_edges & EDGE_V_POS;
                    'grow: while v0 + height < CHUNK_SIZE {
                        let row = (v0 + height) * CHUNK_SIZE;
                        let row_top = (mask[u0 + row] >> 16) & EDGE_V_POS;
                        for k in 0..width {
                            let cell = mask[u0 + k + row];
                            let edges = cell >> 16;
                            if cell == 0
                                || cell & 0xFFFF != key
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
                    mesh.quads.push([
                        origin[0] as u32
                            | (origin[1] as u32) << 5
                            | (origin[2] as u32) << 10
                            | (width as u32 - 1) << 15
                            | (height as u32 - 1) << 20
                            | (face as u32) << 25
                            | edges << 28,
                        key,
                    ]);

                    u0 += width;
                }
            }
        }

        mesh.face_ranges[face] = (range_start, mesh.quads.len() as u32);
    }

    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRASS: Block = Block(1);
    const STONE: Block = Block(3);

    fn hood(center: Chunk, around: Block) -> Neighborhood {
        let filler = Arc::new(Chunk::Uniform(around));
        let mut hood: Neighborhood = std::array::from_fn(|_| filler.clone());
        hood[CENTER] = Arc::new(center);
        hood
    }

    fn size(quad: [u32; 2]) -> (u32, u32) {
        (((quad[0] >> 15) & 31) + 1, ((quad[0] >> 20) & 31) + 1)
    }

    fn edges(quad: [u32; 2]) -> u32 {
        quad[0] >> 28
    }

    #[test]
    fn single_voxel_is_six_fully_beveled_quads() {
        let mut data = Chunk::new_dense_buffer();
        data[Chunk::index(5, 6, 7)] = STONE.0;
        let mesh = mesh_chunk(&hood(Chunk::Dense(data), Block::AIR));

        assert_eq!(mesh.quads.len(), 6);
        for (face, &(start, end)) in mesh.face_ranges.iter().enumerate() {
            assert_eq!(end - start, 1, "face {face}");
            let quad = mesh.quads[start as usize];
            assert_eq!(quad[0] & 0x7FFF, 5 | 6 << 5 | 7 << 10);
            assert_eq!(size(quad), (1, 1));
            assert_eq!((quad[0] >> 25) & 7, face as u32);
            assert_eq!(edges(quad), 0b1111);
            assert_eq!(quad[1], STONE.0 as u32 | 0xFF << 8);
        }
    }

    #[test]
    fn slab_merges_into_one_quad_per_side() {
        let mut data = Chunk::new_dense_buffer();
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                data[Chunk::index(x, 0, z)] = GRASS.0;
            }
        }
        let mesh = mesh_chunk(&hood(Chunk::Dense(data), Block::AIR));

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
                    data[Chunk::index(x, 0, z)] = GRASS.0;
                }
            }
            Chunk::Dense(data)
        };
        let air = Arc::new(Chunk::Uniform(Block::AIR));
        let mut hood: Neighborhood = std::array::from_fn(|_| air.clone());
        // Misma altura (dy = 0): los 9 chunks del plano horizontal son losas.
        for dz in 0..3 {
            for dx in 0..3 {
                hood[dx + 3 + dz * 9] = Arc::new(slab());
            }
        }
        let mesh = mesh_chunk(&hood);

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
                data[Chunk::index(x, 0, z)] = GRASS.0;
                if x >= 16 {
                    data[Chunk::index(x, 1, z)] = GRASS.0;
                }
            }
        }
        let mesh = mesh_chunk(&hood(Chunk::Dense(data), Block::AIR));
        let (start, end) = mesh.face_ranges[2];
        // Suelo bajo despejado, suelo alto y la franja ocluida junto al escalón, que se
        // parte en 3 porque la oclusión se aclara en los dos extremos de la pared.
        assert_eq!(end - start, 5);
    }

    #[test]
    fn buried_and_empty_chunks_have_no_geometry() {
        let solid = hood(Chunk::Uniform(STONE), STONE);
        assert!(is_trivially_empty(&solid));
        assert!(mesh_chunk(&solid).quads.is_empty());

        let air = hood(Chunk::Uniform(Block::AIR), STONE);
        assert!(mesh_chunk(&air).quads.is_empty());

        let exposed = hood(Chunk::Uniform(STONE), Block::AIR);
        assert!(!is_trivially_empty(&exposed));
        assert_eq!(mesh_chunk(&exposed).quads.len(), 6);
    }
}
