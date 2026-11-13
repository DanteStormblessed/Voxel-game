//! Ítems sueltos en el mundo: los que un jugador tira con `Q`.
//!
//! El servidor los simula (caen y se posan con la misma física que el jugador, como
//! cubitos de 25 cm), decide quién los recoge y los retira al caducar. Los clientes solo
//! los dibujan flotando y girando.

use std::time::Duration;

use glam::DVec3;
use kuravoxel_common::content::ItemId;
use kuravoxel_common::physics::{self, Collider, Loose, ITEM_BOX, PLAYER_HEIGHT, PLAYER_WIDTH};
use kuravoxel_common::protocol::{DropId, PlayerId};
use rustc_hash::FxHashMap;

/// Tiempo antes de que nadie pueda recoger un ítem recién tirado (así no vuelve a quien
/// lo tira en el mismo instante).
pub const PICKUP_DELAY: Duration = Duration::from_millis(1500);
/// Un ítem suelto desaparece a los 5 minutos.
pub const LIFETIME: Duration = Duration::from_secs(300);
/// Distancia extra alrededor de la caja del jugador a la que se recoge.
const PICKUP_REACH: f64 = 0.6;
/// Velocidad con la que sale un ítem tirado (m/s), y un poco hacia arriba.
const THROW_SPEED: f64 = 6.0;
const THROW_LIFT: f64 = 2.0;
/// Ítems sueltos como mucho a la vez en el mundo; al pasar, los más viejos desaparecen.
pub const MAX_DROPS: usize = 512;

pub struct Drop {
    pub item: ItemId,
    pub body: Loose,
    pub age: Duration,
}

#[derive(Default)]
pub struct Drops {
    next_id: DropId,
    drops: FxHashMap<DropId, Drop>,
}

impl Drops {
    pub fn iter(&self) -> impl Iterator<Item = (DropId, &Drop)> {
        self.drops.iter().map(|(&id, drop)| (id, drop))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.drops.len()
    }

    /// Tira un ítem desde los ojos `eye` en la dirección `dir`. Devuelve su id y, si se
    /// ha superado el máximo, el del más viejo, que desaparece.
    pub fn throw(&mut self, item: ItemId, eye: DVec3, dir: DVec3) -> (DropId, Option<DropId>) {
        let dir = dir.normalize_or_zero();
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let body = Loose {
            // Algo por delante y por debajo de los ojos, como si saliera de la mano.
            pos: eye + dir * 0.3 - DVec3::new(0.0, 0.4, 0.0),
            vel: dir * THROW_SPEED + DVec3::new(0.0, THROW_LIFT, 0.0),
            on_ground: false,
        };
        self.drops.insert(id, Drop { item, body, age: Duration::ZERO });
        let evicted = (self.drops.len() > MAX_DROPS)
            .then(|| self.drops.iter().max_by_key(|(_, d)| d.age).map(|(&id, _)| id))
            .flatten();
        if let Some(old) = evicted {
            self.drops.remove(&old);
        }
        (id, evicted)
    }

    /// Avanza `dt`. Devuelve los que se han movido (con su posición nueva) y los que han
    /// caducado.
    pub fn step(&mut self, dt: Duration, world: &impl Collider) -> (Vec<(DropId, DVec3)>, Vec<DropId>) {
        let mut moved = Vec::new();
        let mut expired = Vec::new();
        for (&id, drop) in &mut self.drops {
            drop.age += dt;
            if drop.age >= LIFETIME {
                expired.push(id);
                continue;
            }
            let before = drop.body.pos;
            physics::step_loose(&mut drop.body, ITEM_BOX, dt.as_secs_f64(), world);
            if drop.body.pos.distance_squared(before) > 1e-10 {
                moved.push((id, drop.body.pos));
            }
        }
        for id in &expired {
            self.drops.remove(id);
        }
        (moved, expired)
    }

    /// Ítems que recoge cada jugador (por la posición de sus pies). Los quita del mundo.
    pub fn pick_up(&mut self, players: &[(PlayerId, DVec3)]) -> Vec<(DropId, ItemId, PlayerId)> {
        let mut picked = Vec::new();
        for (&id, drop) in &self.drops {
            if drop.age < PICKUP_DELAY {
                continue;
            }
            let center = drop.body.pos + DVec3::new(0.0, ITEM_BOX.height / 2.0, 0.0);
            let reach = PLAYER_WIDTH / 2.0 + PICKUP_REACH;
            let taker = players.iter().find(|(_, feet)| {
                (center.x - feet.x).abs() <= reach
                    && (center.z - feet.z).abs() <= reach
                    && center.y >= feet.y - PICKUP_REACH
                    && center.y <= feet.y + PLAYER_HEIGHT + PICKUP_REACH
            });
            if let Some(&(player, _)) = taker {
                picked.push((id, drop.item, player));
            }
        }
        for (id, _, _) in &picked {
            self.drops.remove(id);
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;

    fn ground(voxel: IVec3) -> bool {
        voxel.y < 0
    }

    #[test]
    fn thrown_items_land_and_are_picked_up_after_a_moment() {
        let mut drops = Drops::default();
        let (id, _) = drops.throw(ItemId(3), DVec3::new(0.5, 1.62, 0.5), DVec3::new(0.0, 0.0, -1.0));
        let tick = Duration::from_millis(50);
        let mut moved_once = false;
        for _ in 0..40 {
            let (moved, expired) = drops.step(tick, &ground);
            moved_once |= moved.iter().any(|(m, _)| *m == id);
            assert!(expired.is_empty());
        }
        let drop = drops.iter().next().unwrap().1;
        assert!(moved_once && drop.body.on_ground && drop.body.pos.y.abs() < 1e-9);
        assert!(drop.body.pos.z < -0.5, "it flew forward: {:?}", drop.body.pos);
        // En reposo ya no se mueve.
        assert!(drops.step(tick, &ground).0.is_empty());

        // Un jugador lejos no lo recoge; uno encima, sí.
        let far = [(1, DVec3::new(10.0, 0.0, 10.0))];
        assert!(drops.pick_up(&far).is_empty());
        let pos = drops.iter().next().unwrap().1.body.pos;
        assert_eq!(drops.pick_up(&[(2, pos)]), vec![(id, ItemId(3), 2)]);
        assert_eq!(drops.len(), 0);
    }

    #[test]
    fn fresh_items_wait_and_old_ones_expire() {
        let mut drops = Drops::default();
        drops.throw(ItemId(1), DVec3::new(0.5, 1.0, 0.5), DVec3::Y);
        let pos = drops.iter().next().unwrap().1.body.pos;
        assert!(drops.pick_up(&[(1, pos)]).is_empty(), "too soon to pick it up");
        let (_, expired) = drops.step(LIFETIME, &ground);
        assert_eq!(expired.len(), 1);
        assert_eq!(drops.len(), 0);
    }
}
