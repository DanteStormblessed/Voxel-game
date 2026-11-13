//! Lista de mundos (provisional): elegir uno para jugar, borrarlo o crear otro.

use std::path::PathBuf;

use kuravoxel_ui::core::{Alignment, Length};
use kuravoxel_ui::widget::{button, column, row, scrollable, space, text, Column};
use kuravoxel_ui::{Element, Screen};

use super::style;
use crate::worlds::{self, WorldInfo};

/// Lo que la pantalla pide al juego.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Abrir el mundo con este nombre.
    Play(String),
    /// Ir a la pantalla de crear un mundo.
    Create,
    Back,
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(usize),
    Play,
    Create,
    Delete,
    Back,
}

pub struct WorldSelect {
    worlds_dir: PathBuf,
    worlds: Vec<WorldInfo>,
    selected: Option<usize>,
    status: String,
    /// Mundo cuyo borrado está pendiente de confirmar con un segundo clic.
    confirm_delete: Option<String>,
    requests: Vec<Request>,
}

impl WorldSelect {
    pub fn new(worlds_dir: PathBuf) -> Self {
        Self {
            worlds_dir,
            worlds: Vec::new(),
            selected: None,
            status: String::new(),
            confirm_delete: None,
            requests: Vec::new(),
        }
    }

    /// Vuelve a leer los mundos del disco y muestra `status` al pie.
    pub fn refresh(&mut self, status: impl Into<String>) {
        self.worlds = worlds::list(&self.worlds_dir);
        self.selected = self.selected.filter(|&index| index < self.worlds.len());
        self.confirm_delete = None;
        self.status = status.into();
    }

    /// Nombres de los mundos que existen (para no crear uno repetido).
    pub fn names(&self) -> Vec<String> {
        self.worlds.iter().map(|world| world.name.clone()).collect()
    }

    /// Peticiones pendientes para el juego, en orden.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    fn selected_world(&self) -> Option<&WorldInfo> {
        self.selected.and_then(|index| self.worlds.get(index))
    }
}

impl Screen for WorldSelect {
    type Message = Message;

    fn update(&mut self, message: Message) {
        // Cualquier otra acción cancela un borrado a medias.
        let confirming = self.confirm_delete.take();
        match message {
            Message::Select(index) => {
                self.selected = (index < self.worlds.len()).then_some(index);
                self.status.clear();
            }
            Message::Play => match self.selected_world() {
                Some(world) => self.requests.push(Request::Play(world.name.clone())),
                None => self.status = "Select a world first.".into(),
            },
            Message::Create => self.requests.push(Request::Create),
            Message::Delete => match self.selected_world().map(|world| world.name.clone()) {
                None => self.status = "Select a world first.".into(),
                Some(world) if confirming.as_deref() == Some(world.as_str()) => {
                    let status = match worlds::delete(&self.worlds_dir, &world) {
                        Ok(()) => format!("World \"{world}\" deleted."),
                        Err(error) => format!("Could not delete \"{world}\": {error}"),
                    };
                    self.selected = None;
                    self.refresh(status);
                }
                Some(world) => {
                    self.status = format!("Press Delete again to permanently remove \"{world}\".");
                    self.confirm_delete = Some(world);
                }
            },
            Message::Back => self.requests.push(Request::Back),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let worlds: Element<'_, Message> = if self.worlds.is_empty() {
            text("No worlds yet. Create one to start playing.").color(style::MUTED).into()
        } else {
            Column::with_children(self.worlds.iter().enumerate().map(|(index, world)| {
                let seed = world.seed.map_or_else(|| "seed ?".to_string(), |seed| format!("seed {seed}"));
                let style = if self.selected == Some(index) { button::primary } else { button::text };
                button(
                    row![text(&world.name), space().width(Length::Fill), text(seed).size(13)]
                        .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .style(style)
                .on_press(Message::Select(index))
                .into()
            }))
            .spacing(2)
            .into()
        };

        let has_selection = self.selected.is_some();
        let delete_label = if self.confirm_delete.is_some() { "Confirm delete" } else { "Delete" };
        let panel = column![
            text("Worlds").size(32),
            scrollable(worlds).height(240),
            row![
                style::menu_button("Play").on_press_maybe(has_selection.then_some(Message::Play)),
                style::menu_button(delete_label)
                    .style(button::danger)
                    .on_press_maybe(has_selection.then_some(Message::Delete)),
            ]
            .spacing(8),
            style::menu_button("Create new world").on_press(Message::Create),
            text(&self.status).size(14),
            style::menu_button("Back").style(button::secondary).on_press(Message::Back),
        ]
        .spacing(12)
        .width(style::MENU_WIDTH);

        style::backdrop(style::panel(panel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Carpeta temporal con unos cuantos mundos y la pantalla ya abierta sobre ella.
    struct Fixture {
        dir: PathBuf,
        screen: WorldSelect,
    }

    impl Fixture {
        fn new(test: &str, worlds: &[&str]) -> Self {
            let dir = std::env::temp_dir().join(format!("tokikura-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            for world in worlds {
                std::fs::create_dir_all(dir.join(world)).unwrap();
                std::fs::write(dir.join(world).join("world.toml"), "seed = 1\nblocks = []\n").unwrap();
            }
            let mut screen = WorldSelect::new(dir.clone());
            screen.refresh("");
            Self { dir, screen }
        }

        fn send(&mut self, message: Message) -> Vec<Request> {
            self.screen.update(message);
            self.screen.take_requests()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn lists_existing_worlds_in_order() {
        let fixture = Fixture::new("list", &["beta", "Alpha"]);
        assert_eq!(fixture.screen.names(), ["Alpha", "beta"]);
        // La vista se construye sin GPU ni ventana.
        let _ = fixture.screen.view();
    }

    #[test]
    fn play_needs_a_selection_and_requests_that_world() {
        let mut fixture = Fixture::new("play", &["alpha", "beta"]);
        assert!(fixture.send(Message::Play).is_empty());
        assert_eq!(fixture.screen.status, "Select a world first.");

        fixture.send(Message::Select(1));
        assert_eq!(fixture.send(Message::Play), [Request::Play("beta".into())]);
    }

    #[test]
    fn delete_asks_twice_and_removes_only_that_world() {
        let mut fixture = Fixture::new("delete", &["alpha", "beta"]);
        fixture.send(Message::Select(0));

        fixture.send(Message::Delete);
        assert!(fixture.screen.status.contains("Press Delete again"));
        assert!(fixture.dir.join("alpha").exists(), "the first click must not delete");

        // Otra acción entre medias cancela la confirmación.
        fixture.send(Message::Select(0));
        fixture.send(Message::Delete);
        assert!(fixture.dir.join("alpha").exists());

        fixture.send(Message::Delete);
        assert!(!fixture.dir.join("alpha").exists());
        assert!(fixture.dir.join("beta").exists());
        assert_eq!(fixture.screen.names(), ["beta"]);
        assert_eq!(fixture.screen.status, "World \"alpha\" deleted.");
        assert_eq!(fixture.screen.selected, None);
    }

    #[test]
    fn navigation_is_forwarded_to_the_game() {
        let mut fixture = Fixture::new("nav", &[]);
        assert_eq!(fixture.send(Message::Create), [Request::Create]);
        assert_eq!(fixture.send(Message::Back), [Request::Back]);
    }
}
