//! Barra de herramientas: nueve ranuras con un ítem cada una y una ranura activa.
//!
//! Es estado del juego, no del motor: el motor solo sabe "qué bloque coloca el clic
//! derecho" y se lo dice `game.rs` cada vez que cambia el ítem activo.

use kuravoxel_common::block::Block;
use kuravoxel_common::content::{ContentData, ItemId};

pub const SLOTS: usize = 9;

/// Busca un ítem por lo que escribe alguien en la consola: identificador completo
/// (`base:lamp`), solo el nombre (`lamp`) o el nombre de un estado de bloque
/// (`base:wood[axis=x]`, que da el ítem de ese bloque).
pub fn find_item(content: &ContentData, text: &str) -> Option<ItemId> {
    let text = text.trim();
    content
        .item_id(text)
        .or_else(|| {
            content
                .numbered_items()
                .find(|(_, item)| item.id.rsplit(':').next() == Some(text))
                .map(|(id, _)| id)
        })
        .or_else(|| content.block_id(text).and_then(|block| content.item_for_block(block)))
}

/// Las ranuras vacías guardan `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Hotbar {
    slots: [Option<ItemId>; SLOTS],
    active: usize,
}

impl Hotbar {
    pub fn slots(&self) -> &[Option<ItemId>; SLOTS] {
        &self.slots
    }

    pub fn active(&self) -> usize {
        self.active
    }

    /// Ítem de la ranura activa.
    pub fn active_item(&self) -> Option<ItemId> {
        self.slots[self.active]
    }

    /// Bloque que coloca el ítem activo (`Block::AIR` si no coloca ninguno).
    pub fn active_block(&self, content: &ContentData) -> Block {
        self.active_item()
            .and_then(|item| content.item(item))
            .and_then(|item| item.block)
            .unwrap_or(Block::AIR)
    }

    pub fn select(&mut self, slot: usize) {
        if slot < SLOTS {
            self.active = slot;
        }
    }

    /// Avanza o retrocede la ranura activa, dando la vuelta en los extremos.
    pub fn scroll(&mut self, step: i32) {
        self.active = (self.active as i32 + step).rem_euclid(SLOTS as i32) as usize;
    }

    pub fn assign(&mut self, slot: usize, item: Option<ItemId>) {
        if slot < SLOTS {
            self.slots[slot] = item;
        }
    }

    /// Copiar un bloque del mundo: si su ítem ya está en la barra se salta a su ranura;
    /// si no, sustituye al de la ranura activa.
    pub fn pick(&mut self, item: ItemId) {
        match self.slots.iter().position(|&slot| slot == Some(item)) {
            Some(slot) => self.active = slot,
            None => self.slots[self.active] = Some(item),
        }
    }

    /// Obtener un ítem (del inventario o con `give`): si ya está en la barra se salta a
    /// él; si no, va a la primera ranura vacía o, si no hay, a la activa. Esa ranura
    /// pasa a ser la activa. Devuelve cuál es.
    pub fn give(&mut self, item: ItemId) -> usize {
        let slot = self
            .slots
            .iter()
            .position(|&slot| slot == Some(item))
            .or_else(|| self.slots.iter().position(Option::is_none))
            .unwrap_or(self.active);
        self.slots[slot] = Some(item);
        self.active = slot;
        slot
    }

    /// Recibir un ítem recogido del suelo: si no está ya en la barra, va a la primera
    /// ranura libre. No cambia la ranura activa; con la barra llena no hace nada (es
    /// creativo: el ítem sigue en el inventario).
    pub fn receive(&mut self, item: ItemId) {
        if self.slots.contains(&Some(item)) {
            return;
        }
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(item);
        }
    }

    /// Ajusta la barra al contenido recibido: vacía las ranuras cuyo ítem ya no
    /// existe y, si la barra está vacía del todo (mundo recién abierto), la rellena con
    /// los primeros ítems que colocan un bloque.
    pub fn sync(&mut self, content: &ContentData) {
        for slot in &mut self.slots {
            if slot.is_some_and(|item| content.item(item).is_none()) {
                *slot = None;
            }
        }
        if self.slots.iter().all(Option::is_none) {
            let placeable = content.numbered_items().filter(|(_, item)| item.block.is_some());
            for (slot, (item, _)) in self.slots.iter_mut().zip(placeable) {
                *slot = Some(item);
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    pub use kuravoxel_common::content::test_content as content;

    #[test]
    fn an_empty_bar_is_filled_with_the_first_items() {
        let content = content(&["stone", "dirt", "grass"]);
        let mut bar = Hotbar::default();
        bar.sync(&content);
        assert_eq!(bar.slots()[..4], [Some(ItemId(0)), Some(ItemId(1)), Some(ItemId(2)), None]);
        assert_eq!(bar.active_block(&content), Block(1));
    }

    #[test]
    fn removed_items_leave_their_slot_empty_without_refilling() {
        let mut bar = Hotbar::default();
        bar.sync(&content(&["stone", "dirt", "grass"]));
        let mut reloaded = content(&["stone", "dirt", "grass"]);
        reloaded.items[1] = None;
        bar.sync(&reloaded);
        assert_eq!(bar.slots()[..3], [Some(ItemId(0)), None, Some(ItemId(2))]);
    }

    #[test]
    fn selecting_and_scrolling_wrap_around() {
        let mut bar = Hotbar::default();
        bar.scroll(-1);
        assert_eq!(bar.active(), SLOTS - 1);
        bar.scroll(1);
        assert_eq!(bar.active(), 0);
        bar.select(4);
        bar.select(SLOTS);
        assert_eq!(bar.active(), 4);
    }

    #[test]
    fn picking_jumps_to_a_known_item_or_replaces_the_active_slot() {
        let mut bar = Hotbar::default();
        bar.sync(&content(&["stone", "dirt", "grass", "sand"]));
        bar.assign(3, None);
        bar.pick(ItemId(2));
        assert_eq!(bar.active(), 2);
        bar.pick(ItemId(3));
        assert_eq!(bar.slots()[2], Some(ItemId(3)));
        assert_eq!(bar.active_item(), Some(ItemId(3)));
    }

    #[test]
    fn picked_up_items_fill_a_free_slot_without_moving_the_selection() {
        let mut bar = Hotbar::default();
        bar.sync(&content(&["stone", "dirt", "grass", "sand"]));
        bar.select(2);
        bar.receive(ItemId(0));
        assert_eq!(bar.slots().iter().filter(|s| **s == Some(ItemId(0))).count(), 1);
        bar.assign(3, None);
        bar.receive(ItemId(3));
        assert_eq!((bar.slots()[3], bar.active()), (Some(ItemId(3)), 2));
    }

    #[test]
    fn giving_prefers_an_empty_slot() {
        let mut bar = Hotbar::default();
        bar.sync(&content(&["stone", "dirt", "grass", "sand"]));
        // Ya está: se salta a él.
        assert_eq!(bar.give(ItemId(1)), 1);
        bar.assign(3, None);
        // No está: primera ranura vacía (la 3, que se acaba de vaciar).
        assert_eq!(bar.give(ItemId(3)), 3);
        assert_eq!(bar.active_item(), Some(ItemId(3)));
        // Barra llena: sustituye a la activa.
        for slot in 0..SLOTS {
            bar.assign(slot, Some(ItemId(0)));
        }
        bar.select(5);
        assert_eq!(bar.give(ItemId(2)), 5);
    }
}
