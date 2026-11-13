//! Inventario creativo (provisional): todos los ítems del contenido, por categorías y
//! con buscador.
//!
//! Clic izquierdo en un ítem: a la ranura activa de la barra. Clic derecho: a la primera
//! ranura libre. Clic en una ranura la hace activa; clic derecho la vacía. No hay
//! cantidades: es creativo.

use std::sync::Arc;

use kuravoxel_common::content::{ContentData, ItemDef, ItemId};
use kuravoxel_ui::core::{Alignment, Length};
use kuravoxel_ui::widget::{
    button, column, container, mouse_area, row, scrollable, text, text_input, tooltip, Row,
};
use kuravoxel_ui::{Element, Screen};

use super::style;
use crate::hotbar::{Hotbar, SLOTS};
use crate::icons::Icons;

/// Caja de búsqueda (para darle el foco al abrir la pestaña).
pub const SEARCH_INPUT: &str = "inventory-search";
/// Separación entre ranuras.
const GAP: f32 = 6.0;
/// Alto de la rejilla de ítems: unas cinco filas.
const GRID_HEIGHT: f32 = 5.0 * style::SLOT_SIZE + 4.0 * GAP;

/// Lo que el inventario pide al juego.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// La barra ha cambiado: esta es la nueva.
    Hotbar(Hotbar),
    /// Dar el foco a la caja de búsqueda.
    FocusSearch,
    Close,
}

/// Pestaña de ítems que se ve.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    All,
    Category(String),
    Search,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Poner este ítem en la ranura activa.
    Pick(ItemId),
    /// Poner este ítem en la primera ranura libre.
    Give(ItemId),
    /// Hacer activa esta ranura.
    SelectSlot(usize),
    /// Vaciar esta ranura.
    ClearSlot(usize),
    Tab(Tab),
    Search(String),
    Close,
}

#[derive(Default)]
pub struct Inventory {
    content: Arc<ContentData>,
    icons: Arc<Icons>,
    hotbar: Hotbar,
    tab: Tab,
    query: String,
    requests: Vec<Request>,
}

impl Inventory {
    /// Copia el estado del juego que la pantalla enseña.
    pub fn sync(&mut self, content: &Arc<ContentData>, icons: &Arc<Icons>, hotbar: &Hotbar) {
        self.content = content.clone();
        self.icons = icons.clone();
        self.hotbar = *hotbar;
        // Si la categoría elegida desaparece al recargar el contenido, se vuelve a "todo".
        if let Tab::Category(category) = &self.tab {
            if !self.content.categories().contains(&category.as_str()) {
                self.tab = Tab::All;
            }
        }
    }

    /// Peticiones pendientes para el juego, en orden.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// Se está escribiendo en el buscador: las letras y números no son atajos.
    pub fn typing(&self) -> bool {
        self.tab == Tab::Search
    }

    /// Ítems de la pestaña actual, en orden.
    pub fn visible(&self) -> Vec<(ItemId, &ItemDef)> {
        self.content
            .numbered_items()
            .filter(|(_, item)| match &self.tab {
                Tab::All => true,
                Tab::Category(category) => item.category == *category,
                Tab::Search => item.matches(&self.query),
            })
            .collect()
    }

    fn tab_button<'a>(&self, label: &'a str, tab: Tab) -> Element<'a, Message> {
        let style = if self.tab == tab { button::primary } else { button::secondary };
        button(text(label).size(13)).padding([4, 10]).style(style).on_press(Message::Tab(tab)).into()
    }
}

impl Screen for Inventory {
    type Message = Message;

    fn update(&mut self, message: Message) {
        // La pantalla cambia su copia (para verse al día en este mismo frame) y le pasa
        // al juego la barra resultante.
        match message {
            Message::Pick(item) => self.hotbar.assign(self.hotbar.active(), Some(item)),
            Message::Give(item) => {
                self.hotbar.give(item);
            }
            Message::SelectSlot(slot) => self.hotbar.select(slot),
            Message::ClearSlot(slot) => self.hotbar.assign(slot, None),
            Message::Tab(tab) => {
                if tab == Tab::Search {
                    self.requests.push(Request::FocusSearch);
                }
                self.tab = tab;
                return;
            }
            Message::Search(query) => {
                self.query = query;
                return;
            }
            Message::Close => {
                self.requests.push(Request::Close);
                return;
            }
        }
        self.requests.push(Request::Hotbar(self.hotbar));
    }

    fn view(&self) -> Element<'_, Message> {
        let mut tabs = vec![self.tab_button("All", Tab::All)];
        tabs.extend(
            self.content
                .categories()
                .into_iter()
                .map(|category| self.tab_button(category, Tab::Category(category.to_string()))),
        );
        tabs.push(self.tab_button("Search", Tab::Search));
        let tabs = Row::with_children(tabs).spacing(4).wrap().vertical_spacing(4);

        let visible = self.visible();
        let grid: Element<'_, Message> = if visible.is_empty() {
            text("No items here.").color(style::MUTED).into()
        } else {
            Row::with_children(visible.into_iter().map(|(id, item)| {
                let label = column![text(&item.name).size(14), text(&item.id).size(11).color(style::MUTED)];
                tooltip(
                    mouse_area(style::slot(Some(item), self.icons.get(id), false, None))
                        .on_press(Message::Pick(id))
                        .on_right_press(Message::Give(id)),
                    container(label).padding([4, 8]).style(container::rounded_box),
                    tooltip::Position::Top,
                )
                .into()
            }))
            .spacing(GAP)
            .wrap()
            .vertical_spacing(GAP)
            .into()
        };

        let hotbar = row(self.hotbar.slots().iter().enumerate().map(|(index, &item)| {
            mouse_area(style::slot(
                item.and_then(|item| self.content.item(item)),
                item.and_then(|item| self.icons.get(item)),
                index == self.hotbar.active(),
                Some((index + 1).to_string()),
            ))
            .on_press(Message::SelectSlot(index))
            .on_right_press(Message::ClearSlot(index))
            .into()
        }))
        .spacing(GAP);

        let selected = self
            .hotbar
            .active_item()
            .and_then(|item| self.content.item(item))
            .map_or("Empty slot", |item| item.name.as_str());
        let width = SLOTS as f32 * style::SLOT_SIZE + (SLOTS - 1) as f32 * GAP;

        let mut panel = column![
            text("Creative inventory").size(28),
            text("Left click: selected slot. Right click: first free slot.")
                .size(13)
                .color(style::MUTED),
            tabs,
        ]
        .spacing(12)
        .width(width);
        if self.tab == Tab::Search {
            panel = panel.push(
                text_input("Search by name, id or tag", &self.query)
                    .id(SEARCH_INPUT)
                    .on_input(Message::Search)
                    .padding(8),
            );
        }
        let panel = panel
            .push(container(scrollable(grid)).height(GRID_HEIGHT).width(Length::Fill))
            .push(text("Hotbar (right click empties a slot)").size(16))
            .push(hotbar)
            .push(
                row![
                    text(selected).width(Length::Fill),
                    button("Close").on_press(Message::Close),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );

        style::backdrop(style::panel(panel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotbar::tests::content;

    fn inventory() -> Inventory {
        let content = Arc::new(content(&["stone", "dirt", "grass"]));
        let mut hotbar = Hotbar::default();
        hotbar.sync(&content);
        let mut inventory = Inventory::default();
        inventory.sync(&content, &Arc::default(), &hotbar);
        inventory
    }

    fn last_hotbar(inventory: &mut Inventory) -> Hotbar {
        match inventory.take_requests().pop() {
            Some(Request::Hotbar(hotbar)) => hotbar,
            other => panic!("expected a hotbar, got {other:?}"),
        }
    }

    #[test]
    fn picking_fills_the_selected_slot() {
        let mut inventory = inventory();
        let _ = inventory.view();
        inventory.update(Message::SelectSlot(5));
        inventory.update(Message::Pick(ItemId(2)));
        let hotbar = last_hotbar(&mut inventory);
        assert_eq!((hotbar.active(), hotbar.active_item()), (5, Some(ItemId(2))));
        // Lo demás no se toca.
        assert_eq!(hotbar.slots()[..3], [Some(ItemId(0)), Some(ItemId(1)), Some(ItemId(2))]);
    }

    #[test]
    fn right_click_gives_to_a_free_slot_and_clears_slots() {
        let mut inventory = inventory();
        inventory.update(Message::ClearSlot(1));
        assert_eq!(last_hotbar(&mut inventory).slots()[1], None);
        inventory.update(Message::Give(ItemId(1)));
        let hotbar = last_hotbar(&mut inventory);
        assert_eq!((hotbar.active(), hotbar.active_item()), (1, Some(ItemId(1))));
        inventory.update(Message::Close);
        assert_eq!(inventory.take_requests(), [Request::Close]);
    }

    #[test]
    fn tabs_and_search_filter_the_items() {
        let mut inventory = inventory();
        assert_eq!(inventory.visible().len(), 3);
        inventory.update(Message::Tab(Tab::Search));
        assert!(inventory.typing());
        assert_eq!(inventory.take_requests(), [Request::FocusSearch]);
        inventory.update(Message::Search("GRA".into()));
        let found: Vec<_> = inventory.visible().into_iter().map(|(_, item)| item.id.clone()).collect();
        assert_eq!(found, ["base:grass"]);
        let _ = inventory.view();

        inventory.update(Message::Tab(Tab::Category("Nowhere".into())));
        assert!(inventory.visible().is_empty() && !inventory.typing());
        // Una categoría que no existe (p. ej. tras recargar el contenido) vuelve a "todo".
        let (content, icons, hotbar) = (inventory.content.clone(), inventory.icons.clone(), inventory.hotbar);
        inventory.sync(&content, &icons, &hotbar);
        assert_eq!(inventory.tab, Tab::All);
    }
}
