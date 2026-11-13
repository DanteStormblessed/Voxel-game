//! Consola de desarrollador (provisional): historial de salida y una línea de entrada.
//!
//! La consola no sabe ejecutar nada: entrega cada línea escrita como una petición y
//! `game.rs` decide (comando del cliente o del servidor) y le devuelve el resultado con
//! [`Console::print`]. También enseña el registro del motor.

use std::collections::VecDeque;

use kuravoxel_ui::core::{Color, Font, Length};
use kuravoxel_ui::widget::{column, container, scrollable, space, text, text_input, Column};
use kuravoxel_ui::{Element, Screen};

use super::style;

/// Nombre de la caja de texto, para darle el foco al abrir la consola.
pub const INPUT: &str = "console-input";
/// Líneas de salida que se conservan.
const MAX_LINES: usize = 400;
/// Órdenes que se recuerdan para repetirlas con las flechas.
const MAX_HISTORY: usize = 64;

/// De qué tipo es una línea de la consola (decide su color).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Lo que escribió el jugador.
    Input,
    /// Resultado de un comando.
    Output,
    Error,
    /// Registro del motor.
    Log,
}

/// Lo que la consola pide al juego.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Ejecutar esta línea.
    Run(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    InputChanged(String),
    Submit,
    /// Flechas arriba y abajo: recorrer las órdenes anteriores.
    HistoryPrevious,
    HistoryNext,
}

#[derive(Default)]
pub struct Console {
    lines: VecDeque<(Kind, String)>,
    input: String,
    /// Órdenes ejecutadas, de la más antigua a la más reciente.
    history: Vec<String>,
    /// Posición en `history` mientras se recorre con las flechas.
    history_cursor: Option<usize>,
    requests: Vec<Request>,
}

impl Console {
    /// Añade texto a la salida; cada línea del texto es una línea de la consola.
    pub fn print(&mut self, kind: Kind, text: &str) {
        for line in text.lines() {
            if self.lines.len() >= MAX_LINES {
                self.lines.pop_front();
            }
            self.lines.push_back((kind, line.to_string()));
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Peticiones pendientes para el juego, en orden.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    #[cfg(test)]
    pub fn lines(&self) -> Vec<(Kind, &str)> {
        self.lines.iter().map(|(kind, line)| (*kind, line.as_str())).collect()
    }
}

impl Screen for Console {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {
            Message::InputChanged(input) => {
                self.input = input;
                self.history_cursor = None;
            }
            Message::Submit => {
                let line = std::mem::take(&mut self.input).trim().to_string();
                self.history_cursor = None;
                if line.is_empty() {
                    return;
                }
                if self.history.last() != Some(&line) {
                    if self.history.len() >= MAX_HISTORY {
                        self.history.remove(0);
                    }
                    self.history.push(line.clone());
                }
                self.print(Kind::Input, &format!("> {line}"));
                self.requests.push(Request::Run(line));
            }
            Message::HistoryPrevious => {
                let previous = match self.history_cursor {
                    Some(cursor) => cursor.saturating_sub(1),
                    None => self.history.len().saturating_sub(1),
                };
                if let Some(line) = self.history.get(previous) {
                    self.input = line.clone();
                    self.history_cursor = Some(previous);
                }
            }
            Message::HistoryNext => {
                let Some(cursor) = self.history_cursor else { return };
                match self.history.get(cursor + 1) {
                    Some(line) => {
                        self.input = line.clone();
                        self.history_cursor = Some(cursor + 1);
                    }
                    // Pasada la más reciente: se vuelve a la línea en blanco.
                    None => {
                        self.input.clear();
                        self.history_cursor = None;
                    }
                }
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let lines = Column::with_children(self.lines.iter().map(|(kind, line)| {
            let color = match kind {
                Kind::Input => Color::from_rgb(0.6, 0.85, 1.0),
                Kind::Output => Color::WHITE,
                Kind::Error => style::ERROR,
                Kind::Log => style::MUTED,
            };
            text(line).size(14).font(Font::MONOSPACE).color(color).into()
        }))
        .spacing(2)
        .width(Length::Fill);

        let console = container(
            column![
                scrollable(lines).anchor_bottom().height(Length::Fill).width(Length::Fill),
                text_input("Type a command ('help' for a list)", &self.input)
                    .id(INPUT)
                    .font(Font::MONOSPACE)
                    .size(14)
                    .on_input(Message::InputChanged)
                    .on_submit(Message::Submit),
            ]
            .spacing(8),
        )
        .padding(10)
        .width(Length::Fill)
        .height(Length::FillPortion(1))
        .style(|_| container::background(Color::from_rgba(0.03, 0.04, 0.06, 0.88)));

        // La consola ocupa la mitad de arriba; por debajo se sigue viendo el mundo.
        column![console, space().height(Length::FillPortion(1))].into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(console: &mut Console, line: &str) -> Vec<Request> {
        console.update(Message::InputChanged(line.into()));
        console.update(Message::Submit);
        console.take_requests()
    }

    #[test]
    fn submitting_echoes_the_line_and_asks_the_game_to_run_it() {
        let mut console = Console::default();
        let _ = console.view();
        assert_eq!(run(&mut console, "  tp 0 80 0 "), [Request::Run("tp 0 80 0".into())]);
        assert_eq!(console.lines(), [(Kind::Input, "> tp 0 80 0")]);
        assert!(console.input.is_empty());

        assert!(run(&mut console, "   ").is_empty(), "an empty line does nothing");
        console.print(Kind::Output, "one\ntwo");
        assert_eq!(console.lines().len(), 3);
        console.clear();
        assert!(console.lines().is_empty());
    }

    #[test]
    fn arrows_walk_the_history_and_come_back_to_an_empty_line() {
        let mut console = Console::default();
        for line in ["seed", "pos", "pos"] {
            run(&mut console, line);
        }
        assert_eq!(console.history, ["seed", "pos"], "repeating a command is remembered once");

        console.update(Message::HistoryPrevious);
        assert_eq!(console.input, "pos");
        console.update(Message::HistoryPrevious);
        console.update(Message::HistoryPrevious);
        assert_eq!(console.input, "seed", "it stops at the oldest one");
        console.update(Message::HistoryNext);
        assert_eq!(console.input, "pos");
        console.update(Message::HistoryNext);
        assert_eq!(console.input, "");
        console.update(Message::HistoryNext);
        assert_eq!(console.input, "");
    }

    #[test]
    fn old_output_is_dropped() {
        let mut console = Console::default();
        for index in 0..MAX_LINES + 10 {
            console.print(Kind::Log, &format!("line {index}"));
        }
        assert_eq!(console.lines().len(), MAX_LINES);
        assert_eq!(console.lines()[0].1, "line 10");
    }
}
