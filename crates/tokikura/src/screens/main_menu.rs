//! Menú principal (provisional): la portada, con acceso a los mundos y a los ajustes.

use kuravoxel_ui::core::Alignment;
use kuravoxel_ui::widget::{column, text};
use kuravoxel_ui::{Element, Screen};

use super::style;

/// Lo que el menú pide al juego.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Ir a la lista de mundos.
    Worlds,
    Settings,
    Quit,
}

#[derive(Debug, Clone)]
pub enum Message {
    Play,
    Settings,
    Quit,
}

#[derive(Default)]
pub struct MainMenu {
    /// Aviso al pie (por ejemplo, por qué terminó la última partida).
    status: String,
    requests: Vec<Request>,
}

impl MainMenu {
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    /// Peticiones pendientes para el juego, en orden.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }
}

impl Screen for MainMenu {
    type Message = Message;

    fn update(&mut self, message: Message) {
        self.status.clear();
        self.requests.push(match message {
            Message::Play => Request::Worlds,
            Message::Settings => Request::Settings,
            Message::Quit => Request::Quit,
        });
    }

    fn view(&self) -> Element<'_, Message> {
        let panel = column![
            text("Tokikura").size(56),
            text("A voxel sandbox").size(16).color(style::MUTED),
            column![
                style::menu_button("Play").on_press(Message::Play),
                style::menu_button("Settings").on_press(Message::Settings),
                style::menu_button("Quit").style(kuravoxel_ui::widget::button::secondary).on_press(Message::Quit),
            ]
            .spacing(10)
            .padding([16, 0]),
            text(&self.status).size(14).color(style::ERROR),
            text(concat!("v", env!("CARGO_PKG_VERSION"), " - development build"))
                .size(12)
                .color(style::MUTED),
        ]
        .spacing(6)
        .align_x(Alignment::Center)
        .width(style::MENU_WIDTH);

        style::backdrop(style::panel(panel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_button_becomes_a_request_and_clears_the_notice() {
        let mut menu = MainMenu::default();
        menu.set_status("Session ended: disconnected");
        let _ = menu.view();
        for (message, request) in [
            (Message::Play, Request::Worlds),
            (Message::Settings, Request::Settings),
            (Message::Quit, Request::Quit),
        ] {
            menu.update(message);
            assert_eq!(menu.take_requests(), [request]);
        }
        assert!(menu.status.is_empty());
    }
}
