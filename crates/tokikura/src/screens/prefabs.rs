//! Menú de prefabs (provisional): guardar la selección, cargar un prefab para colocarlo y
//! las acciones de la herramienta de construcción (copiar, cortar, vaciar, deshacer).
//!
//! La pantalla no copia ni guarda nada: lo pide (`Request`) y `game.rs` se lo encarga al
//! servidor. Lo que enseña (selección, portapapeles, lista) se lo copia `game.rs`.

use kuravoxel_common::prefab::PrefabInfo;
use kuravoxel_ui::core::{Alignment, Length};
use kuravoxel_ui::widget::{button, checkbox, column, row, scrollable, space, text, text_input, Column};
use kuravoxel_ui::{Element, Screen};

use super::style;
use crate::builder::{Builder, RegionAction};

/// Caja de texto del nombre, para darle el foco al abrir el menú.
pub const NAME_INPUT: &str = "prefab-name";

/// Lo que la pantalla pide al juego.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Guardar la selección con este nombre.
    Save(String),
    /// Cargar este prefab y empezar a colocarlo.
    Load(String),
    Delete(String),
    Region(RegionAction),
    Deselect,
    Undo,
    /// Empezar a colocar lo que hay en el portapapeles.
    Place,
    PasteAir(bool),
    Close,
}

#[derive(Debug, Clone)]
pub enum Message {
    NameChanged(String),
    FilterChanged(String),
    Save,
    Select(String),
    Load,
    Delete,
    Region(RegionAction),
    Deselect,
    Undo,
    Place,
    PasteAir(bool),
    Close,
}

#[derive(Default)]
pub struct Prefabs {
    // --- Copiado de la herramienta en cada frame ---
    selection: String,
    has_selection: bool,
    clipboard: String,
    has_clipboard: bool,
    paste_air: bool,
    prefabs: Vec<PrefabInfo>,
    status: Option<(String, bool)>,
    /// Esta partida aloja el servidor: solo así se pueden usar las herramientas.
    hosting: bool,

    // --- Propio de la pantalla ---
    name: String,
    filter: String,
    /// Identificador del prefab elegido en la lista.
    selected: Option<String>,
    /// Prefab cuyo borrado espera un segundo clic.
    confirm_delete: Option<String>,
    requests: Vec<Request>,
}

impl Prefabs {
    /// Copia el estado que se va a dibujar.
    pub fn sync(&mut self, builder: &Builder, hosting: bool) {
        self.selection = builder.selection_text();
        self.has_selection = builder.selection().is_some();
        self.clipboard = builder.clipboard_text();
        self.has_clipboard = builder.has_clipboard();
        self.paste_air = builder.paste_air;
        if self.prefabs != builder.prefabs() {
            self.prefabs = builder.prefabs().to_vec();
            self.selected = self.selected.take().filter(|id| self.prefabs.iter().any(|p| p.id == *id));
        }
        self.status = builder.status().map(|(text, ok)| (text.to_string(), ok));
        self.hosting = hosting;
    }

    /// Peticiones pendientes para el juego, en orden.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    fn shown_prefabs(&self) -> impl Iterator<Item = &PrefabInfo> {
        let filter = self.filter.trim().to_lowercase();
        self.prefabs.iter().filter(move |prefab| prefab.id.contains(&filter))
    }
}

impl Screen for Prefabs {
    type Message = Message;

    fn update(&mut self, message: Message) {
        // Cualquier otra acción cancela un borrado a medias.
        let confirming = self.confirm_delete.take();
        match message {
            Message::NameChanged(name) => self.name = name,
            Message::FilterChanged(filter) => self.filter = filter,
            Message::Save => self.requests.push(Request::Save(self.name.trim().to_string())),
            Message::Select(id) => {
                // Elegir uno deja su nombre listo para guardarlo encima.
                self.name = id.strip_prefix("base:").unwrap_or(&id).to_string();
                self.selected = Some(id);
            }
            Message::Load => {
                if let Some(id) = &self.selected {
                    self.requests.push(Request::Load(id.clone()));
                }
            }
            Message::Delete => match self.selected.clone() {
                Some(id) if confirming.as_deref() == Some(id.as_str()) => {
                    self.selected = None;
                    self.requests.push(Request::Delete(id));
                }
                Some(id) => self.confirm_delete = Some(id),
                None => {}
            },
            Message::Region(action) => self.requests.push(Request::Region(action)),
            Message::Deselect => self.requests.push(Request::Deselect),
            Message::Undo => self.requests.push(Request::Undo),
            Message::Place => self.requests.push(Request::Place),
            Message::PasteAir(on) => self.requests.push(Request::PasteAir(on)),
            Message::Close => self.requests.push(Request::Close),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let small = |label: &'static str| button(text(label).size(14)).padding([6, 10]);

        // --- Izquierda: prefabs guardados ---
        let entries: Vec<Element<'_, Message>> = self
            .shown_prefabs()
            .map(|prefab| {
                let [x, y, z] = prefab.size;
                let style = if self.selected.as_deref() == Some(prefab.id.as_str()) {
                    button::primary
                } else {
                    button::text
                };
                button(
                    row![
                        text(&prefab.id).size(14),
                        space().width(Length::Fill),
                        text(format!("{x}x{y}x{z}")).size(12)
                    ]
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .style(style)
                .on_press(Message::Select(prefab.id.clone()))
                .into()
            })
            .collect();
        let list: Element<'_, Message> = if entries.is_empty() {
            let hint = if self.prefabs.is_empty() {
                "No prefabs yet. Select a box with the wand and save it."
            } else {
                "No prefab matches."
            };
            text(hint).size(14).color(style::MUTED).into()
        } else {
            Column::with_children(entries).spacing(2).into()
        };
        let chosen = self.selected.is_some();
        let delete_label = if self.confirm_delete.is_some() { "Confirm delete" } else { "Delete" };
        let saved = column![
            text("Saved prefabs").size(20),
            text_input("Filter", &self.filter).size(14).on_input(Message::FilterChanged),
            scrollable(list).height(260),
            row![
                small("Load and place").on_press_maybe(chosen.then_some(Message::Load)),
                small(delete_label)
                    .style(button::danger)
                    .on_press_maybe(chosen.then_some(Message::Delete)),
            ]
            .spacing(8),
        ]
        .spacing(10)
        .width(340);

        // --- Derecha: selección y portapapeles ---
        let selected = self.has_selection;
        let region = |label, action| small(label).on_press_maybe(selected.then_some(Message::Region(action)));
        let tools = column![
            text("Selection").size(20),
            text(&self.selection).size(13),
            row![
                text_input("Name, e.g. trees/oak", &self.name)
                    .id(NAME_INPUT)
                    .size(14)
                    .on_input(Message::NameChanged)
                    .on_submit(Message::Save),
                small("Save").on_press_maybe(selected.then_some(Message::Save)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                region("Copy", RegionAction::Copy),
                region("Cut", RegionAction::Cut),
                region("Erase", RegionAction::Erase).style(button::danger),
                small("Deselect").style(button::secondary).on_press(Message::Deselect),
            ]
            .spacing(8),
            space().height(6),
            text("Clipboard").size(20),
            text(&self.clipboard).size(13),
            row![
                small("Place").on_press_maybe(self.has_clipboard.then_some(Message::Place)),
                small("Undo last change").style(button::secondary).on_press(Message::Undo),
            ]
            .spacing(8),
            row![
                checkbox(self.paste_air).on_toggle(Message::PasteAir),
                text("Empty space in the prefab also clears blocks").size(13)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(10)
        .width(400);

        let status: Element<'_, Message> = match &self.status {
            _ if !self.hosting => text("Only the host of the world can use the building tools.")
                .size(14)
                .color(style::ERROR)
                .into(),
            Some((line, ok)) => {
                text(line).size(14).color(if *ok { style::MUTED } else { style::ERROR }).into()
            }
            None => text("Hold the selection wand: left and right click mark the two corners.")
                .size(14)
                .color(style::MUTED)
                .into(),
        };

        let panel = column![
            text("Prefabs").size(30),
            row![saved, tools].spacing(28),
            status,
            row![space().width(Length::Fill), small("Close").style(button::secondary).on_press(Message::Close)],
        ]
        .spacing(14)
        .width(770);

        style::backdrop(style::panel(panel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Prefabs {
        let mut builder = Builder::default();
        builder.set_prefabs(vec![
            PrefabInfo { id: "base:house".into(), size: [9, 6, 7] },
            PrefabInfo { id: "base:trees/oak".into(), size: [5, 8, 5] },
        ]);
        let mut screen = Prefabs::default();
        screen.sync(&builder, true);
        screen
    }

    fn send(screen: &mut Prefabs, message: Message) -> Vec<Request> {
        screen.update(message);
        screen.take_requests()
    }

    #[test]
    fn saving_and_region_actions_are_forwarded() {
        let mut screen = screen();
        send(&mut screen, Message::NameChanged(" trees/pine ".into()));
        assert_eq!(send(&mut screen, Message::Save), [Request::Save("trees/pine".into())]);
        assert_eq!(send(&mut screen, Message::Region(RegionAction::Cut)), [Request::Region(RegionAction::Cut)]);
        assert_eq!(send(&mut screen, Message::PasteAir(true)), [Request::PasteAir(true)]);
        assert_eq!(send(&mut screen, Message::Place), [Request::Place]);
        assert_eq!(send(&mut screen, Message::Undo), [Request::Undo]);
        assert_eq!(send(&mut screen, Message::Close), [Request::Close]);
        let _ = screen.view();
    }

    #[test]
    fn the_list_filters_loads_and_deletes_with_two_clicks() {
        let mut screen = screen();
        assert!(send(&mut screen, Message::Load).is_empty(), "nothing chosen yet");
        send(&mut screen, Message::FilterChanged("oak".into()));
        assert_eq!(screen.shown_prefabs().count(), 1);

        send(&mut screen, Message::Select("base:trees/oak".into()));
        assert_eq!(screen.name, "trees/oak", "ready to save over it");
        assert_eq!(send(&mut screen, Message::Load), [Request::Load("base:trees/oak".into())]);

        assert!(send(&mut screen, Message::Delete).is_empty(), "the first click only asks");
        send(&mut screen, Message::FilterChanged(String::new()));
        assert!(send(&mut screen, Message::Delete).is_empty(), "anything in between cancels it");
        assert_eq!(send(&mut screen, Message::Delete), [Request::Delete("base:trees/oak".into())]);
        let _ = screen.view();

        // Cuando el servidor confirma el borrado, la lista nueva ya no lo trae.
        let mut builder = Builder::default();
        builder.set_prefabs(vec![PrefabInfo { id: "base:house".into(), size: [9, 6, 7] }]);
        screen.sync(&builder, false);
        assert_eq!(screen.selected, None);
        let _ = screen.view();
    }
}
