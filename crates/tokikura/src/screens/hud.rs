//! HUD (provisional): punto de mira, barra de herramientas y, con F3, datos de depuración.
//!
//! Solo se mira: no recibe entrada ni emite mensajes. `game.rs` le copia en cada frame
//! lo que tiene que enseñar.

use std::sync::Arc;

use kuravoxel_client::session::SessionInfo;
use kuravoxel_common::content::ContentData;
use kuravoxel_ui::core::{Alignment, Color, Font, Length, Padding};
use kuravoxel_ui::widget::{bottom_center, center, center_x, column, container, row, space, stack, text, Column};
use kuravoxel_ui::{Element, Screen};

use super::style;
use crate::hotbar::Hotbar;
use crate::icons::Icons;

/// Largo y grosor de cada brazo del punto de mira.
const CROSSHAIR: (f32, f32) = (18.0, 2.0);

/// El HUD no tiene nada que se pueda pulsar.
#[derive(Debug, Clone)]
pub enum Message {}

#[derive(Default)]
pub struct Hud {
    content: Arc<ContentData>,
    icons: Arc<Icons>,
    hotbar: Hotbar,
    /// El punto de mira solo se ve mientras se juega (sin ninguna pantalla encima).
    pub crosshair: bool,
    /// La carga inicial del mundo ha terminado.
    loaded: bool,
    /// Aviso breve en lo alto de la pantalla (p. ej. el modo de juego recién elegido).
    pub notice: Option<String>,
    /// Líneas de la herramienta de construcción activa (vacío = ninguna).
    pub tool: Vec<String>,
    /// Rótulos sobre el mundo (las medidas de la selección): posición en pantalla, de
    /// 0 a 1 desde la esquina superior izquierda, y texto.
    pub labels: Vec<([f32; 2], String)>,
    /// Datos de depuración a mostrar (F3).
    debug: Option<Debug>,
}

struct Debug {
    /// Modo de juego y vista, ya como texto.
    mode: String,
    status: Arc<str>,
    position: [f64; 3],
    target: Option<[i32; 3]>,
    /// Nombre del estado de bloque apuntado (`base:wood[axis=x]`).
    target_block: Option<String>,
}

impl Hud {
    /// Copia el estado que se va a dibujar.
    pub fn sync(&mut self, info: &SessionInfo, icons: &Arc<Icons>, hotbar: &Hotbar, mode: &str, show_debug: bool) {
        self.content = info.content.clone();
        self.icons = icons.clone();
        self.hotbar = *hotbar;
        self.loaded = info.loaded;
        self.debug = show_debug.then(|| Debug {
            mode: format!("mode: {mode} | camera: {:.0} m", info.camera_distance),
            status: info.status.clone(),
            position: info.position,
            target: info.target,
            target_block: info.target_block.and_then(|block| info.content.state_name(block)),
        });
    }

    fn hotbar_view(&self) -> Element<'_, Message> {
        let slots = self.hotbar.slots().iter().enumerate().map(|(index, &item)| {
            style::slot(
                item.and_then(|item| self.content.item(item)),
                item.and_then(|item| self.icons.get(item)),
                index == self.hotbar.active(),
                Some((index + 1).to_string()),
            )
        });
        let name = self
            .hotbar
            .active_item()
            .and_then(|item| self.content.item(item))
            .map_or("", |item| item.name.as_str());
        bottom_center(
            column![text(name).size(15), row(slots).spacing(4)]
                .spacing(6)
                .align_x(Alignment::Center),
        )
        .padding(14)
        .into()
    }
}

/// Barra blanca del punto de mira, centrada en la pantalla.
fn crosshair_bar<'a>(width: f32, height: f32) -> Element<'a, Message> {
    center(
        container(space())
            .width(width)
            .height(height)
            .style(|_| container::background(Color::from_rgba(1.0, 1.0, 1.0, 0.85))),
    )
    .into()
}

impl Screen for Hud {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {}
    }

    fn view(&self) -> Element<'_, Message> {
        let mut layers = vec![self.hotbar_view()];
        if self.crosshair {
            let (length, thickness) = CROSSHAIR;
            layers.push(crosshair_bar(length, thickness));
            layers.push(crosshair_bar(thickness, length));
        }
        let notice = if self.loaded { self.notice.as_deref() } else { Some("Loading world...") };
        if let Some(notice) = notice {
            layers.push(
                center_x(
                    container(text(notice))
                        .padding([6, 14])
                        .style(container::rounded_box),
                )
                .padding(24)
                .into(),
            );
        }
        for (position, label) in &self.labels {
            // Sin conocer el tamaño de la ventana: el espacio se reparte en proporción.
            let portion = |value: f32| ((value.clamp(0.0, 1.0) * 1000.0) as u16).clamp(1, 999);
            let (x, y) = (portion(position[0]), portion(position[1]));
            let tag = container(text(label).size(15).color(Color::WHITE))
                .padding([1, 6])
                .style(|_| container::background(Color::from_rgba(0.0, 0.0, 0.0, 0.6)));
            layers.push(
                column![
                    space().height(Length::FillPortion(y)),
                    row![
                        space().width(Length::FillPortion(x)),
                        tag,
                        space().width(Length::FillPortion(1000 - x))
                    ],
                    space().height(Length::FillPortion(1000 - y)),
                ]
                .into(),
            );
        }
        if !self.tool.is_empty() {
            let lines = self.tool.iter().map(|line| text(line).size(13).font(Font::MONOSPACE).into());
            layers.push(
                container(row![
                    space().width(Length::Fill),
                    container(Column::with_children(lines).spacing(3))
                        .padding(10)
                        .style(|_| container::background(Color::from_rgba(0.0, 0.0, 0.0, 0.55))),
                ])
                // Por debajo del aviso que sale centrado en lo alto.
                .padding(Padding::new(10.0).top(72.0))
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            );
        }
        if let Some(debug) = &self.debug {
            let [x, y, z] = debug.position;
            let target = debug
                .target
                .map_or_else(|| "target: -".to_string(), |[x, y, z]| format!("target: {x} {y} {z}"));
            let target = match &debug.target_block {
                Some(name) => format!("{target} {name}"),
                None => target,
            };
            let lines = [format!("pos: {x:.1} {y:.1} {z:.1}"), target, debug.mode.clone()]
                .into_iter()
                // La línea de estado trae la posición sin decimales: ya está arriba.
                .chain(
                    debug
                        .status
                        .split(" | ")
                        .filter(|part| !part.starts_with("pos "))
                        .map(str::to_string),
                )
                .map(|line| text(line).size(13).font(Font::MONOSPACE).into());
            layers.push(
                container(
                    container(Column::with_children(lines).spacing(2))
                        .padding(8)
                        .style(|_| container::background(Color::from_rgba(0.0, 0.0, 0.0, 0.55))),
                )
                .padding(10)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            );
        }
        stack(layers).width(Length::Fill).height(Length::Fill).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotbar::tests::content;

    #[test]
    fn the_view_builds_in_every_state() {
        let mut hud = Hud::default();
        let _ = hud.view();

        let info = SessionInfo {
            content: Arc::new(content(&["stone", "dirt"])),
            status: Arc::from("60 fps | chunks 10"),
            target: Some([1, 2, 3]),
            ..SessionInfo::default()
        };
        let mut hotbar = Hotbar::default();
        hotbar.sync(&info.content);
        hud.crosshair = true;
        hud.notice = Some("Creative".into());
        hud.tool = vec!["Selection wand".into(), "LMB corner 1".into()];
        hud.labels = vec![([0.5, 0.25], "12".into())];
        hud.sync(&info, &Arc::new(Icons::new(&info.content)), &hotbar, "Creative", true);
        assert!(hud.debug.is_some());
        let _ = hud.view();
    }
}
