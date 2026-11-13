//! Iconos de los ítems.
//!
//! Como en Hytale, el icono de un ítem se genera a partir de su modelo (o del cubo de su
//! bloque) con unos ajustes de cámara que viven en su archivo, o es una imagen PNG
//! dibujada a mano que lo sustituye:
//!
//! ```toml
//! [icon]
//! rotation = [30, 45, 0]   # grados: inclinación, giro y alabeo
//! scale = 1.0              # 1 = el objeto llena el icono
//! offset = [0, 0]          # desplazamiento, en fracción del icono
//! image = "base:sword"     # opcional: content/base/icons/sword.png
//! ```
//!
//! Aquí solo están los datos y el PNG; el dibujo de los iconos generados lo hace el
//! cliente (`kuravoxel_client::icons`).

use anyhow::{bail, ensure, Context};
use serde::{Deserialize, Serialize};

/// Lado máximo de una imagen de icono, en píxeles.
pub const MAX_ICON_SIZE: u32 = 256;

/// Cómo se ve el icono de un ítem.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IconSpec {
    /// Imagen dibujada a mano: si está, se usa tal cual.
    pub image: Option<IconImage>,
    /// Grados: inclinación (hacia la cámara), giro (alrededor del eje vertical) y alabeo
    /// (en el plano del icono).
    pub rotation: [f32; 3],
    /// 1 = el objeto llena el icono (con un pequeño margen).
    pub scale: f32,
    /// Desplazamiento en fracción del icono: [0.1, 0] lo mueve un 10 % a la derecha.
    pub offset: [f32; 2],
}

impl Default for IconSpec {
    fn default() -> Self {
        Self { image: None, rotation: DEFAULT_ROTATION, scale: 1.0, offset: [0.0, 0.0] }
    }
}

/// Vista por defecto: tres cuartos desde arriba, como los bloques de Minecraft o Hytale.
pub const DEFAULT_ROTATION: [f32; 3] = [30.0, 45.0, 0.0];

/// Imagen RGBA8 (sRGB, alfa sin premultiplicar).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for IconImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IconImage({}×{})", self.width, self.height)
    }
}

impl IconImage {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, rgba: vec![0; (width * height * 4) as usize] }
    }
}

/// Lee un PNG (de cualquier tipo de color) como RGBA8.
pub fn decode_png(bytes: &[u8]) -> anyhow::Result<IconImage> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().context("not a valid PNG image")?;
    let (width, height) = (reader.info().width, reader.info().height);
    ensure!(
        width > 0 && height > 0 && width <= MAX_ICON_SIZE && height <= MAX_ICON_SIZE,
        "icon is {width}×{height}; the maximum is {MAX_ICON_SIZE}×{MAX_ICON_SIZE}"
    );
    let mut buffer = vec![0; reader.output_buffer_size().context("image too large")?];
    let frame = reader.next_frame(&mut buffer).context("failed to decode the PNG image")?;
    let data = &buffer[..frame.buffer_size()];
    let pixels = (width * height) as usize;
    let rgba: Vec<u8> = match frame.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => data.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        other => bail!("unsupported PNG color type {other:?}"),
    };
    ensure!(rgba.len() == pixels * 4, "PNG data has the wrong size");
    Ok(IconImage { width, height, rgba })
}

/// Escribe una imagen como PNG RGBA.
pub fn encode_png(image: &IconImage) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgba)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_icons_round_trip() {
        let mut image = IconImage::new(3, 2);
        image.rgba[0..4].copy_from_slice(&[255, 10, 20, 255]);
        image.rgba[20..24].copy_from_slice(&[0, 0, 255, 128]);
        let decoded = decode_png(&encode_png(&image).unwrap()).unwrap();
        assert_eq!(decoded, image);
    }

    #[test]
    fn bad_or_huge_images_are_rejected() {
        assert!(decode_png(b"not a png").is_err());
        let huge = IconImage::new(MAX_ICON_SIZE + 1, 1);
        assert!(decode_png(&encode_png(&huge).unwrap()).is_err());
    }
}
