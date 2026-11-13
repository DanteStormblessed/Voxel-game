use serde::{Deserialize, Serialize};

use super::block::Block;

pub const CHUNK_BITS: usize = 5;
pub const CHUNK_SIZE: usize = 1 << CHUNK_BITS;
pub const CHUNK_SIZE_I: i32 = CHUNK_SIZE as i32;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// Bloques distintos que caben en la paleta de un chunk (índices de 1 byte).
pub const CHUNK_PALETTE_MAX: usize = 256;

/// Voxeles de un chunk sin comprimir, tal como los rellena el generador.
pub type ChunkVoxels = Box<[Block; CHUNK_VOLUME]>;

/// Chunk cúbico de 32³ voxeles.
///
/// Los números de bloque son de 16 bits, pero un chunk casi nunca mezcla más de unos
/// pocos, así que cada uno guarda su propia paleta y 1 byte por voxel:
///
/// * `Uniform`: un solo bloque (cielo, roca maciza). No reserva memoria.
/// * `Paletted`: hasta 256 bloques distintos. 32 KiB más la paleta.
/// * `Wide`: más de 256 bloques distintos (rarísimo). 64 KiB.
pub enum Chunk {
    Uniform(Block),
    Paletted(PalettedChunk),
    Wide(ChunkVoxels),
}

pub struct PalettedChunk {
    /// Bloque de cada índice. Entre 2 y 256 entradas; puede contener alguna que ya no se
    /// use (tras editar), y se limpia cuando hace falta sitio.
    palette: Vec<Block>,
    indices: Box<[u8; CHUNK_VOLUME]>,
}

impl PalettedChunk {
    pub fn palette(&self) -> &[Block] {
        &self.palette
    }

    pub fn indices(&self) -> &[u8; CHUNK_VOLUME] {
        &self.indices
    }
}

fn boxed_array<T: Copy, const N: usize>(value: T) -> Box<[T; N]> {
    vec![value; N].into_boxed_slice().try_into().unwrap_or_else(|_| unreachable!())
}

impl Chunk {
    /// X es el eje rápido, luego Y, luego Z (mismo orden que el buffer acolchado del mallador).
    #[inline(always)]
    pub const fn index(x: usize, y: usize, z: usize) -> usize {
        x | (y << CHUNK_BITS) | (z << (2 * CHUNK_BITS))
    }

    /// Buffer de voxeles lleno de aire, para rellenarlo y pasarlo a `from_dense`.
    pub fn new_dense_buffer() -> ChunkVoxels {
        boxed_array(Block::AIR)
    }

    /// Comprime un buffer de voxeles a la forma más pequeña que lo representa.
    pub fn from_dense(data: ChunkVoxels) -> Self {
        let mut palette: Vec<Block> = Vec::new();
        let mut indices: Box<[u8; CHUNK_VOLUME]> = boxed_array(0);
        // Los voxeles vienen en tramos largos: casi siempre basta con el último índice.
        let mut last: Option<(Block, u8)> = None;
        for (slot, &block) in indices.iter_mut().zip(data.iter()) {
            let index = match last {
                Some((previous, index)) if previous == block => index,
                _ => {
                    let index = match palette.iter().position(|&b| b == block) {
                        Some(index) => index,
                        None if palette.len() < CHUNK_PALETTE_MAX => {
                            palette.push(block);
                            palette.len() - 1
                        }
                        None => return Chunk::Wide(data),
                    } as u8;
                    last = Some((block, index));
                    index
                }
            };
            *slot = index;
        }
        if palette.len() == 1 {
            Chunk::Uniform(palette[0])
        } else {
            Chunk::Paletted(PalettedChunk { palette, indices })
        }
    }

    /// Todos los voxeles sin comprimir.
    pub fn to_dense(&self) -> ChunkVoxels {
        match self {
            Chunk::Uniform(block) => boxed_array(*block),
            Chunk::Paletted(chunk) => {
                let mut data = Self::new_dense_buffer();
                for (out, &index) in data.iter_mut().zip(chunk.indices.iter()) {
                    *out = chunk.palette[index as usize];
                }
                data
            }
            Chunk::Wide(data) => data.clone(),
        }
    }

    #[inline]
    pub fn uniform(&self) -> Option<Block> {
        match self {
            Chunk::Uniform(block) => Some(*block),
            _ => None,
        }
    }

    /// Copia del chunk con un voxel cambiado. Los chunks se comparten entre hilos (y, en
    /// el servidor integrado, entre servidor y cliente), así que nunca se modifican in situ.
    pub fn with_block(&self, x: usize, y: usize, z: usize, block: Block) -> Chunk {
        let at = Self::index(x, y, z);
        match self {
            Chunk::Uniform(fill) if *fill == block => Chunk::Uniform(block),
            Chunk::Uniform(fill) => {
                let mut indices = boxed_array(0);
                indices[at] = 1;
                Chunk::Paletted(PalettedChunk { palette: vec![*fill, block], indices })
            }
            Chunk::Paletted(chunk) => {
                let known = chunk.palette.iter().position(|&b| b == block);
                if known.is_none() && chunk.palette.len() >= CHUNK_PALETTE_MAX {
                    // Paleta llena: se reconstruye, lo que de paso quita las entradas sin uso.
                    return self.rebuilt_with(at, block);
                }
                let mut palette = chunk.palette.clone();
                let index = known.unwrap_or_else(|| {
                    palette.push(block);
                    palette.len() - 1
                }) as u8;
                let mut indices = chunk.indices.clone();
                indices[at] = index;
                // Si vuelve a quedar todo igual (p. ej. se rompe el único bloque), deja de
                // ocupar memoria.
                if indices.iter().all(|&i| i == index) {
                    Chunk::Uniform(block)
                } else {
                    Chunk::Paletted(PalettedChunk { palette, indices })
                }
            }
            Chunk::Wide(_) => self.rebuilt_with(at, block),
        }
    }

    fn rebuilt_with(&self, at: usize, block: Block) -> Chunk {
        let mut data = self.to_dense();
        data[at] = block;
        Self::from_dense(data)
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> Block {
        let at = Self::index(x, y, z);
        match self {
            Chunk::Uniform(block) => *block,
            Chunk::Paletted(chunk) => chunk.palette[chunk.indices[at] as usize],
            Chunk::Wide(data) => data[at],
        }
    }

    /// Copia en `out` los voxeles consecutivos en X que empiezan en (x, y, z).
    #[inline]
    pub fn copy_row(&self, x: usize, y: usize, z: usize, out: &mut [Block]) {
        let start = Self::index(x, y, z);
        debug_assert!(x + out.len() <= CHUNK_SIZE);
        match self {
            Chunk::Uniform(block) => out.fill(*block),
            Chunk::Paletted(chunk) => {
                let row = &chunk.indices[start..start + out.len()];
                for (out, &index) in out.iter_mut().zip(row) {
                    *out = chunk.palette[index as usize];
                }
            }
            Chunk::Wide(data) => out.copy_from_slice(&data[start..start + out.len()]),
        }
    }
}

/// Codifica una secuencia como tramos (longitud, valor). Los datos de voxeles son muy
/// repetitivos: un chunk típico pasa de 32 KiB a unos cientos de bytes.
pub fn rle_encode<T: Copy + PartialEq>(data: &[T]) -> Vec<(u16, T)> {
    let mut runs: Vec<(u16, T)> = Vec::new();
    for &value in data {
        match runs.last_mut() {
            Some((count, last)) if *last == value && *count < u16::MAX => *count += 1,
            _ => runs.push((1, value)),
        }
    }
    runs
}

/// Inversa de `rle_encode`. Devuelve `false` si los tramos no llenan `out` exactamente
/// (datos corruptos o malintencionados).
pub fn rle_decode<T: Copy>(runs: &[(u16, T)], out: &mut [T]) -> bool {
    let mut at = 0usize;
    for &(count, value) in runs {
        let end = at + count as usize;
        if end > out.len() {
            return false;
        }
        out[at..end].fill(value);
        at = end;
    }
    at == out.len()
}

/// Forma en la que un chunk viaja por la red y se guarda en disco.
#[derive(Serialize, Deserialize)]
enum ChunkWire {
    Uniform(Block),
    Paletted { palette: Vec<Block>, runs: Vec<(u16, u8)> },
    Wide(Vec<(u16, Block)>),
}

impl Serialize for Chunk {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Chunk::Uniform(block) => ChunkWire::Uniform(*block),
            Chunk::Paletted(chunk) => ChunkWire::Paletted {
                palette: chunk.palette.clone(),
                runs: rle_encode(&chunk.indices[..]),
            },
            Chunk::Wide(data) => ChunkWire::Wide(rle_encode(&data[..])),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Chunk {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        Ok(match ChunkWire::deserialize(deserializer)? {
            ChunkWire::Uniform(block) => Chunk::Uniform(block),
            ChunkWire::Paletted { palette, runs } => {
                if palette.len() < 2 || palette.len() > CHUNK_PALETTE_MAX {
                    return Err(D::Error::custom("chunk palette has the wrong size"));
                }
                let mut indices = boxed_array(0);
                if !rle_decode(&runs, &mut indices[..]) {
                    return Err(D::Error::custom("chunk has the wrong size"));
                }
                if runs.iter().any(|&(_, index)| index as usize >= palette.len()) {
                    return Err(D::Error::custom("chunk refers to a missing palette entry"));
                }
                Chunk::Paletted(PalettedChunk { palette, indices })
            }
            ChunkWire::Wide(runs) => {
                let mut data = Chunk::new_dense_buffer();
                if !rle_decode(&runs, &mut data[..]) {
                    return Err(D::Error::custom("chunk has the wrong size"));
                }
                Chunk::Wide(data)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn striped(kinds: usize) -> ChunkVoxels {
        let mut data = Chunk::new_dense_buffer();
        for (i, voxel) in data.iter_mut().enumerate() {
            *voxel = Block(((i / 100) % kinds) as u16 * 211 + 1);
        }
        data
    }

    fn same(a: &Chunk, b: &Chunk) -> bool {
        a.to_dense()[..] == b.to_dense()[..]
    }

    #[test]
    fn chunk_picks_the_smallest_form() {
        assert!(matches!(Chunk::from_dense(Chunk::new_dense_buffer()), Chunk::Uniform(Block::AIR)));
        assert!(matches!(Chunk::from_dense(striped(5)), Chunk::Paletted(_)));
        assert!(matches!(Chunk::from_dense(striped(256)), Chunk::Paletted(_)));
        assert!(matches!(Chunk::from_dense(striped(257)), Chunk::Wide(_)));
    }

    #[test]
    fn chunk_survives_the_wire() {
        let paletted = Chunk::from_dense(striped(5));
        let bytes = postcard::to_allocvec(&paletted).unwrap();
        assert!(bytes.len() < 1024, "{} bytes", bytes.len());
        assert!(same(&postcard::from_bytes::<Chunk>(&bytes).unwrap(), &paletted));

        let wide = Chunk::from_dense(striped(300));
        let bytes = postcard::to_allocvec(&wide).unwrap();
        assert!(same(&postcard::from_bytes::<Chunk>(&bytes).unwrap(), &wide));
    }

    #[test]
    fn malformed_chunk_is_rejected() {
        let short = ChunkWire::Paletted { palette: vec![Block(1), Block(2)], runs: vec![(10, 1)] };
        assert!(postcard::from_bytes::<Chunk>(&postcard::to_allocvec(&short).unwrap()).is_err());
        let runs = vec![(CHUNK_VOLUME as u16 / 2, 0), (CHUNK_VOLUME as u16 / 2, 7)];
        let missing = ChunkWire::Paletted { palette: vec![Block(1), Block(2)], runs };
        assert!(postcard::from_bytes::<Chunk>(&postcard::to_allocvec(&missing).unwrap()).is_err());
    }

    #[test]
    fn edits_keep_every_voxel() {
        let mut chunk = Chunk::Uniform(Block::AIR);
        let mut expected = Chunk::new_dense_buffer();
        // 300 bloques distintos: obliga a pasar de paleta a `Wide`.
        for i in 0..300u16 {
            let (x, y, z) = (i as usize % 32, i as usize / 32, 7);
            chunk = chunk.with_block(x, y, z, Block(i + 1));
            expected[Chunk::index(x, y, z)] = Block(i + 1);
            assert_eq!(chunk.get(x, y, z), Block(i + 1));
        }
        assert!(matches!(chunk, Chunk::Wide(_)));
        assert!(chunk.to_dense()[..] == expected[..]);

        // Al borrarlos casi todos, la paleta se limpia y vuelve a caber en 1 byte.
        for i in 0..290u16 {
            chunk = chunk.with_block(i as usize % 32, i as usize / 32, 7, Block::AIR);
        }
        assert!(matches!(chunk, Chunk::Paletted(_)));
        for i in 290..300u16 {
            chunk = chunk.with_block(i as usize % 32, i as usize / 32, 7, Block::AIR);
        }
        assert!(matches!(chunk, Chunk::Uniform(Block::AIR)));
    }

    #[test]
    fn full_palette_drops_unused_entries() {
        // 256 bloques, se borra uno y se coloca otro nuevo: debe seguir en paleta.
        let mut chunk = Chunk::from_dense(striped(256));
        let victim = chunk.get(20, 15, 0);
        assert_ne!(victim, Block(1));
        let data = chunk.to_dense();
        for (i, &block) in data.iter().enumerate() {
            if block == victim {
                let (x, y, z) = (i & 31, (i >> 5) & 31, i >> 10);
                chunk = chunk.with_block(x, y, z, Block(1));
            }
        }
        chunk = chunk.with_block(0, 0, 0, Block(60_000));
        assert!(matches!(chunk, Chunk::Paletted(_)));
        assert_eq!(chunk.get(0, 0, 0), Block(60_000));
    }

    #[test]
    fn rows_match_single_reads() {
        for chunk in [Chunk::Uniform(Block(4)), Chunk::from_dense(striped(5)), Chunk::from_dense(striped(300))] {
            let mut row = [Block::AIR; 20];
            chunk.copy_row(6, 3, 9, &mut row);
            for (i, &block) in row.iter().enumerate() {
                assert_eq!(block, chunk.get(6 + i, 3, 9));
            }
        }
    }
}
