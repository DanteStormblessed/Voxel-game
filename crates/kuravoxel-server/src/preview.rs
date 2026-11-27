//! Vista previa del terreno: un mapa visto desde arriba, pintado con los colores de los
//! bloques y sombreado por el relieve. No genera chunks, solo consulta alturas y
//! superficie, así que un mapa del mundo entero sale en segundos: sirve para ver el
//! efecto de un cambio en un bioma sin abrir el juego.

use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;

use kuravoxel_common::block::Block;
use kuravoxel_common::content::{self, ContentData};
use kuravoxel_common::icon::IconImage;
use kuravoxel_common::shape::BlockTable;

use crate::biomes::{Terrain, WorldSource, SEA_LEVEL};
use crate::graph::{At, Program, MAX_OPS};
use crate::worldgen::WorldGen;

/// Índice de la cara de arriba en `BlockProps::faces`.
const TOP_FACE: usize = 2;

/// Qué parte del mundo se dibuja.
#[derive(Clone, Copy, Debug)]
pub struct MapView {
    /// Punto del mundo en el centro de la imagen (x, z).
    pub center: [i32; 2],
    /// Metros por píxel.
    pub scale: i32,
    /// Tamaño de la imagen en píxeles.
    pub size: [u32; 2],
    /// Tiñe cada zona con el color de su bioma.
    pub tint_biomes: bool,
}

impl MapView {
    /// El mundo entero, con un margen alrededor.
    pub fn whole(terrain: &Terrain, scale: i32) -> Self {
        let scale = scale.max(1);
        let margin = 256;
        Self {
            center: [0, 0],
            scale,
            size: [((terrain.width + margin * 2) / scale) as u32, ((terrain.length + margin * 2) / scale) as u32],
            tint_biomes: false,
        }
    }
}

/// Carga el contenido y el terreno de `content_dir` y prepara un generador, sin servidor.
pub fn open(content_dir: &Path, seed: u32, solo: Option<&str>) -> anyhow::Result<(WorldGen, BlockTable)> {
    let data = content::load(content_dir).and_then(|loaded| ContentData::assign(loaded, &[], &[]))?;
    let source = WorldSource::load(content_dir)?;
    let terrain = Terrain::compile(&source, &data, seed, solo)?;
    Ok((WorldGen::new(seed, Arc::new(terrain)), BlockTable::new(&data)))
}

/// Zona cuadrada del mundo que enseña una vista previa: centro (x, z) y lado en metros.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub center: [f64; 2],
    pub span: f64,
}

impl Window {
    /// Punto del mundo en el centro del píxel (i, j) de una imagen de `size` de lado.
    pub fn point(&self, i: u32, j: u32, size: u32) -> (f64, f64) {
        let step = self.span / size.max(1) as f64;
        (
            self.center[0] - self.span / 2.0 + (i as f64 + 0.5) * step,
            self.center[1] - self.span / 2.0 + (j as f64 + 0.5) * step,
        )
    }
}

/// Valores de un nodo sobre una ventana, fila a fila, con su mínimo y su máximo.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub size: u32,
    pub values: Vec<f32>,
    pub min: f32,
    pub max: f32,
}

/// Valores de unos nodos sobre una ventana: los comunes de `world.toml` y, con `biome`,
/// los de ese bioma (que tapan a los comunes del mismo nombre). Un nodo que no compila
/// da su error y no impide ver los demás.
pub fn node_grids(
    source: &WorldSource,
    biome: Option<&str>,
    names: &[String],
    seed: u32,
    window: Window,
    size: u32,
) -> Vec<Result<Grid, String>> {
    let mut nodes = source.world.nodes.clone();
    if let Some(file) = biome.and_then(|id| source.biomes.get(id)) {
        nodes.extend(file.nodes.iter().map(|(name, node)| (name.clone(), node.clone())));
    }
    let (width, length) = (source.world.width as f64, source.world.length as f64);
    let size = size.max(1);
    let evaluate = |program: &Program, regs: &[u16]| -> Vec<Grid> {
        let rows: Vec<Vec<Vec<f32>>> = (0..size)
            .into_par_iter()
            .map(|j| {
                let mut values = [0.0; MAX_OPS];
                let mut row = vec![Vec::with_capacity(size as usize); regs.len()];
                for i in 0..size {
                    let (x, z) = window.point(i, j, size);
                    let at = At { x, z, u: (x / width + 0.5) as f32, v: (z / length + 0.5) as f32 };
                    program.eval(at, &mut values);
                    for (out, &reg) in row.iter_mut().zip(regs) {
                        out.push(values[reg as usize]);
                    }
                }
                row
            })
            .collect();
        (0..regs.len())
            .map(|k| {
                let values: Vec<f32> = rows.iter().flat_map(|row| row[k].iter().copied()).collect();
                let finite = values.iter().copied().filter(|v| v.is_finite());
                let (min, max) = finite.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
                Grid { size, values, min, max }
            })
            .collect()
    };
    let roots: Vec<&str> = names.iter().map(String::as_str).collect();
    if let Ok((program, regs)) = Program::compile(&nodes, &roots, seed) {
        return evaluate(&program, &regs).into_iter().map(Ok).collect();
    }
    // Algún nodo no compila: uno a uno, para que los demás se vean igualmente.
    roots
        .iter()
        .map(|root| match Program::compile(&nodes, &[root], seed) {
            Ok((program, regs)) => Ok(evaluate(&program, &regs).remove(0)),
            Err(error) => Err(format!("{error:#}")),
        })
        .collect()
}

/// Mapa de una ventana del mundo: el bioma `biome` solo, o el mundo entero (con cada
/// bioma teñido de su color) si es `None`.
pub fn window_map(
    source: &WorldSource,
    content: &ContentData,
    seed: u32,
    biome: Option<&str>,
    window: Window,
    size: u32,
) -> anyhow::Result<IconImage> {
    let terrain = Terrain::compile(source, content, seed, biome)?;
    let generator = WorldGen::new(seed, Arc::new(terrain));
    let blocks = BlockTable::new(content);
    let scale = (window.span / size.max(1) as f64).round().max(1.0) as i32;
    let view = MapView {
        center: [window.center[0].round() as i32, window.center[1].round() as i32],
        scale,
        size: [size, size],
        tint_biomes: biome.is_none(),
    };
    Ok(render(&generator, &blocks, &view))
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Dibuja el mapa. El norte queda arriba y el oeste a la izquierda.
pub fn render(generator: &WorldGen, blocks: &BlockTable, view: &MapView) -> IconImage {
    let (width, height) = (view.size[0].max(1) as usize, view.size[1].max(1) as usize);
    let scale = view.scale.max(1);
    let terrain = generator.terrain();
    let color = |block: Block| blocks.get(block).faces[TOP_FACE].map(|c| c as f32);
    let (x0, z0) = (
        view.center[0] - width as i32 * scale / 2,
        view.center[1] - height as i32 * scale / 2,
    );

    // Alturas con una fila y una columna de más, para el sombreado y la pendiente.
    let stride = width + 1;
    let mut heights = vec![0i32; stride * (height + 1)];
    heights.par_chunks_mut(stride).enumerate().for_each(|(row, line)| {
        for (column, slot) in line.iter_mut().enumerate() {
            *slot = generator.height_at(x0 + column as i32 * scale, z0 + row as i32 * scale);
        }
    });

    let mut image = IconImage::new(width as u32, height as u32);
    image.rgba.par_chunks_mut(width * 4).enumerate().for_each(|(row, line)| {
        for column in 0..width {
            let (wx, wz) = (x0 + column as i32 * scale, z0 + row as i32 * scale);
            let h = heights[column + row * stride];
            let (east, south) = (heights[column + 1 + row * stride], heights[column + (row + 1) * stride]);
            let mut rgb = if h < SEA_LEVEL {
                // El mar se oscurece con la profundidad.
                let depth = ((SEA_LEVEL - h) as f32 / 40.0).min(1.0);
                mix(color(terrain.water), [10.0, 40.0, 110.0], 0.25 + 0.6 * depth)
            } else {
                let steep = (((east - h).pow(2) + (south - h).pow(2)) as f32).sqrt() / scale as f32;
                let mut rgb = color(terrain.surface(wx, wz, h, steep).top);
                if let Some((density, tree)) = terrain.forest(wx, wz) {
                    if tree.grows_on(terrain.surface(wx, wz, h, steep).top, h) {
                        rgb = mix(rgb, color(tree.leaves[0]), (density * 1.4).min(0.85));
                    }
                }
                // Luz desde el noroeste: las laderas que miran al sureste quedan en sombra.
                let relief = ((h - east) + (h - south)) as f32 / scale as f32;
                let shade = (1.0 + relief * 0.35).clamp(0.55, 1.35);
                // Y un poco más claro cuanto más alto, para leer la altitud.
                let lift = (h as f32 / 1500.0).clamp(0.0, 0.14);
                mix(rgb.map(|c| c * shade), [255.0; 3], lift)
            };
            if view.tint_biomes {
                rgb = mix(rgb, terrain.biome_at(wx, wz).color.map(|c| c as f32), 0.45);
            }
            let pixel = &mut line[column * 4..column * 4 + 4];
            for (slot, value) in pixel.iter_mut().zip(rgb) {
                *slot = value.clamp(0.0, 255.0) as u8;
            }
            pixel[3] = 255;
        }
    });
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_previews_show_each_node_even_with_a_broken_one() {
        use crate::biomes::tests::{TEST_DUNES, TEST_PLAINS, TEST_WORLD};
        let broken = TEST_PLAINS.replace("inputs = [20, \"hills\"]", "inputs = [20, \"hils\"]");
        let source = WorldSource::parse(TEST_WORLD, &[("base:plains", &broken), ("base:dunes", TEST_DUNES)]).unwrap();
        let window = Window { center: [0.0, -1500.0], span: 1000.0 };
        let names: Vec<String> = ["hills", "height", "land"].map(String::from).into();
        let grids = node_grids(&source, Some("base:plains"), &names, 7, window, 16);
        let hills = grids[0].as_ref().expect("hills works");
        assert_eq!(hills.values.len(), 256);
        assert!(hills.min < 0.0 && hills.max > 0.0 && hills.max <= 1.0);
        assert!(grids[1].as_ref().unwrap_err().contains("hils"));
        // `land` es un nodo común: crece de oeste a este.
        let land = grids[2].as_ref().unwrap();
        assert!(land.values[15] >= land.values[0]);

        let content = content::test_content(&crate::biomes::tests::TEST_BLOCKS);
        let source = WorldSource::parse(TEST_WORLD, &[("base:plains", TEST_PLAINS), ("base:dunes", TEST_DUNES)]).unwrap();
        let map = window_map(&source, &content, 7, Some("base:dunes"), window, 32).unwrap();
        assert_eq!((map.width, map.height), (32, 32));
        assert!(window_map(&source, &content, 7, Some("nope"), window, 32).is_err());
    }

    #[test]
    fn the_map_shows_sea_and_land() {
        let generator = WorldGen::new(1337, Arc::new(crate::biomes::tests::terrain()));
        let blocks = BlockTable::new(&content::test_content(&crate::biomes::tests::TEST_BLOCKS));
        let mut view = MapView::whole(generator.terrain(), 16);
        let image = render(&generator, &blocks, &view);
        assert_eq!((image.width, image.height), (160, 288));
        let pixel = |x: u32, y: u32| {
            let i = ((x + y * image.width) * 4) as usize;
            [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]]
        };
        // Las dunas (sur) están bajo el mar al oeste y en tierra al este.
        let (sea, land) = (pixel(20, 240), pixel(140, 240));
        assert!(sea[2] > sea[0], "sea is blue: {sea:?}");
        assert_ne!(sea, land);
        view.tint_biomes = true;
        assert_ne!(render(&generator, &blocks, &view).rgba, image.rgba);
    }
}
