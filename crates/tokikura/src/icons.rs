//! Iconos de los ítems para la interfaz.
//!
//! Se generan una vez cada vez que llega contenido nuevo: la imagen dibujada a mano si el
//! ítem la trae (`[icon] image`) o, si no, la vista de su modelo que dibuja el motor
//! (`kuravoxel_client::icons`). El comando `icons` de la consola los exporta a PNG para
//! poder retocarlos y usarlos como imagen propia.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kuravoxel_client::icons::{self as engine_icons, ICON_SIZE};
use kuravoxel_client::mesher;
use kuravoxel_common::content::{ContentData, ItemId};
use kuravoxel_common::icon::{self, IconImage};
use kuravoxel_common::shape::BlockTable;
use kuravoxel_ui::widget::image::Handle;

/// Icono de un ítem listo para Iced.
pub struct Icon {
    pub image: IconImage,
    pub handle: Handle,
    /// Dibujado a mano: se amplía sin suavizar, para que el pixel art siga nítido.
    pub hand_drawn: bool,
}

/// Iconos de todos los ítems del contenido actual.
#[derive(Default)]
pub struct Icons {
    icons: HashMap<ItemId, Icon>,
}

impl Icons {
    pub fn new(content: &ContentData) -> Self {
        let blocks = BlockTable::new(content);
        let (mesh, _) = mesher::build_tables(content, &blocks);
        let mut icons = HashMap::new();
        for (id, item) in content.numbered_items() {
            let (image, hand_drawn) = match &item.icon.image {
                Some(image) => (image.clone(), true),
                None => match engine_icons::item_shape(content, &blocks, &mesh, item) {
                    Some(shape) => (engine_icons::render_icon(&shape, &item.icon, ICON_SIZE), false),
                    // Sin modelo ni bloque (una herramienta): la ranura enseña su color.
                    None => continue,
                },
            };
            let handle = Handle::from_rgba(image.width, image.height, image.rgba.clone());
            icons.insert(id, Icon { image, handle, hand_drawn });
        }
        Self { icons }
    }

    pub fn get(&self, item: ItemId) -> Option<&Icon> {
        self.icons.get(&item)
    }

    /// Escribe cada icono como `<carpeta>/<paquete>/<nombre>.png`. Devuelve cuántos.
    pub fn export(&self, content: &ContentData, dir: &Path) -> anyhow::Result<usize> {
        let mut written = 0;
        for (id, item) in content.numbered_items() {
            let Some(icon) = self.icons.get(&id) else { continue };
            let (pack, name) = item.id.split_once(':').unwrap_or(("unknown", &item.id));
            let path: PathBuf = dir.join(pack).join(format!("{name}.png"));
            std::fs::create_dir_all(path.parent().expect("has a parent"))?;
            std::fs::write(&path, icon::encode_png(&icon.image)?)?;
            written += 1;
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuravoxel_common::content::test_content;

    #[test]
    fn every_block_item_gets_an_icon_and_they_can_be_exported() {
        let content = test_content(&["stone", "dirt"]);
        let icons = Icons::new(&content);
        let stone = icons.get(content.item_id("base:stone").unwrap()).unwrap();
        assert_eq!((stone.image.width, stone.hand_drawn), (ICON_SIZE, false));

        let dir = std::env::temp_dir().join(format!("tokikura-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(icons.export(&content, &dir).unwrap(), 2);
        let bytes = std::fs::read(dir.join("base/dirt.png")).unwrap();
        assert_eq!(icon::decode_png(&bytes).unwrap(), icons.get(content.item_id("base:dirt").unwrap()).unwrap().image);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
