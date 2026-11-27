//! Modelos de mini-voxeles (8 por metro): flores, losas, escaleras, puertas, armas.
//!
//! Se dibujan en MagicaVoxel y se guardan como `.vox` en `content/<paquete>/models/`.
//! Ejes: la X de MagicaVoxel es el este (+X del mundo), su Z es arriba (+Y) y su +Y es el
//! norte (−Z), que es también el **frente** del modelo: un bloque con
//! `orientation = "horizontal"` gira el modelo para que su frente mire hacia `facing`.
//!
//! Un bloque ocupa una celda de 8³. Un modelo mayor (una puerta de 8×16×2) se reparte en
//! celdas: se centra en X y Z sobre una rejilla de celdas y se apoya en el suelo, y cada
//! bloque dice cuál de esas celdas dibuja (`cell`). Cada celda es un estado o bloque
//! distinto, como en Minecraft.

use anyhow::{bail, ensure, Context};
use serde::{Deserialize, Serialize};

use crate::chunk::{rle_decode, rle_encode};

/// Mini-voxeles por lado de una celda (un bloque).
pub const CELL: usize = 8;
pub const CELL_VOLUME: usize = CELL * CELL * CELL;
/// Lado máximo de un modelo, en mini-voxeles (16 m).
pub const MAX_MODEL_SIZE: usize = 128;
/// Cajas de colisión como mucho por estado; con más se usa la caja que lo envuelve todo.
pub const MAX_COLLISION_BOXES: usize = 16;

/// Índices de color (en la paleta del modelo, 0 = vacío) de una celda de 8³, en orden
/// x + y·8 + z·64 y ya en ejes del mundo.
pub type CellVoxels = [u8; CELL_VOLUME];

/// Caja en mini-voxeles dentro de una celda: mínimo (incluido) y máximo (excluido),
/// `[x0, y0, z0, x1, y1, z1]`, de 0 a 8.
pub type CellBox = [u8; 6];

#[inline]
pub const fn cell_index(x: usize, y: usize, z: usize) -> usize {
    x + y * CELL + z * CELL * CELL
}

/// Un modelo ya en ejes del mundo.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// Tamaño (x, y, z) en mini-voxeles.
    pub size: [usize; 3],
    /// Índice de color de cada mini-voxel (x + y·sx + z·sx·sy); 0 = vacío.
    pub voxels: Vec<u8>,
    /// Color sRGB de cada índice (el índice `i` es `palette[i - 1]`).
    pub palette: Vec<[u8; 3]>,
}

/// Forma en la que viaja un modelo.
#[derive(Serialize, Deserialize)]
struct ModelWire {
    size: [u16; 3],
    runs: Vec<(u16, u8)>,
    palette: Vec<[u8; 3]>,
}

impl Serialize for Model {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ModelWire {
            size: self.size.map(|s| s as u16),
            runs: rle_encode(&self.voxels),
            palette: self.palette.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Model {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let wire = ModelWire::deserialize(deserializer)?;
        let size = wire.size.map(|s| s as usize);
        if size.iter().any(|&s| s == 0 || s > MAX_MODEL_SIZE) || wire.palette.len() > 255 {
            return Err(D::Error::custom("model has an invalid size"));
        }
        let mut voxels = vec![0u8; size[0] * size[1] * size[2]];
        if !rle_decode(&wire.runs, &mut voxels)
            || voxels.iter().any(|&v| v as usize > wire.palette.len())
        {
            return Err(D::Error::custom("model data is corrupt"));
        }
        Ok(Model { size, voxels, palette: wire.palette })
    }
}

impl Model {
    /// El modelo girado `turns` cuartos de vuelta alrededor de la vertical, en el sentido
    /// que lleva el norte (−Z) al este (+X): el mismo de `Rotation::FrontToEast`.
    pub fn rotated_y(&self, turns: u8) -> Model {
        let mut model = self.clone();
        for _ in 0..turns % 4 {
            let [sx, sy, sz] = model.size;
            let mut voxels = vec![0u8; model.voxels.len()];
            for z in 0..sz {
                for y in 0..sy {
                    for x in 0..sx {
                        // (x, z) → (sz − 1 − z, x); el tamaño pasa a ser (sz, sy, sx).
                        voxels[(sz - 1 - z) + y * sz + x * sz * sy] = model.get(x, y, z);
                    }
                }
            }
            model = Model { size: [sz, sy, sx], voxels, palette: model.palette };
        }
        model
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> u8 {
        let [sx, sy, _] = self.size;
        self.voxels[x + y * sx + z * sx * sy]
    }

    /// Celdas que ocupa por eje.
    pub fn cells(&self) -> [usize; 3] {
        self.size.map(|s| s.div_ceil(CELL))
    }

    /// Mini-voxeles de una celda, sin girar. `None` si la celda no existe.
    pub fn cell(&self, cell: [u8; 3]) -> Option<CellVoxels> {
        let cells = self.cells();
        if (0..3).any(|axis| cell[axis] as usize >= cells[axis]) {
            return None;
        }
        // Centrado en X y Z sobre la rejilla de celdas; apoyado abajo en Y.
        let offset = [
            (cells[0] * CELL - self.size[0]) / 2,
            0,
            (cells[2] * CELL - self.size[2]) / 2,
        ];
        let mut out = [0u8; CELL_VOLUME];
        for z in 0..CELL {
            for y in 0..CELL {
                for x in 0..CELL {
                    let local = [x, y, z];
                    let mut at = [0usize; 3];
                    let inside = (0..3).all(|axis| {
                        let p = (cell[axis] as usize * CELL + local[axis]) as isize
                            - offset[axis] as isize;
                        at[axis] = p.max(0) as usize;
                        p >= 0 && (p as usize) < self.size[axis]
                    });
                    if inside {
                        out[cell_index(x, y, z)] = self.get(at[0], at[1], at[2]);
                    }
                }
            }
        }
        Some(out)
    }
}

/// Giro de una celda según el estado de su bloque.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    Identity,
    /// Lo que era arriba pasa a +X (tronco tumbado en X).
    UpToX,
    /// Lo que era arriba pasa a +Z.
    UpToZ,
    /// Giro alrededor de Y: el frente (−Z) pasa a +X, +Z o −X.
    FrontToEast,
    FrontToSouth,
    FrontToWest,
}

impl Rotation {
    /// Dónde acaba el mini-voxel (x, y, z) de una celda.
    pub fn apply(self, [x, y, z]: [usize; 3]) -> [usize; 3] {
        let m = CELL - 1;
        match self {
            Rotation::Identity => [x, y, z],
            Rotation::UpToX => [y, m - x, z],
            Rotation::UpToZ => [x, m - z, y],
            Rotation::FrontToEast => [m - z, y, x],
            Rotation::FrontToSouth => [m - x, y, m - z],
            Rotation::FrontToWest => [z, y, m - x],
        }
    }

    /// Hacia dónde acaba apuntando un vector (una dirección o un desplazamiento entre
    /// celdas) tras el giro.
    pub fn apply_dir(self, [x, y, z]: [i32; 3]) -> [i32; 3] {
        match self {
            Rotation::Identity => [x, y, z],
            Rotation::UpToX => [y, -x, z],
            Rotation::UpToZ => [x, -z, y],
            Rotation::FrontToEast => [-z, y, x],
            Rotation::FrontToSouth => [-x, y, -z],
            Rotation::FrontToWest => [z, y, -x],
        }
    }

    pub fn rotate(self, cell: &CellVoxels) -> CellVoxels {
        let mut out = [0u8; CELL_VOLUME];
        for z in 0..CELL {
            for y in 0..CELL {
                for x in 0..CELL {
                    let [rx, ry, rz] = self.apply([x, y, z]);
                    out[cell_index(rx, ry, rz)] = cell[cell_index(x, y, z)];
                }
            }
        }
        out
    }
}

/// Celda con todas las cajas dadas rellenas (para girar y descomponer igual que un modelo).
pub fn rasterize(boxes: &[CellBox]) -> CellVoxels {
    let mut out = [0u8; CELL_VOLUME];
    for b in boxes {
        for z in b[2]..b[5].min(CELL as u8) {
            for y in b[1]..b[4].min(CELL as u8) {
                for x in b[0]..b[3].min(CELL as u8) {
                    out[cell_index(x as usize, y as usize, z as usize)] = 1;
                }
            }
        }
    }
    out
}

/// Descompone los mini-voxeles ocupados de una celda en pocas cajas (fusión voraz en X,
/// luego Z, luego Y). Con más de `MAX_COLLISION_BOXES` devuelve la caja envolvente.
pub fn boxes(cell: &CellVoxels) -> Vec<CellBox> {
    let filled = |x: usize, y: usize, z: usize| cell[cell_index(x, y, z)] != 0;
    let mut used = [false; CELL_VOLUME];
    let mut out: Vec<CellBox> = Vec::new();
    for y in 0..CELL {
        for z in 0..CELL {
            for x in 0..CELL {
                if !filled(x, y, z) || used[cell_index(x, y, z)] {
                    continue;
                }
                let free = |x: usize, y: usize, z: usize| filled(x, y, z) && !used[cell_index(x, y, z)];
                let mut x1 = x + 1;
                while x1 < CELL && free(x1, y, z) {
                    x1 += 1;
                }
                let mut z1 = z + 1;
                while z1 < CELL && (x..x1).all(|i| free(i, y, z1)) {
                    z1 += 1;
                }
                let mut y1 = y + 1;
                while y1 < CELL && (z..z1).all(|k| (x..x1).all(|i| free(i, y1, k))) {
                    y1 += 1;
                }
                for k in z..z1 {
                    for j in y..y1 {
                        for i in x..x1 {
                            used[cell_index(i, j, k)] = true;
                        }
                    }
                }
                out.push([x as u8, y as u8, z as u8, x1 as u8, y1 as u8, z1 as u8]);
            }
        }
    }
    if out.len() > MAX_COLLISION_BOXES {
        let mut bound = [CELL as u8, CELL as u8, CELL as u8, 0, 0, 0];
        for b in &out {
            for axis in 0..3 {
                bound[axis] = bound[axis].min(b[axis]);
                bound[axis + 3] = bound[axis + 3].max(b[axis + 3]);
            }
        }
        return vec![bound];
    }
    out
}

// --- Lectura de .vox ---

fn read_u32(bytes: &[u8], at: usize) -> anyhow::Result<u32> {
    let slice = bytes.get(at..at + 4).context("truncated .vox file")?;
    Ok(u32::from_le_bytes(slice.try_into().expect("4 bytes")))
}

/// Lee un `.vox` de MagicaVoxel (el primer modelo del archivo) y lo pasa a ejes del mundo.
pub fn parse_vox(bytes: &[u8]) -> anyhow::Result<Model> {
    ensure!(bytes.get(0..4) == Some(b"VOX "), "not a MagicaVoxel .vox file");
    ensure!(bytes.get(8..12) == Some(b"MAIN"), "missing MAIN chunk");
    let mut size: Option<[usize; 3]> = None;
    let mut points: Option<Vec<[u8; 4]>> = None;
    let mut palette: Option<Vec<[u8; 3]>> = None;

    // Los hijos de MAIN van seguidos; solo interesan SIZE, XYZI y RGBA.
    let mut at = 20;
    while at + 12 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let content = read_u32(bytes, at + 4)? as usize;
        let children = read_u32(bytes, at + 8)? as usize;
        let body = bytes.get(at + 12..at + 12 + content).context("truncated .vox chunk")?;
        match id {
            // Solo el primer modelo: los archivos con varios (escenas) usan el primero.
            b"SIZE" if size.is_none() => {
                let s = [read_u32(body, 0)?, read_u32(body, 4)?, read_u32(body, 8)?];
                size = Some(s.map(|v| v as usize));
            }
            b"XYZI" if points.is_none() => {
                let count = read_u32(body, 0)? as usize;
                let data = body.get(4..4 + count * 4).context("truncated XYZI chunk")?;
                points = Some(data.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect());
            }
            b"RGBA" => {
                palette = Some(body.chunks_exact(4).take(255).map(|c| [c[0], c[1], c[2]]).collect());
            }
            _ => {}
        }
        at += 12 + content + children;
    }

    let [vx, vy, vz] = size.context("missing SIZE chunk")?;
    ensure!(
        [vx, vy, vz].iter().all(|&s| s > 0 && s <= MAX_MODEL_SIZE),
        "model is {vx}×{vy}×{vz}; the maximum is {MAX_MODEL_SIZE} per side"
    );
    let points = points.context("missing XYZI chunk")?;
    let Some(palette) = palette else {
        bail!("the file has no palette (RGBA chunk): save it again from MagicaVoxel");
    };

    // MagicaVoxel (x, y, z arriba) → mundo (x, y arriba, z sur): su +Y es nuestro −Z.
    let our = [vx, vz, vy];
    let mut used = vec![false; palette.len()];
    let mut voxels = vec![0u8; our[0] * our[1] * our[2]];
    for [x, y, z, color] in points {
        let (x, y, z) = (x as usize, y as usize, z as usize);
        ensure!(x < vx && y < vy && z < vz, "a voxel is outside the model");
        ensure!(color != 0 && (color as usize) <= palette.len(), "a voxel uses a missing color");
        let (wx, wy, wz) = (x, z, vy - 1 - y);
        voxels[wx + wy * our[0] + wz * our[0] * our[1]] = color;
        used[color as usize - 1] = true;
    }

    // Solo se guardan los colores que se usan, renumerados (paletas de 255 → unos pocos).
    let mut remap = vec![0u8; palette.len() + 1];
    let mut compact = Vec::new();
    for (index, color) in palette.iter().enumerate() {
        if used[index] {
            compact.push(*color);
            remap[index + 1] = compact.len() as u8;
        }
    }
    for voxel in &mut voxels {
        *voxel = remap[*voxel as usize];
    }
    Ok(Model { size: our, voxels, palette: compact })
}

/// Escribe un `.vox` mínimo (para tests y herramientas). `voxels` en ejes del mundo.
pub fn write_vox(model: &Model) -> Vec<u8> {
    let [sx, sy, sz] = model.size;
    // Mundo → MagicaVoxel: su x = x, su y = sz - 1 - z, su z = y.
    let mut points = Vec::new();
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                let color = model.get(x, y, z);
                if color != 0 {
                    points.extend_from_slice(&[x as u8, (sz - 1 - z) as u8, y as u8, color]);
                }
            }
        }
    }
    let chunk = |id: &[u8; 4], body: &[u8]| {
        let mut out = id.to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(body);
        out
    };
    let mut size = Vec::new();
    for v in [sx, sz, sy] {
        size.extend_from_slice(&(v as u32).to_le_bytes());
    }
    let mut xyzi = ((points.len() / 4) as u32).to_le_bytes().to_vec();
    xyzi.extend_from_slice(&points);
    let mut rgba = Vec::new();
    for i in 0..256 {
        let [r, g, b] = model.palette.get(i).copied().unwrap_or([0, 0, 0]);
        rgba.extend_from_slice(&[r, g, b, 255]);
    }
    let children = [chunk(b"SIZE", &size), chunk(b"XYZI", &xyzi), chunk(b"RGBA", &rgba)].concat();
    let mut out = b"VOX ".to_vec();
    out.extend_from_slice(&150u32.to_le_bytes());
    out.extend_from_slice(b"MAIN");
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(children.len() as u32).to_le_bytes());
    out.extend_from_slice(&children);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Modelo de prueba: sx×sy×sz con `fill(x, y, z)` como índice de color.
    fn model(size: [usize; 3], fill: impl Fn(usize, usize, usize) -> u8) -> Model {
        let mut voxels = vec![0; size[0] * size[1] * size[2]];
        for z in 0..size[2] {
            for y in 0..size[1] {
                for x in 0..size[0] {
                    voxels[x + y * size[0] + z * size[0] * size[1]] = fill(x, y, z);
                }
            }
        }
        Model { size, voxels, palette: vec![[255, 0, 0], [0, 255, 0]] }
    }

    #[test]
    fn vox_files_round_trip_in_world_axes() {
        // Una L: suelo verde y una columna roja en la esquina norte-oeste (x=0, z=0).
        let original = model([3, 4, 2], |x, y, z| match (x, y, z) {
            (_, 0, _) => 2,
            (0, _, 0) => 1,
            _ => 0,
        });
        let parsed = parse_vox(&write_vox(&original)).unwrap();
        assert_eq!(parsed.size, [3, 4, 2]);
        assert_eq!(parsed.get(0, 3, 0), original.get(0, 3, 0));
        assert_eq!(parsed.get(2, 0, 1), original.get(2, 0, 1));
        assert_eq!(parsed.get(2, 3, 1), 0);
        // La paleta se compacta a los colores usados, en su orden.
        assert_eq!(parsed.palette, [[255, 0, 0], [0, 255, 0]]);
    }

    #[test]
    fn broken_vox_files_are_rejected() {
        assert!(parse_vox(b"nope").is_err());
        let mut bytes = write_vox(&model([2, 2, 2], |_, _, _| 1));
        bytes.truncate(bytes.len() - 100);
        assert!(parse_vox(&bytes).is_err());
    }

    #[test]
    fn small_models_are_centered_and_large_ones_split_into_cells() {
        // 2×3×2 en el centro de la celda, apoyado en el suelo.
        let flower = model([2, 3, 2], |_, _, _| 1);
        let cell = flower.cell([0, 0, 0]).unwrap();
        assert_eq!(cell[cell_index(3, 0, 3)], 1);
        assert_eq!(cell[cell_index(4, 2, 4)], 1);
        assert_eq!(cell[cell_index(4, 3, 4)], 0);
        assert_eq!(cell[cell_index(2, 0, 3)], 0);
        assert!(flower.cell([0, 1, 0]).is_none());

        // Puerta de 8×16×2: dos celdas en vertical.
        let door = model([8, 16, 2], |_, y, _| if y < 8 { 1 } else { 2 });
        assert_eq!(door.cells(), [1, 2, 1]);
        assert_eq!(door.cell([0, 0, 0]).unwrap()[cell_index(0, 7, 3)], 1);
        assert_eq!(door.cell([0, 1, 0]).unwrap()[cell_index(0, 0, 3)], 2);
    }

    #[test]
    fn rotations_move_up_and_front_where_expected() {
        let mut cell = [0u8; CELL_VOLUME];
        cell[cell_index(0, 7, 0)] = 1; // arriba, en la esquina norte-oeste
        let up_to_x = Rotation::UpToX.rotate(&cell);
        assert_eq!(up_to_x[cell_index(7, 7, 0)], 1);
        let up_to_z = Rotation::UpToZ.rotate(&cell);
        assert_eq!(up_to_z[cell_index(0, 7, 7)], 1);

        let mut front = [0u8; CELL_VOLUME];
        front[cell_index(3, 0, 0)] = 1; // en el lado norte
        assert_eq!(Rotation::FrontToEast.rotate(&front)[cell_index(7, 0, 3)], 1);
        assert_eq!(Rotation::FrontToSouth.rotate(&front)[cell_index(4, 0, 7)], 1);
        assert_eq!(Rotation::FrontToWest.rotate(&front)[cell_index(0, 0, 4)], 1);
    }

    #[test]
    fn cells_decompose_into_few_boxes() {
        let slab = rasterize(&[[0, 0, 0, 8, 4, 8]]);
        assert_eq!(boxes(&slab), [[0, 0, 0, 8, 4, 8]]);
        // Escalera: losa más medio bloque detrás.
        let stairs = rasterize(&[[0, 0, 0, 8, 4, 8], [0, 4, 4, 8, 8, 8]]);
        assert_eq!(boxes(&stairs).len(), 2);
        // Un tablero de ajedrez no se trocea en 256 cajas: se envuelve.
        let mut checker = [0u8; CELL_VOLUME];
        for (i, v) in checker.iter_mut().enumerate() {
            *v = ((i + i / 8 + i / 64) % 2) as u8;
        }
        assert_eq!(boxes(&checker), [[0, 0, 0, 8, 8, 8]]);
    }

    #[test]
    fn models_survive_the_wire() {
        let door = model([8, 16, 2], |x, _, _| (x % 2) as u8 + 1);
        let bytes = postcard::to_allocvec(&door).unwrap();
        assert_eq!(postcard::from_bytes::<Model>(&bytes).unwrap(), door);
    }
}
