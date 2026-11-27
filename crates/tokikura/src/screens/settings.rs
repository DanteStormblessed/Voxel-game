//! Ajustes del juego (provisional). Se abre desde el menú principal y desde la pausa.
//!
//! Trabaja sobre un borrador: nada cambia hasta pulsar "Done".

use kuravoxel_ui::core::{Alignment, Length};
use kuravoxel_ui::widget::{button, checkbox, column, row, slider, text, text_input};
use kuravoxel_ui::{Element, Screen};

use super::style;
use crate::settings::{self, UserSettings};
use kuravoxel_client::render::ShadowQuality;

/// Lo que la pantalla pide al juego.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Aplicar y guardar estos ajustes, y cerrar la pantalla.
    Apply(UserSettings),
    /// Cerrar sin cambiar nada.
    Cancel,
}

#[derive(Debug, Clone)]
pub enum Message {
    NameChanged(String),
    ViewDistance(u8),
    FarDistance(u8),
    Fov(f32),
    MouseSensitivity(f32),
    Vsync(bool),
    UiScale(f32),
    /// Calidad de las sombras, como posición en `ShadowQuality::ALL`.
    Shadows(u8),
    /// Tope de fps, como posición en `settings::MAX_FPS_CHOICES`.
    MaxFps(u8),
    Reset,
    Done,
    Cancel,
}

#[derive(Default)]
pub struct SettingsScreen {
    draft: UserSettings,
    /// Hay un mundo abierto: las distancias no cambian hasta volver a entrar.
    in_world: bool,
    requests: Vec<Request>,
}

impl SettingsScreen {
    /// Abre la pantalla con los ajustes actuales como borrador.
    pub fn open(&mut self, current: &UserSettings, in_world: bool) {
        self.draft = current.clone();
        self.in_world = in_world;
    }

    /// Peticiones pendientes para el juego, en orden.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }
}

impl Screen for SettingsScreen {
    type Message = Message;

    fn update(&mut self, message: Message) {
        let draft = &mut self.draft;
        match message {
            Message::NameChanged(name) => draft.player_name = name,
            Message::ViewDistance(chunks) => {
                draft.view_distance = chunks;
                draft.far_distance = draft.far_distance.max(chunks);
            }
            Message::FarDistance(chunks) => draft.far_distance = chunks.max(draft.view_distance),
            Message::Fov(degrees) => draft.fov = degrees,
            Message::MouseSensitivity(factor) => draft.mouse_sensitivity = factor,
            Message::Vsync(on) => draft.vsync = on,
            Message::UiScale(scale) => draft.ui_scale = scale,
            Message::Shadows(level) => {
                let quality = ShadowQuality::ALL[(level as usize).min(ShadowQuality::ALL.len() - 1)];
                draft.shadows = quality.name().into();
            }
            Message::MaxFps(index) => {
                let index = (index as usize).min(settings::MAX_FPS_CHOICES.len() - 1);
                draft.max_fps = settings::MAX_FPS_CHOICES[index].into();
            }
            // El nombre no es una preferencia que "restaurar": se conserva.
            Message::Reset => {
                *draft = UserSettings { player_name: draft.player_name.clone(), ..UserSettings::default() }
            }
            Message::Done => self.requests.push(Request::Apply(self.draft.clone().sanitized())),
            Message::Cancel => self.requests.push(Request::Cancel),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        fn setting<'a>(
            label: &'a str,
            control: impl Into<Element<'a, Message>>,
            value: String,
        ) -> Element<'a, Message> {
            row![text(label).width(150), control.into(), text(value).width(64)]
                .spacing(12)
                .align_y(Alignment::Center)
                .into()
        }
        let draft = &self.draft;
        let distances_note = if self.in_world {
            "View distances apply the next time a world is opened."
        } else {
            "View distance is in chunks of 32 m; the server may grant less."
        };

        let panel = column![
            text("Settings").size(32),
            setting(
                "Player name",
                text_input("Player", &draft.player_name).on_input(Message::NameChanged),
                String::new()
            ),
            setting(
                "View distance",
                slider(settings::VIEW_DISTANCE, draft.view_distance, Message::ViewDistance),
                format!("{}", draft.view_distance)
            ),
            setting(
                "Far terrain",
                slider(settings::FAR_DISTANCE, draft.far_distance, Message::FarDistance),
                format!("{}", draft.far_distance)
            ),
            text(distances_note).size(13).color(style::MUTED),
            setting(
                "Field of view",
                slider(settings::FOV, draft.fov, Message::Fov).step(1.0_f32),
                format!("{:.0}°", draft.fov)
            ),
            setting(
                "Mouse sensitivity",
                slider(settings::MOUSE_SENSITIVITY, draft.mouse_sensitivity, Message::MouseSensitivity)
                    .step(0.05_f32),
                format!("{:.2}x", draft.mouse_sensitivity)
            ),
            setting(
                "Interface scale",
                slider(settings::UI_SCALE, draft.ui_scale, Message::UiScale).step(0.05_f32),
                format!("{:.0}%", draft.ui_scale * 100.0)
            ),
            setting(
                "Shadows",
                slider(
                    0..=(ShadowQuality::ALL.len() as u8 - 1),
                    ShadowQuality::ALL.iter().position(|q| *q == draft.shadow_quality()).unwrap_or(0) as u8,
                    Message::Shadows
                ),
                draft.shadows.clone()
            ),
            setting(
                "Max FPS",
                slider(
                    0..=(settings::MAX_FPS_CHOICES.len() as u8 - 1),
                    max_fps_index(&draft.max_fps),
                    Message::MaxFps
                ),
                match draft.max_fps.as_str() {
                    "monitor" => "Monitor".into(),
                    "unlimited" => "Unlimited".into(),
                    fps => fps.to_string(),
                }
            ),
            setting(
                "Vertical sync",
                row![checkbox(draft.vsync).on_toggle(Message::Vsync)].width(Length::Fill),
                if draft.vsync { "On".into() } else { "Off".into() }
            ),
            row![
                style::menu_button("Done").on_press(Message::Done),
                style::menu_button("Defaults").style(button::secondary).on_press(Message::Reset),
                style::menu_button("Cancel").style(button::secondary).on_press(Message::Cancel),
            ]
            .spacing(8)
            .padding([10, 0]),
        ]
        .spacing(12)
        .width(style::MENU_WIDTH + 80.0);

        style::backdrop(style::panel(panel))
    }
}

/// Posición del valor en la lista del deslizador; un número escrito a mano cae en la
/// opción más cercana por debajo.
fn max_fps_index(value: &str) -> u8 {
    let choices = settings::MAX_FPS_CHOICES;
    if let Some(index) = choices.iter().position(|choice| *choice == value) {
        return index as u8;
    }
    let fps: u32 = value.parse().unwrap_or(0);
    choices
        .iter()
        .rposition(|choice| choice.parse::<u32>().is_ok_and(|choice| choice <= fps))
        .unwrap_or(0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_fps_slider_walks_the_choices() {
        let mut screen = screen();
        assert_eq!(max_fps_index(&screen.draft.max_fps), 0, "monitor by default");
        screen.update(Message::MaxFps(6));
        assert_eq!(screen.draft.max_fps, "144");
        screen.update(Message::MaxFps(200));
        assert_eq!(screen.draft.max_fps, "unlimited");
        assert_eq!(max_fps_index("100"), 4, "a hand-written number shows as the closest choice below");
    }

    fn screen() -> SettingsScreen {
        let mut screen = SettingsScreen::default();
        screen.open(&UserSettings::default(), false);
        screen
    }

    #[test]
    fn nothing_is_applied_until_done() {
        let mut screen = screen();
        let _ = screen.view();
        screen.update(Message::Fov(95.0));
        screen.update(Message::Vsync(false));
        assert!(screen.take_requests().is_empty());

        screen.update(Message::Done);
        let expected = UserSettings { fov: 95.0, vsync: false, ..UserSettings::default() };
        assert_eq!(screen.take_requests(), [Request::Apply(expected)]);
    }

    #[test]
    fn far_terrain_never_ends_before_the_voxels() {
        let mut screen = screen();
        screen.update(Message::FarDistance(4));
        assert_eq!(screen.draft.far_distance, screen.draft.view_distance);
        screen.update(Message::ViewDistance(16));
        screen.update(Message::FarDistance(20));
        screen.update(Message::ViewDistance(8));
        assert_eq!((screen.draft.view_distance, screen.draft.far_distance), (8, 20));
    }

    #[test]
    fn cancel_and_reopen_discard_the_draft_and_defaults_keep_the_name() {
        let mut screen = screen();
        screen.update(Message::NameChanged("Kura".into()));
        screen.update(Message::UiScale(1.5));
        screen.update(Message::Reset);
        assert_eq!(screen.draft, UserSettings { player_name: "Kura".into(), ..UserSettings::default() });

        screen.update(Message::Cancel);
        assert_eq!(screen.take_requests(), [Request::Cancel]);
        screen.open(&UserSettings::default(), true);
        assert_eq!(screen.draft, UserSettings::default());
    }
}
