use serde::{Deserialize, Serialize};

use crate::content::ContentData;

/// Identificador numérico de bloque. Un byte por voxel: 0 es aire y el resto indexa el
/// contenido que el servidor ha cargado (ver `content`). Los números los asigna el
/// servidor al arrancar: el código nunca debe dar por hecho qué bloque es cada uno.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Block(pub u8);

impl Block {
    pub const AIR: Block = Block(0);

    #[inline(always)]
    pub const fn is_solid(self) -> bool {
        self.0 != 0
    }
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

/// Paleta que se sube a la GPU. Los ids sin definir salen magenta para detectarlos.
pub fn palette(content: &ContentData) -> [[f32; 4]; 256] {
    let mut palette = [hex_to_linear(0xFF00FF); 256];
    palette[0] = [0.0; 4];
    for (id, block) in content.blocks.iter().enumerate().take(256).skip(1) {
        if let Some(block) = block {
            palette[id] = rgb_to_linear(block.color);
        }
    }
    palette
}
