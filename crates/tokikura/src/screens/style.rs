//! Piezas de aspecto que comparten las pantallas: fondo, panel, botones de menú y
//! ranuras de ítem. Cambiar el aspecto general de la interfaz empieza aquí.

use kuravoxel_common::content::ItemDef;

use crate::icons::Icon;
use kuravoxel_ui::core::{border, Alignment, Color, Length};
use kuravoxel_ui::widget::{button, center, container, image, space, stack, text, Button};
use kuravoxel_ui::Element;

/// Ancho de los paneles de menú.
pub const MENU_WIDTH: f32 = 400.0;
/// Lado de una ranura de ítem.
pub const SLOT_SIZE: f32 = 48.0;
const SWATCH_SIZE: f32 = 30.0;
/// Lado del icono dentro de una ranura.
const ICON_SIZE: f32 = 40.0;

/// Texto secundario (pistas, estado).
pub const MUTED: Color = Color::from_rgb(0.72, 0.74, 0.78);
/// Texto de error.
pub const ERROR: Color = Color::from_rgb(1.0, 0.45, 0.4);

/// Fondo de una pantalla que tapa el juego: oscurece la escena y centra el contenido.
pub fn backdrop<'a, Message: 'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .center(Length::Fill)
        .style(|_| container::background(Color::from_rgba(0.0, 0.0, 0.0, 0.5)))
        .into()
}

/// Caja con borde redondeado en la que va el contenido de un menú.
pub fn panel<'a, Message: 'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content).padding(28).style(container::rounded_box).into()
}

/// Botón de menú a todo el ancho, con el texto centrado. Sin `on_press` queda desactivado.
pub fn menu_button<'a, Message: Clone + 'a>(label: &'a str) -> Button<'a, Message> {
    button(text(label).width(Length::Fill).align_x(Alignment::Center))
        .width(Length::Fill)
        .padding(10)
}

/// Color de un ítem para la interfaz.
pub fn item_color(item: &ItemDef) -> Color {
    let [r, g, b] = item.color;
    Color::from_rgb8(r, g, b)
}

/// Una ranura de ítem: fondo oscuro, borde destacado si es la activa, el icono del ítem
/// (o su color si no tiene) dentro, nada si está vacía y, opcionalmente, una etiqueta en
/// la esquina.
pub fn slot<'a, Message: 'a>(
    item: Option<&ItemDef>,
    icon: Option<&Icon>,
    active: bool,
    label: Option<String>,
) -> Element<'a, Message> {
    let mut layers: Vec<Element<'a, Message>> = Vec::new();
    if let Some(icon) = icon.filter(|_| item.is_some()) {
        let filter = if icon.hand_drawn { image::FilterMethod::Nearest } else { image::FilterMethod::Linear };
        layers.push(
            center(image(icon.handle.clone()).width(ICON_SIZE).height(ICON_SIZE).filter_method(filter))
                .into(),
        );
    } else if let Some(item) = item {
        let color = item_color(item);
        layers.push(
            center(container(space()).width(SWATCH_SIZE).height(SWATCH_SIZE).style(move |_| {
                container::background(color).border(border::rounded(4))
            }))
            .into(),
        );
    }
    if let Some(label) = label {
        layers.push(container(text(label).size(11).color(MUTED)).padding([3, 6]).into());
    }
    let frame = if active {
        border::rounded(6).width(3).color(Color::WHITE)
    } else {
        border::rounded(6).width(1).color(Color::from_rgba(1.0, 1.0, 1.0, 0.35))
    };
    container(stack(layers).width(Length::Fill).height(Length::Fill))
        .width(SLOT_SIZE)
        .height(SLOT_SIZE)
        .style(move |_| container::background(Color::from_rgba(0.0, 0.0, 0.0, 0.55)).border(frame))
        .into()
}
