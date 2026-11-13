use serde::{Deserialize, Serialize};

/// Identificador numérico de bloque, de 16 bits: 0 es aire y el resto indexa el contenido
/// que el servidor ha cargado (ver `content`). En memoria cada chunk lo reduce a 1 byte
/// por voxel con su propia paleta (ver `chunk`). Los números los asigna el
/// servidor al arrancar: el código nunca debe dar por hecho qué bloque es cada uno.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[derive(bytemuck::Pod, bytemuck::Zeroable)]
#[repr(transparent)]
pub struct Block(pub u16);

impl Block {
    pub const AIR: Block = Block(0);
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Convierte un color sRGB a RGBA lineal, que es el espacio en el que ilumina el shader.
pub fn rgb_to_linear(rgb: [u8; 3]) -> [f32; 4] {
    let channel = |value: u8| srgb_to_linear(value as f32 / 255.0);
    [channel(rgb[0]), channel(rgb[1]), channel(rgb[2]), 1.0]
}

/// Convierte 0xRRGGBB (sRGB) a RGBA lineal.
pub fn hex_to_linear(rgb: u32) -> [f32; 4] {
    rgb_to_linear([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8])
}
