//! Panel de cielo y luz (provisional): la hora del mundo, la duración del día y todo lo
//! que se ajusta a ojo del sol, las sombras y los colores del cielo, con el mundo a la
//! vista detrás para ver el resultado al momento.
//!
//! La pantalla no cambia nada: lo pide (`Request`) y `game.rs` se lo encarga al motor (el
//! ambiente) o al servidor (la hora).

use kuravoxel_client::render::Environment;
use kuravoxel_common::protocol::{WorldTime, DEFAULT_DAY_LENGTH};
use kuravoxel_ui::core::alignment::Horizontal;
use kuravoxel_ui::core::{Alignment, Length};
use kuravoxel_ui::widget::{button, checkbox, column, container, row, scrollable, slider, text, text_input, Column, Row};
use kuravoxel_ui::{Element, Screen};

use super::style;
use crate::environment::{self, Scalar, PHASES, SLOTS, SLOT_LABELS};

/// Duración del día que ofrece el deslizador, en minutos reales.
const DAY_MINUTES: std::ops::RangeInclusive<f32> = 0.5..=120.0;
const PANEL_WIDTH: f32 = 400.0;
const LABEL_WIDTH: f32 = 118.0;
const VALUE_WIDTH: f32 = 58.0;

/// Lo que la pantalla pide al juego.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Poner esta hora del día (0-24).
    SetTime(f32),
    /// Duración del día en segundos reales; 0 = parar el tiempo.
    SetDayLength(f32),
    /// Usar este ambiente desde ya.
    Apply(Environment),
    /// Guardarlo en su archivo.
    Save,
    Close,
}

#[derive(Debug, Clone)]
pub enum Message {
    Time(f32),
    DayMinutes(f32),
    Paused(bool),
    Scalar(Scalar, f32),
    /// Texto de un color: momento del día y cuál (índices en `PHASES` y `SLOTS`).
    Color(usize, usize, String),
    Save,
    Reset,
    Close,
}

pub struct Sky {
    // --- Copiado del juego en cada frame ---
    time: WorldTime,
    environment: Environment,
    /// Esta partida aloja el servidor: solo así se puede cambiar la hora.
    hosting: bool,
    /// El ambiente tiene cambios sin guardar.
    unsaved: bool,

    // --- Propio de la pantalla ---
    /// Lo escrito en cada caja de color (puede estar a medias).
    colors: [[String; SLOTS.len()]; PHASES.len()],
    /// Duración del día a la que se vuelve al reanudar el tiempo.
    resume_length: f32,
    requests: Vec<Request>,
}

impl Default for Sky {
    fn default() -> Self {
        let environment = Environment::default();
        Self {
            time: WorldTime::default(),
            colors: color_texts(&environment),
            environment,
            hosting: false,
            unsaved: false,
            resume_length: DEFAULT_DAY_LENGTH,
            requests: Vec::new(),
        }
    }
}

fn color_texts(environment: &Environment) -> [[String; SLOTS.len()]; PHASES.len()] {
    std::array::from_fn(|p| std::array::from_fn(|s| environment::hex(environment::color(environment, p, s))))
}

impl Sky {
    /// Copia el estado que se va a dibujar.
    pub fn sync(&mut self, time: WorldTime, environment: &Environment, hosting: bool, unsaved: bool) {
        self.time = time;
        self.hosting = hosting;
        self.unsaved = unsaved;
        if time.day_length > 0.0 {
            self.resume_length = time.day_length;
        }
        // Cambiado desde fuera (la consola, o los valores por defecto): las cajas de
        // color se ponen al día.
        if *environment != self.environment {
            self.environment = environment.clone();
            self.colors = color_texts(environment);
        }
    }

    /// Peticiones pendientes para el juego, en orden.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    fn apply(&mut self) {
        self.environment = self.environment.clone().sanitized();
        self.requests.push(Request::Apply(self.environment.clone()));
    }
}

impl Screen for Sky {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {
            Message::Time(hours) => {
                self.time.hours = hours;
                self.requests.push(Request::SetTime(hours));
            }
            Message::DayMinutes(minutes) => {
                self.resume_length = minutes * 60.0;
                self.time.day_length = self.resume_length;
                self.requests.push(Request::SetDayLength(self.resume_length));
            }
            Message::Paused(paused) => {
                self.time.day_length = if paused { 0.0 } else { self.resume_length };
                self.requests.push(Request::SetDayLength(self.time.day_length));
            }
            Message::Scalar(scalar, value) => {
                scalar.set(&mut self.environment, value);
                self.apply();
            }
            Message::Color(phase, slot, written) => {
                // Solo cuenta cuando ya es un color entero; mientras, se deja escribir.
                if let Some(color) = environment::parse_hex(&written) {
                    environment::set_color(&mut self.environment, phase, slot, color);
                    self.apply();
                }
                self.colors[phase][slot] = written;
            }
            Message::Save => self.requests.push(Request::Save),
            Message::Reset => {
                self.environment = Environment::default();
                self.colors = color_texts(&self.environment);
                self.apply();
            }
            Message::Close => self.requests.push(Request::Close),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        fn setting<'a>(
            label: &'a str,
            control: impl Into<Element<'a, Message>>,
            value: String,
        ) -> Element<'a, Message> {
            row![
                text(label).size(13).width(LABEL_WIDTH),
                control.into(),
                text(value).size(13).width(VALUE_WIDTH)
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        }
        let small = |label: &'static str| button(text(label).size(13)).padding([5, 10]);

        // --- Hora ---
        let paused = self.time.day_length <= 0.0;
        let minutes = self.resume_length / 60.0;
        let presets = [("Dawn", 6.5), ("Noon", 12.0), ("Dusk", 17.5), ("Midnight", 0.0)].map(|(label, hours)| {
            small(label)
                .style(button::secondary)
                .on_press_maybe(self.hosting.then_some(Message::Time(hours)))
                .into()
        });
        let mut time = column![
            text("Time of day").size(18),
            setting(
                "Time",
                slider(0.0..=23.99_f32, self.time.hours, Message::Time).step(0.05_f32),
                self.time.clock()
            ),
            text(format!("Day {}; tonight: {}", self.time.day, self.time.moon_phase())).size(13).color(style::MUTED),
            Row::with_children(presets).spacing(6),
            setting(
                "Day length",
                slider(DAY_MINUTES, minutes.clamp(*DAY_MINUTES.start(), *DAY_MINUTES.end()), Message::DayMinutes)
                    .step(0.5_f32),
                format!("{minutes:.1} min")
            ),
            row![checkbox(paused).on_toggle(Message::Paused), text("Stop time").size(13)]
                .spacing(8)
                .align_y(Alignment::Center),
        ]
        .spacing(8);
        if !self.hosting {
            time = time.push(text("Only the host of the world can change the time.").size(12).color(style::ERROR));
        }

        // --- Sol y sombras ---
        let mut light = column![text("Sun and shadows").size(18)].spacing(8);
        for scalar in Scalar::ALL {
            let value = scalar.get(&self.environment);
            light = light.push(setting(
                scalar.label(),
                slider(scalar.range(), value, move |value| Message::Scalar(scalar, value)).step(scalar.step()),
                scalar.show(value),
            ));
        }

        // --- Colores ---
        let mut header: Vec<Element<'_, Message>> = vec![text("").width(LABEL_WIDTH - 40.0).into()];
        header.extend(PHASES.iter().map(|phase| text(*phase).size(13).width(Length::Fill).into()));
        let mut colors = column![text("Colors").size(18), Row::with_children(header).spacing(6)].spacing(6);
        for slot in 0..SLOTS.len() {
            let mut cells: Vec<Element<'_, Message>> =
                vec![text(SLOT_LABELS[slot]).size(13).width(LABEL_WIDTH - 40.0).into()];
            for phase in 0..PHASES.len() {
                cells.push(
                    text_input("#RRGGBB", &self.colors[phase][slot])
                        .size(13)
                        .padding(4)
                        .on_input(move |written| Message::Color(phase, slot, written))
                        .into(),
                );
            }
            colors = colors.push(Row::with_children(cells).spacing(6).align_y(Alignment::Center));
        }

        let save_label = if self.unsaved { "Save *" } else { "Save" };
        let content = Column::new()
            .push(text("Sky and light").size(24))
            .push(time)
            .push(light)
            .push(colors)
            .push(
                row![
                    small(save_label).on_press(Message::Save),
                    small("Defaults").style(button::secondary).on_press(Message::Reset),
                    small("Close").style(button::secondary).on_press(Message::Close),
                ]
                .spacing(8),
            )
            .push(text("Changes apply at once. Save keeps them in environment.toml.").size(12).color(style::MUTED))
            .spacing(16)
            .width(PANEL_WIDTH);

        // A un lado y sin oscurecer la escena: lo que se ajusta es lo que se ve detrás.
        container(container(scrollable(content)).padding(18).style(container::rounded_box))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Right)
            .padding(12)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Sky {
        let mut screen = Sky::default();
        screen.sync(WorldTime { hours: 9.0, day_length: 600.0, day: 0 }, &Environment::default(), true, false);
        screen
    }

    #[test]
    fn time_controls_ask_the_server() {
        let mut screen = screen();
        let _ = screen.view();
        screen.update(Message::Time(18.5));
        screen.update(Message::DayMinutes(5.0));
        assert_eq!(screen.take_requests(), [Request::SetTime(18.5), Request::SetDayLength(300.0)]);

        // Parar y reanudar vuelve a la duración que había.
        screen.update(Message::Paused(true));
        screen.update(Message::Paused(false));
        assert_eq!(screen.take_requests(), [Request::SetDayLength(0.0), Request::SetDayLength(300.0)]);
    }

    #[test]
    fn sliders_and_whole_colors_apply_at_once() {
        let mut screen = screen();
        screen.update(Message::Scalar(Scalar::ShadowSoftness, 0.2));
        let requests = screen.take_requests();
        assert!(matches!(&requests[..], [Request::Apply(env)] if env.shadow_softness == 0.2));

        // Un color a medio escribir no cambia nada, pero se conserva en su caja.
        screen.update(Message::Color(2, 0, "#1020".into()));
        assert!(screen.take_requests().is_empty());
        assert_eq!(screen.colors[2][0], "#1020");
        screen.update(Message::Color(2, 0, "#102030".into()));
        let requests = screen.take_requests();
        assert!(matches!(&requests[..], [Request::Apply(env)] if env.night.zenith == 0x102030));
        let _ = screen.view();
    }

    #[test]
    fn defaults_and_outside_changes_refresh_the_color_boxes() {
        let mut screen = screen();
        screen.update(Message::Color(0, 1, "#000000".into()));
        screen.update(Message::Reset);
        assert_eq!(screen.colors[0][1], environment::hex(Environment::default().day.horizon));
        assert_eq!(screen.take_requests().last(), Some(&Request::Apply(Environment::default())));

        let mut changed = Environment::default();
        changed.dusk.light = 0xABCDEF;
        screen.sync(WorldTime::default(), &changed, true, true);
        assert_eq!(screen.colors[1][2], "#ABCDEF");
    }
}
