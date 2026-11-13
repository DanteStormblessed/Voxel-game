//! Crear un mundo (provisional): nombre y semilla.

use kuravoxel_ui::core::Alignment;
use kuravoxel_ui::widget::{button, column, row, text, text_input};
use kuravoxel_ui::{Element, Screen};

use super::style;
use crate::worlds;

/// Nombre de la caja de texto del nombre, para darle el foco al abrir la pantalla.
pub const NAME_INPUT: &str = "world-create-name";

/// Lo que la pantalla pide al juego.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Crear y abrir un mundo con este nombre y esta semilla.
    Create { name: String, seed: u32 },
    Cancel,
}

#[derive(Debug, Clone)]
pub enum Message {
    NameChanged(String),
    SeedChanged(String),
    Create,
    Cancel,
}

#[derive(Default)]
pub struct WorldCreate {
    /// Mundos que ya existen: no se puede repetir nombre.
    existing: Vec<String>,
    name: String,
    seed: String,
    status: String,
    requests: Vec<Request>,
}

impl WorldCreate {
    /// Deja la pantalla en blanco para crear un mundo nuevo.
    pub fn open(&mut self, existing: Vec<String>) {
        *self = Self { existing, ..Self::default() };
    }

    /// Peticiones pendientes para el juego, en orden.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }
}

impl Screen for WorldCreate {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {
            Message::NameChanged(name) => self.name = name,
            Message::SeedChanged(seed) => self.seed = seed,
            Message::Create => {
                let name = self.name.trim().to_string();
                if let Err(problem) = worlds::validate_name(&name) {
                    self.status = problem.into();
                } else if self.existing.iter().any(|world| world.eq_ignore_ascii_case(&name)) {
                    self.status = "A world with that name already exists.".into();
                } else {
                    // El servidor crea la carpeta al abrir un mundo que no existe.
                    let seed = worlds::parse_seed(&self.seed).unwrap_or_else(worlds::random_seed);
                    self.requests.push(Request::Create { name, seed });
                }
            }
            Message::Cancel => self.requests.push(Request::Cancel),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let field = |label: &'static str, input: Element<'static, Message>| {
            row![text(label).width(70), input].spacing(12).align_y(Alignment::Center)
        };
        let panel = column![
            text("New world").size(32),
            field(
                "Name",
                text_input("Letters, digits, '-' and '_'", &self.name)
                    .id(NAME_INPUT)
                    .on_input(Message::NameChanged)
                    .on_submit(Message::Create)
                    .into()
            ),
            field(
                "Seed",
                text_input("Leave empty for a random one", &self.seed)
                    .on_input(Message::SeedChanged)
                    .on_submit(Message::Create)
                    .into()
            ),
            text("The seed can be a number or any text: the same seed always gives the same terrain.")
                .size(13)
                .color(style::MUTED),
            text(&self.status).size(14).color(style::ERROR),
            row![
                style::menu_button("Create").on_press(Message::Create),
                style::menu_button("Cancel").style(button::secondary).on_press(Message::Cancel),
            ]
            .spacing(8),
        ]
        .spacing(12)
        .width(style::MENU_WIDTH);

        style::backdrop(style::panel(panel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> WorldCreate {
        let mut screen = WorldCreate::default();
        screen.open(vec!["alpha".into()]);
        screen
    }

    #[test]
    fn create_validates_the_name() {
        let mut screen = screen();
        let _ = screen.view();
        for (typed, expected) in [
            ("", "Type a name for the new world."),
            ("../escape", "Use only letters, digits, '-' and '_'."),
            ("with space", "Use only letters, digits, '-' and '_'."),
            ("ALPHA", "A world with that name already exists."),
        ] {
            screen.update(Message::NameChanged(typed.into()));
            screen.update(Message::Create);
            assert!(screen.take_requests().is_empty(), "{typed:?} must be rejected");
            assert_eq!(screen.status, expected);
        }
    }

    #[test]
    fn the_seed_is_the_typed_one_or_a_random_one() {
        let mut screen = screen();
        screen.update(Message::NameChanged("  new_world-2 ".into()));
        screen.update(Message::SeedChanged("42".into()));
        screen.update(Message::Create);
        assert_eq!(screen.take_requests(), [Request::Create { name: "new_world-2".into(), seed: 42 }]);

        screen.update(Message::SeedChanged(String::new()));
        screen.update(Message::Create);
        assert!(matches!(&screen.take_requests()[..], [Request::Create { name, .. }] if name == "new_world-2"));
    }

    #[test]
    fn opening_again_starts_from_scratch_and_cancel_is_forwarded() {
        let mut screen = screen();
        screen.update(Message::NameChanged("x".into()));
        screen.open(Vec::new());
        assert!(screen.name.is_empty());
        screen.update(Message::Cancel);
        assert_eq!(screen.take_requests(), [Request::Cancel]);
    }
}
