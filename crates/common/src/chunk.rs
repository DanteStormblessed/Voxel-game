use serde::{Deserialize, Serialize};

use super::block::Block;

pub const CHUNK_BITS: usize = 5;
pub const CHUNK_SIZE: usize = 1 << CHUNK_BITS;
pub const CHUNK_SIZE_I: i32 = CHUNK_SIZE as i32;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// Chunk cúbico de 32³ voxeles.
///
/// La mayoría de chunks del mundo son cielo o roca maciza: esos no reservan memoria
/// (`Uniform`). Solo los que mezclan bloques guardan el array denso de 32 KiB.
pub enum Chunk {
    Uniform(Block),
    Dense(Box<[u8; CHUNK_VOLUME]>),
}

impl Chunk {
    /// X es el eje rápido, luego Y, luego Z (mismo orden que el buffer acolchado del mallador).
    #[inline(always)]
    pub const fn index(x: usize, y: usize, z: usize) -> usize {
        x | (y << CHUNK_BITS) | (z << (2 * CHUNK_BITS))
    }

    pub fn new_dense_buffer() -> Box<[u8; CHUNK_VOLUME]> {
        vec![0u8; CHUNK_VOLUME]
            .into_boxed_slice()
            .try_into()
            .expect("chunk size")
    }

    /// Envuelve un buffer denso, colapsándolo a `Uniform` si todos los voxeles son iguales.
    pub fn from_dense(data: Box<[u8; CHUNK_VOLUME]>) -> Self {
        let first = data[0];
        if data.iter().all(|&b| b == first) {
            Chunk::Uniform(Block(first))
        } else {
            Chunk::Dense(data)
        }
    }

    #[inline]
    pub fn uniform(&self) -> Option<Block> {
        match self {
            Chunk::Uniform(block) => Some(*block),
            Chunk::Dense(_) => None,
        }
    }

    /// Copia del chunk con un voxel cambiado. Los chunks se comparten entre hilos (y, en
    /// el servidor integrado, entre servidor y cliente), así que nunca se modifican in situ.
    pub fn with_block(&self, x: usize, y: usize, z: usize, block: Block) -> Chunk {
        let mut data = match self {
            Chunk::Uniform(fill) => {
                let mut data = Self::new_dense_buffer();
                data.fill(fill.0);
                data
            }
            Chunk::Dense(data) => data.clone(),
        };
        data[Self::index(x, y, z)] = block.0;
        // Si vuelve a quedar todo igual (p. ej. se rompe el único bloque), deja de ocupar memoria.
        Self::from_dense(data)
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> Block {
        match self {
            Chunk::Uniform(block) => *block,
            Chunk::Dense(data) => Block(data[Self::index(x, y, z)]),
        }
    }
}

/// Codifica una secuencia de bytes como tramos (longitud, valor). Los datos de voxeles
/// son muy repetitivos: un chunk típico pasa de 32 KiB a unos cientos de bytes.
pub fn rle_encode(data: &[u8]) -> Vec<(u16, u8)> {
    let mut runs: Vec<(u16, u8)> = Vec::new();
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
pub fn rle_decode(runs: &[(u16, u8)], out: &mut [u8]) -> bool {
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

/// Forma en la que un chunk viaja por la red.
#[derive(Serialize, Deserialize)]
enum ChunkWire {
    Uniform(u8),
    Rle(Vec<(u16, u8)>),
}

impl Serialize for Chunk {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Chunk::Uniform(block) => ChunkWire::Uniform(block.0),
            Chunk::Dense(data) => ChunkWire::Rle(rle_encode(&data[..])),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Chunk {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match ChunkWire::deserialize(deserializer)? {
            ChunkWire::Uniform(block) => Chunk::Uniform(Block(block)),
            ChunkWire::Rle(runs) => {
                let mut data = Chunk::new_dense_buffer();
                if !rle_decode(&runs, &mut data[..]) {
                    return Err(serde::de::Error::custom("chunk has the wrong size"));
                }
                Chunk::Dense(data)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_survives_the_wire() {
        let mut data = Chunk::new_dense_buffer();
        for (i, voxel) in data.iter_mut().enumerate() {
            *voxel = ((i / 700) % 5) as u8;
        }
        let bytes = postcard::to_allocvec(&Chunk::Dense(data.clone())).unwrap();
        assert!(bytes.len() < 1024, "{} bytes", bytes.len());
        match postcard::from_bytes::<Chunk>(&bytes).unwrap() {
            Chunk::Dense(decoded) => assert!(decoded[..] == data[..]),
            Chunk::Uniform(_) => panic!("should be dense"),
        }
    }

    #[test]
    fn malformed_chunk_is_rejected() {
        let bytes = postcard::to_allocvec(&ChunkWire::Rle(vec![(10, 1)])).unwrap();
        assert!(postcard::from_bytes::<Chunk>(&bytes).is_err());
    }
}
