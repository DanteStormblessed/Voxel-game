//! Forma 3D de los ítems e iconos generados a partir de ella.
//!
//! Un ítem se ve como su modelo propio (la espada), el modelo de su bloque (la puerta
//! entera) o el cubo de su bloque con los colores de sus caras. Esa forma (`ItemShape`)
//! sirve para dos cosas: dibujar el icono del inventario (aquí, en CPU) y dibujar el
//! ítem flotando cuando está suelto en el mundo (`render::items`).
//!
//! El icono se genera como en Hytale: una vista ortográfica del objeto con los ajustes
//! de `[icon]` del archivo (giro, escala y desplazamiento). Si el archivo trae una imagen
//! dibujada a mano, se usa esa.

use kuravoxel_common::content::{ContentData, ItemDef};
use kuravoxel_common::icon::{IconImage, IconSpec};
use kuravoxel_common::model::Model;
use kuravoxel_common::shape::{BlockTable, FACE_DIRS};

use crate::mesher::{MeshTable, PALETTE_STRIDE};

/// Lado de los iconos generados, en píxeles.
pub const ICON_SIZE: u32 = 64;
/// Muestras por píxel y eje al generar un icono (bordes suaves).
const SUPERSAMPLE: u32 = 3;
/// Opacidad mínima de los bloques translúcidos en su icono.
const MIN_ICON_OPACITY: u8 = 150;
/// Margen alrededor del objeto con `scale = 1`, en fracción del icono.
const MARGIN: f32 = 0.08;

/// Una cara visible de un voxel de la forma.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeFace {
    /// Voxel, en unidades de `ItemShape::voxel`.
    pub voxel: [u8; 3],
    /// Índice en `FACE_DIRS` (+X, −X, +Y, −Y, +Z, −Z).
    pub face: u8,
    /// Color sRGB y opacidad.
    pub color: [u8; 4],
    /// Entrada de la paleta de GPU con ese color.
    pub palette: u32,
}

/// Caras visibles del objeto de un ítem.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemShape {
    /// Tamaño en voxeles de la forma.
    pub size: [u32; 3],
    /// Lado de cada voxel de la forma en mini-voxeles (8 para un cubo de bloque, 1 para
    /// un modelo).
    pub voxel: f32,
    /// Es un bloque (su cubo o su modelo), no un objeto con modelo propio: suelto se ve
    /// más pequeño, como un bloque en miniatura.
    pub block: bool,
    pub faces: Vec<ShapeFace>,
}

impl ItemShape {
    /// Tamaño del objeto en mini-voxeles.
    pub fn extent(&self) -> [f32; 3] {
        self.size.map(|s| s as f32 * self.voxel)
    }
}

/// Forma de un ítem: su modelo, el de su bloque o el cubo de su bloque. `None` si no
/// tiene ninguno (una herramienta sin modelo): entonces se dibuja su color.
pub fn item_shape(content: &ContentData, blocks: &BlockTable, mesh: &MeshTable, item: &ItemDef) -> Option<ItemShape> {
    let own = item.model.as_ref().map(|id| (id, false));
    let of_block = || item.block.and_then(|b| content.block(b)?.model.as_ref()).map(|id| (id, true));
    if let Some((id, block)) = own.or_else(of_block) {
        let index = content.models.iter().position(|m| m.id == *id)?;
        let shape = model_shape(&content.models[index].model, mesh.model_palette_base(index));
        return Some(ItemShape { block, ..shape });
    }
    let block = item.block?;
    let props = blocks.get(block);
    let faces = (0..6)
        .map(|face| {
            let [r, g, b] = props.faces[face];
            ShapeFace {
                voxel: [0; 3],
                face: face as u8,
                // Lo translúcido se ve algo más opaco en el icono, o el cristal no se vería.
                color: [r, g, b, props.opacity.max(MIN_ICON_OPACITY)],
                palette: (block.0 as usize * PALETTE_STRIDE + face) as u32,
            }
        })
        .collect();
    Some(ItemShape { size: [1; 3], voxel: 8.0, block: true, faces })
}

fn model_shape(model: &Model, palette_base: u32) -> ItemShape {
    let [sx, sy, sz] = model.size;
    let filled = |x: i32, y: i32, z: i32| {
        (0..sx as i32).contains(&x)
            && (0..sy as i32).contains(&y)
            && (0..sz as i32).contains(&z)
            && model.get(x as usize, y as usize, z as usize) != 0
    };
    let mut faces = Vec::new();
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                let color = model.get(x, y, z);
                if color == 0 {
                    continue;
                }
                for (face, [dx, dy, dz]) in FACE_DIRS.iter().enumerate() {
                    if filled(x as i32 + dx, y as i32 + dy, z as i32 + dz) {
                        continue;
                    }
                    let [r, g, b] = model.palette[color as usize - 1];
                    faces.push(ShapeFace {
                        voxel: [x as u8, y as u8, z as u8],
                        face: face as u8,
                        color: [r, g, b, 255],
                        palette: palette_base + color as u32 - 1,
                    });
                }
            }
        }
    }
    ItemShape { size: [sx as u32, sy as u32, sz as u32], voxel: 1.0, block: false, faces }
}

/// Las 4 esquinas de una cara de un voxel de lado 1 en (0,0,0), en orden de recorrido.
fn face_corners(face: u8) -> [[f32; 3]; 4] {
    match face {
        0 => [[1., 0., 0.], [1., 1., 0.], [1., 1., 1.], [1., 0., 1.]],
        1 => [[0., 0., 0.], [0., 0., 1.], [0., 1., 1.], [0., 1., 0.]],
        2 => [[0., 1., 0.], [0., 1., 1.], [1., 1., 1.], [1., 1., 0.]],
        3 => [[0., 0., 0.], [1., 0., 0.], [1., 0., 1.], [0., 0., 1.]],
        4 => [[0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.]],
        _ => [[0., 0., 0.], [0., 1., 0.], [1., 1., 0.], [1., 0., 0.]],
    }
}

/// Gira un punto como pide `[icon] rotation`: primero el giro (eje vertical), luego la
/// inclinación hacia la cámara y por último el alabeo en el plano del icono.
fn view_rotate(p: [f32; 3], [pitch, yaw, roll]: [f32; 3]) -> [f32; 3] {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    let (sr, cr) = roll.to_radians().sin_cos();
    let [x, y, z] = p;
    let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
    let (y, z) = (y * cp + z * sp, -y * sp + z * cp);
    [x * cr - y * sr, x * sr + y * cr, z]
}

/// Dibuja el icono de una forma con los ajustes de `spec`. La cámara mira el frente del
/// objeto (−Z) desde delante; con la rotación por defecto se ve en tres cuartos desde
/// arriba. Pantalla: x a la derecha, y hacia abajo; profundidad mayor = más lejos.
pub fn render_icon(shape: &ItemShape, spec: &IconSpec, size: u32) -> IconImage {
    let s = size * SUPERSAMPLE;
    let extent = shape.extent();
    let center = extent.map(|e| e * 0.5);

    // Cuadriláteros ya girados, con su color sombreado.
    let light = normalize([-0.35, 1.0, -0.6]);
    let mut quads: Vec<([[f32; 3]; 4], [f32; 4])> = Vec::with_capacity(shape.faces.len());
    for face in &shape.faces {
        let normal = view_rotate(FACE_DIRS[face.face as usize].map(|v| v as f32), spec.rotation);
        // La cámara está en −Z: solo se ven las caras que miran hacia ella.
        if normal[2] >= 0.0 {
            continue;
        }
        let corners = face_corners(face.face).map(|c| {
            let p = [0, 1, 2].map(|a| (face.voxel[a] as f32 + c[a]) * shape.voxel - center[a]);
            let [x, y, z] = view_rotate(p, spec.rotation);
            // La pantalla tiene la x al revés que la vista (se mira hacia +Z).
            [-x, -y, z]
        });
        let shade = 0.5 + 0.5 * dot([-normal[0], normal[1], normal[2]], light).max(0.0);
        let [r, g, b, a] = face.color.map(|c| c as f32 / 255.0);
        quads.push((corners, [r * shade, g * shade, b * shade, a]));
    }

    // Encaje: el objeto entero (su caja) cabe en el icono con un margen, por `scale`.
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    for x in [0.0, extent[0]] {
        for y in [0.0, extent[1]] {
            for z in [0.0, extent[2]] {
                let [vx, vy, _] = view_rotate([x - center[0], y - center[1], z - center[2]], spec.rotation);
                for (axis, v) in [-vx, -vy].into_iter().enumerate() {
                    lo[axis] = lo[axis].min(v);
                    hi[axis] = hi[axis].max(v);
                }
            }
        }
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-3);
    let scale = s as f32 * (1.0 - 2.0 * MARGIN) / span * spec.scale;
    let mid = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let to_screen = |p: [f32; 3]| {
        [
            (p[0] - mid[0]) * scale + s as f32 * (0.5 + spec.offset[0]),
            (p[1] - mid[1]) * scale + s as f32 * (0.5 - spec.offset[1]),
            p[2],
        ]
    };

    // Primero lo opaco con z-buffer; después lo translúcido de lejos a cerca, mezclado.
    let mut color = vec![[0f32; 4]; (s * s) as usize];
    let mut depth = vec![f32::MAX; (s * s) as usize];
    quads.sort_by(|a, b| {
        let far = |q: &([[f32; 3]; 4], [f32; 4])| q.0.iter().map(|c| c[2]).sum::<f32>();
        (a.1[3] < 1.0).cmp(&(b.1[3] < 1.0)).then(far(b).total_cmp(&far(a)))
    });
    for (corners, rgba) in &quads {
        let p = corners.map(to_screen);
        for tri in [[p[0], p[1], p[2]], [p[0], p[2], p[3]]] {
            fill_triangle(&tri, s, |i, z| {
                if z < depth[i] {
                    if rgba[3] >= 1.0 {
                        depth[i] = z;
                        color[i] = *rgba;
                    } else {
                        let a = rgba[3];
                        let under = color[i];
                        color[i] = [0, 1, 2]
                            .map(|c| rgba[c] * a + under[c] * (1.0 - a))
                            .into_iter()
                            .chain([a + under[3] * (1.0 - a)])
                            .collect::<Vec<_>>()
                            .try_into()
                            .unwrap();
                    }
                }
            });
        }
    }

    // Reducción: media de las muestras de cada píxel (color ponderado por alfa).
    let mut image = IconImage::new(size, size);
    let n = (SUPERSAMPLE * SUPERSAMPLE) as f32;
    for py in 0..size {
        for px in 0..size {
            let mut sum = [0f32; 4];
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let c = color[((py * SUPERSAMPLE + sy) * s + px * SUPERSAMPLE + sx) as usize];
                    for k in 0..3 {
                        sum[k] += c[k] * c[3];
                    }
                    sum[3] += c[3];
                }
            }
            let out = &mut image.rgba[((py * size + px) * 4) as usize..][..4];
            if sum[3] > 0.0 {
                for k in 0..3 {
                    out[k] = (sum[k] / sum[3] * 255.0).round().clamp(0.0, 255.0) as u8;
                }
                out[3] = (sum[3] / n * 255.0).round() as u8;
            }
        }
    }
    image
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    v.map(|c| c / len)
}

/// Rellena un triángulo en una imagen de `size`×`size`, con la profundidad interpolada.
fn fill_triangle(p: &[[f32; 3]; 3], size: u32, mut plot: impl FnMut(usize, f32)) {
    let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
    let area = edge(p[0], p[1], p[2][0], p[2][1]);
    if area.abs() < 1e-6 {
        return;
    }
    let min_x = p.iter().map(|v| v[0]).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
    let max_x = p.iter().map(|v| v[0]).fold(f32::MIN, f32::max).ceil().min(size as f32) as u32;
    let min_y = p.iter().map(|v| v[1]).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
    let max_y = p.iter().map(|v| v[1]).fold(f32::MIN, f32::max).ceil().min(size as f32) as u32;
    for y in min_y..max_y {
        for x in min_x..max_x {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(p[1], p[2], cx, cy) / area;
            let w1 = edge(p[2], p[0], cx, cy) / area;
            let w2 = 1.0 - w0 - w1;
            // Regla de bordes sencilla: se aceptan los puntos sobre el borde, así dos
            // triángulos vecinos no dejan grietas (repetir un píxel no se nota).
            if w0 >= -1e-6 && w1 >= -1e-6 && w2 >= -1e-6 {
                plot((y * size + x) as usize, w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuravoxel_common::content::{test_content, BlockDef, Loaded, ModelDef};

    fn tables(content: &ContentData) -> (BlockTable, MeshTable) {
        let blocks = BlockTable::new(content);
        let (mesh, _) = crate::mesher::build_tables(content, &blocks);
        (blocks, mesh)
    }

    fn pixel(image: &IconImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * image.width + x) * 4) as usize;
        image.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn a_block_icon_is_a_cube_lit_from_above() {
        let content = test_content(&["stone"]);
        let (blocks, mesh) = tables(&content);
        let item = content.item(content.item_id("base:stone").unwrap()).unwrap();
        let shape = item_shape(&content, &blocks, &mesh, item).unwrap();
        assert_eq!((shape.size, shape.faces.len()), ([1, 1, 1], 6));

        let icon = render_icon(&shape, &item.icon, ICON_SIZE);
        // Las esquinas quedan vacías, el centro lleno; la cara de arriba (en la parte
        // alta del icono) es la más clara.
        assert_eq!(pixel(&icon, 0, 0)[3], 0);
        assert_eq!(pixel(&icon, 32, 32)[3], 255);
        let top = pixel(&icon, 32, 14);
        let side = pixel(&icon, 20, 44);
        assert!(top[0] as u32 + top[1] as u32 > side[0] as u32 + side[1] as u32, "{top:?} vs {side:?}");
    }

    #[test]
    fn model_items_use_their_model_and_scale_moves_the_border() {
        let sword = Model { size: [1, 16, 1], voxels: vec![1; 16], palette: vec![[200, 200, 220]] };
        let content = ContentData::assign(
            Loaded {
                blocks: vec![BlockDef::simple("a:stone", "s", [1; 3])],
                items: vec![kuravoxel_common::content::ItemSource {
                    id: "a:sword".into(),
                    name: Some("Sword".into()),
                    color: Some([1; 3]),
                    category: None,
                    tags: None,
                    block: None,
                    tool: None,
                    model: Some("a:sword".into()),
                    max_stack: None,
                    icon: None,
                }],
                models: vec![ModelDef { id: "a:sword".into(), model: sword }],
            },
            &[],
            &[],
        )
        .unwrap();
        let (blocks, mesh) = tables(&content);
        let item = content.item(content.item_id("a:sword").unwrap()).unwrap();
        let shape = item_shape(&content, &blocks, &mesh, item).unwrap();
        assert_eq!(shape.size, [1, 16, 1]);
        // 16 voxeles en columna: 4 caras laterales cada uno y las dos puntas.
        assert_eq!(shape.faces.len(), 16 * 4 + 2);

        // De frente y sin girar, la espada es una línea vertical centrada.
        let flat = IconSpec { rotation: [0.0, 0.0, 0.0], ..IconSpec::default() };
        let icon = render_icon(&shape, &flat, ICON_SIZE);
        assert_eq!(pixel(&icon, 32, 32)[3], 255);
        assert_eq!(pixel(&icon, 20, 32)[3], 0);
        assert!(pixel(&icon, 32, 6)[3] > 0, "the sword reaches the top margin");
        let small = render_icon(&shape, &IconSpec { scale: 0.5, ..flat }, ICON_SIZE);
        assert_eq!(pixel(&small, 32, 6)[3], 0, "half scale leaves room above");
    }
}
