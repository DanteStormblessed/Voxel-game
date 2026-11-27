//! Menú de pausa (provisional): lo que se ve al pulsar Escape dentro de un mundo.
//!
//! El mundo no se detiene (el servidor sigue, también en un jugador): solo se suelta el
//! ratón y se tapa la vista.

use kuravoxel_ui::core::Alignment;
use kuravoxel_ui::widget::{button, column, text};
use kuravoxel_ui::{Element, Screen};

use super::style;

/// Lo que el menú pide al juego.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Resume,
    Settings,
    /// Abrir el editor de biomas (en su propia ventana).
    BiomeEditor,
    /// Guardar, salir del mundo y volver al menú principal.
    Leave,
    /// Guardar y cerrar el juego.
    Quit,
}

#[derive(Debug, Clone)]
pub enum Message {
    Resume,
    Settings,
    BiomeEditor,
    Leave,
    Quit,
}

#[derive(Default)]
pub struct PauseMenu {
    requests: Vec<Request>,
}

impl PauseMenu {
    /// Peticiones pendientes para el juego, en orden.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }
}

impl Screen for PauseMenu {
    type Message = Message;

    fn update(&mut self, message: Message) {
        self.requests.push(match message {
            Message::Resume => Request::Resume,
            Message::Settings => Request::Settings,
            Message::BiomeEditor => Request::BiomeEditor,
            Message::Leave => Request::Leave,
            Message::Quit => Request::Quit,
        });
    }

    fn view(&self) -> Element<'_, Message> {
        let panel = column![
            text("Game menu").size(32),
            column![
                style::menu_button("Back to game").on_press(Message::Resume),
                style::menu_button("Settings").on_press(Message::Settings),
                style::menu_button("Biome editor").on_press(Message::BiomeEditor),
                style::menu_button("Save and exit to menu").on_press(Message::Leave),
                style::menu_button("Save and quit game").style(button::secondary).on_press(Message::Quit),
            ]
            .spacing(10)
            .padding([12, 0]),
            text("E: blocks   º / F1: console   F3: debug info   F7: biome editor").size(13).color(style::MUTED),
        ]
        .spacing(6)
        .align_x(Alignment::Center)
        .width(style::MENU_WIDTH - 60.0);

        style::backdrop(style::panel(panel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_button_becomes_a_request() {
        let mut menu = PauseMenu::default();
        let _ = menu.view();
        for (message, request) in [
            (Message::Resume, Request::Resume),
            (Message::Settings, Request::Settings),
            (Message::BiomeEditor, Request::BiomeEditor),
            (Message::Leave, Request::Leave),
            (Message::Quit, Request::Quit),
        ] {
            menu.update(message);
            assert_eq!(menu.take_requests(), [request]);
        }
    }
}
