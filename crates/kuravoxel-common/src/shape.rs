//! Propiedades de cada número de bloque, derivadas del contenido una vez y listas para
//! los bucles internos: física, rayo de edición y mallado.
//!
//! Aquí se aplica la orientación de cada estado: los colores de cada cara y la celda del
//! modelo salen ya girados, y la colisión también.

use crate::block::Block;
use crate::content::{CollisionShape, ContentData};
use crate::model::{self, CellBox, CellVoxels};

/// Caras en el orden del mallador y del shader: +X, −X, +Y, −Y, +Z, −Z.
pub const FACE_DIRS: [[i32; 3]; 6] = [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]];

/// Con qué choca un voxel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Collision {
    Empty,
    Full,
    /// Cajas en mini-voxeles, ya giradas.
    Boxes(Vec<CellBox>),
}

/// Celda de modelo que dibuja un estado de bloque, ya girada.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelCell {
    /// Índice en `ContentData::models` (para la paleta del modelo).
    pub model: u16,
    pub voxels: CellVoxels,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockProps {
    /// Algo que dibujar (todo menos el aire).
    pub visible: bool,
    /// Cubo entero y opaco: tapa las caras de sus vecinos y hace sombra (oclusión).
    pub opaque: bool,
    /// Cubo translúcido: va en el segundo pase.
    pub transparent: bool,
    /// El rayo de edición se detiene en él (no atraviesa: sólidos y modelos).
    pub targetable: bool,
    pub collision: Collision,
    /// Luz que emite (0-15).
    pub light: u8,
    /// Opacidad de los cubos translúcidos (255 en los demás).
    pub opacity: u8,
    /// Color de cada cara (+X, −X, +Y, −Y, +Z, −Z), ya orientado.
    pub faces: [[u8; 3]; 6],
    pub model: Option<ModelCell>,
}

impl BlockProps {
    const AIR: BlockProps = BlockProps {
        visible: false,
        opaque: false,
        transparent: false,
        targetable: false,
        collision: Collision::Empty,
        light: 0,
        opacity: 0,
        faces: [[0; 3]; 6],
        model: None,
    };

    /// Lo que no se conoce (contenido aún no recibido, número retirado): un cubo
    /// sólido magenta, para que no se pueda atravesar y se vea que falta algo.
    const UNKNOWN: BlockProps = BlockProps {
        visible: true,
        opaque: true,
        transparent: false,
        targetable: true,
        collision: Collision::Full,
        light: 0,
        opacity: 255,
        faces: [[255, 0, 255]; 6],
        model: None,
    };
}

/// Propiedades por número de bloque.
#[derive(Clone, Debug, Default)]
pub struct BlockTable {
    props: Vec<BlockProps>,
}

fn face_index(dir: [i32; 3]) -> usize {
    FACE_DIRS.iter().position(|&d| d == dir).expect("axis direction")
}

impl BlockTable {
    pub fn new(content: &ContentData) -> Self {
        let mut props = vec![BlockProps::UNKNOWN; content.states.len().max(1)];
        props[0] = BlockProps::AIR;
        for (number, state) in content.numbered_states() {
            let def = &content.blocks[state.block as usize];
            let rotation = def.orientation.rotation(state.state);

            // Colores en el espacio del bloque: arriba, abajo, frente (−Z) y lados.
            let mut local = [def.faces.side; 6];
            local[2] = def.faces.top;
            local[3] = def.faces.bottom;
            local[5] = def.faces.front;
            let mut faces = [[0; 3]; 6];
            for (index, color) in local.into_iter().enumerate() {
                faces[face_index(rotation.apply_dir(FACE_DIRS[index]))] = color;
            }

            let model = def.model.as_ref().and_then(|id| {
                let index = content.models.iter().position(|m| m.id == *id)?;
                let cell = content.models[index].model.cell(*def.parts.get(state.part as usize)?)?;
                Some(ModelCell { model: index as u16, voxels: rotation.rotate(&cell) })
            });
            let collision = match (&def.collision, &model) {
                _ if !def.solid => Collision::Empty,
                (CollisionShape::Full, _) => Collision::Full,
                (CollisionShape::Model, Some(cell)) => collision_of(&cell.voxels),
                (CollisionShape::Model, None) => Collision::Full,
                (CollisionShape::Boxes(boxes), _) => {
                    collision_of(&rotation.rotate(&model::rasterize(boxes)))
                }
            };
            let cube = model.is_none();
            props[number.0 as usize] = BlockProps {
                visible: true,
                opaque: cube && !def.transparent,
                transparent: cube && def.transparent,
                targetable: def.solid || !cube,
                collision,
                light: def.light,
                opacity: if cube && def.transparent { def.opacity } else { 255 },
                faces,
                model,
            };
        }
        Self { props }
    }

    #[inline]
    pub fn get(&self, block: Block) -> &BlockProps {
        if block == Block::AIR {
            return &BlockProps::AIR;
        }
        self.props.get(block.0 as usize).unwrap_or(&BlockProps::UNKNOWN)
    }

    /// Números de bloque que cubre la tabla.
    pub fn len(&self) -> usize {
        self.props.len()
    }

    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    /// Las dos tablas dan la misma geometría (si no, hay que volver a mallar).
    pub fn same_geometry(&self, other: &BlockTable) -> bool {
        self.props.len() == other.props.len()
            && self.props.iter().zip(&other.props).all(|(a, b)| {
                (a.visible, a.opaque, a.transparent, &a.model)
                    == (b.visible, b.opaque, b.transparent, &b.model)
            })
    }
}

fn collision_of(cell: &CellVoxels) -> Collision {
    let boxes = model::boxes(cell);
    match boxes.as_slice() {
        [] => Collision::Empty,
        [[0, 0, 0, 8, 8, 8]] => Collision::Full,
        _ => Collision::Boxes(boxes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{BlockDef, FaceColors, Loaded, ModelDef, Orientation};
    use crate::model::Model;

    fn table(blocks: Vec<BlockDef>, models: Vec<ModelDef>) -> (ContentData, BlockTable) {
        let content = ContentData::assign(Loaded { blocks, items: vec![], models }, &[], &[]).unwrap();
        let table = BlockTable::new(&content);
        (content, table)
    }

    #[test]
    fn faces_follow_the_orientation() {
        let faces = FaceColors { top: [1; 3], side: [2; 3], bottom: [3; 3], front: [4; 3] };
        let log = BlockDef { orientation: Orientation::Axis, faces, ..BlockDef::simple("a:log", "l", [2; 3]) };
        let kiln = BlockDef { orientation: Orientation::Horizontal, faces, ..BlockDef::simple("a:kiln", "k", [2; 3]) };
        let (content, table) = table(vec![log, kiln], vec![]);

        let up = table.get(content.block_id("a:log").unwrap());
        assert_eq!((up.faces[2], up.faces[3], up.faces[0]), ([1; 3], [3; 3], [2; 3]));
        let x = table.get(content.block_id("a:log[axis=x]").unwrap());
        assert_eq!((x.faces[0], x.faces[1], x.faces[2]), ([1; 3], [3; 3], [2; 3]));

        let east = table.get(content.block_id("a:kiln[facing=east]").unwrap());
        assert_eq!((east.faces[0], east.faces[5], east.faces[2]), ([4; 3], [2; 3], [1; 3]));
        let north = table.get(content.block_id("a:kiln").unwrap());
        assert_eq!(north.faces[5], [4; 3]);
    }

    #[test]
    fn models_give_shape_and_collision() {
        let slab_model = Model { size: [8, 4, 8], voxels: vec![1; 256], palette: vec![[9; 3]] };
        let flower_model = Model { size: [2, 5, 2], voxels: vec![1; 20], palette: vec![[9; 3]] };
        let models = vec![
            ModelDef { id: "a:flower".into(), model: flower_model },
            ModelDef { id: "a:slab".into(), model: slab_model },
        ];
        let slab = BlockDef {
            model: Some("a:slab".into()),
            collision: CollisionShape::Model,
            ..BlockDef::simple("a:slab", "s", [1; 3])
        };
        let flower = BlockDef {
            model: Some("a:flower".into()),
            solid: false,
            ..BlockDef::simple("a:flower", "f", [1; 3])
        };
        let glass = BlockDef { transparent: true, opacity: 80, ..BlockDef::simple("a:glass", "g", [1; 3]) };
        let water = BlockDef { solid: false, ..glass.clone() };
        let water = BlockDef { id: "a:water".into(), ..water };
        let (content, table) = table(vec![slab, flower, glass, water], models);

        let slab = table.get(content.block_id("a:slab").unwrap());
        assert_eq!(slab.collision, Collision::Boxes(vec![[0, 0, 0, 8, 4, 8]]));
        assert!(!slab.opaque && slab.targetable && slab.model.is_some());

        let flower = table.get(content.block_id("a:flower").unwrap());
        assert_eq!(flower.collision, Collision::Empty);
        assert!(flower.targetable, "flowers can be broken");

        let glass = table.get(content.block_id("a:glass").unwrap());
        assert!(glass.transparent && !glass.opaque && glass.targetable);
        assert_eq!((glass.collision.clone(), glass.opacity), (Collision::Full, 80));
        let water = table.get(content.block_id("a:water").unwrap());
        assert!(!water.targetable && water.collision == Collision::Empty);

        assert!(!table.get(Block::AIR).visible);
        assert!(table.get(Block(999)).opaque, "unknown numbers are solid");
    }
}
