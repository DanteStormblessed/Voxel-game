//! El jugador local: su cuerpo, hacia dónde mira y desde dónde se le ve.
//!
//! La física es la de `kuravoxel_common::physics`; aquí se traduce la entrada a un
//! movimiento y se coloca la cámara: en los ojos (primera persona), orbitando detrás
//! (tercera persona) o muy por encima (vista de mapa).

use std::f32::consts::{PI, TAU};

use glam::{DVec3, IVec3, Vec3};
use kuravoxel_common::chunk::CHUNK_SIZE;
use kuravoxel_common::physics::{self, Body, MoveInput, MoveMode, PLAYER_WIDTH};
use kuravoxel_common::protocol::WorldBounds;

use crate::camera::{Camera, Input, MOUSE_SENSITIVITY};
use crate::world::World;

/// Distancias de la cámara en tercera persona, en metros.
pub const THIRD_PERSON_MIN: f32 = 2.0;
pub const THIRD_PERSON_MAX: f32 = 32.0;
/// Más allá de la tercera persona la cámara salta a la vista de mapa, entre estas dos
/// distancias. No hay nada intermedio: o se juega o se mira el mapa.
pub const MAP_MIN: f32 = 160.0;
pub const MAP_MAX: f32 = 3200.0;
/// Factor de cada paso de zoom (una muesca de la rueda).
const ZOOM_STEP: f32 = 1.25;
/// Hasta esta distancia el personaje mira hacia donde apunta la cámara (como en Hytale);
/// más lejos, la cámara orbita y el personaje gira hacia donde camina (como en Veloren).
const FACE_CAMERA_DISTANCE: f32 = 8.0;
/// En la vista de mapa la cámara siempre mira hacia abajo, al menos con esta inclinación.
const MAP_MAX_PITCH: f32 = -0.6;
const PITCH_LIMIT: f32 = 1.55;
/// Rapidez del zoom y del giro del personaje (1/s).
const ZOOM_SMOOTHING: f32 = 9.0;
const TURN_SMOOTHING: f32 = 14.0;
/// Separación entre la cámara y la pared que le corta el paso.
const CAMERA_MARGIN: f64 = 0.35;
/// En tercera persona la cámara mira un poco por encima de la cabeza, para que el
/// personaje no tape lo que se apunta.
const PIVOT_LIFT: f64 = 0.3;
/// A partir de esta distancia de cámara se dibuja al propio jugador.
const SELF_VISIBLE_DISTANCE: f32 = 0.9;

/// Desde dónde se ve la partida.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ViewKind {
    #[default]
    FirstPerson,
    ThirdPerson,
    /// Cámara muy alejada: solo para mirar el terreno. No se edita.
    Map,
}

pub struct Player {
    pub body: Body,
    pub mode: MoveMode,
    /// Hacia dónde mira la cámara. 0 mira a -Z; crece girando a la derecha.
    pub yaw: f32,
    pub pitch: f32,
    /// Hacia dónde mira el personaje (lo que ven los demás).
    pub body_yaw: f32,
    /// Radianes de giro por unidad de movimiento del ratón.
    pub sensitivity: f32,
    /// Zona jugable: el cuerpo no sale de ella (el servidor también lo comprueba).
    pub bounds: WorldBounds,
    /// Distancia de cámara pedida, la suavizada hacia ella y la que queda tras chocar
    /// con el terreno.
    zoom_target: f32,
    zoom: f32,
    distance: f32,
}

impl Player {
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            mode: MoveMode::Walk,
            yaw: 0.0,
            pitch: -0.3,
            body_yaw: 0.0,
            sensitivity: MOUSE_SENSITIVITY,
            bounds: WorldBounds::UNLIMITED,
            zoom_target: 0.0,
            zoom: 0.0,
            distance: 0.0,
        }
    }

    pub fn view(&self) -> ViewKind {
        match self.zoom_target {
            d if d <= 0.0 => ViewKind::FirstPerson,
            d if d <= THIRD_PERSON_MAX => ViewKind::ThirdPerson,
            _ => ViewKind::Map,
        }
    }

    /// Distancia de cámara pedida (0 = primera persona).
    pub fn camera_distance(&self) -> f32 {
        self.zoom_target
    }

    /// Distancia a la que está la cámara ahora mismo, tras suavizado y colisiones.
    pub fn shown_distance(&self) -> f32 {
        self.distance
    }

    /// Fija la distancia de cámara, ajustada a los tramos que existen.
    pub fn set_camera_distance(&mut self, meters: f32) {
        self.zoom_target = match meters {
            d if !(d > 0.0) => 0.0,
            d if d <= THIRD_PERSON_MAX => d.max(THIRD_PERSON_MIN),
            d => d.clamp(MAP_MIN, MAP_MAX),
        };
    }

    /// Aleja (`steps` > 0) o acerca la cámara ese número de pasos.
    pub fn zoom(&mut self, steps: i32) {
        for _ in 0..steps.unsigned_abs().min(64) {
            let d = self.zoom_target;
            self.zoom_target = if steps > 0 {
                match d {
                    d if d <= 0.0 => THIRD_PERSON_MIN,
                    d if d < THIRD_PERSON_MAX => (d * ZOOM_STEP).min(THIRD_PERSON_MAX),
                    d if d < MAP_MIN => MAP_MIN,
                    d => (d * ZOOM_STEP).min(MAP_MAX),
                }
            } else {
                match d {
                    d if d > MAP_MIN => (d / ZOOM_STEP).max(MAP_MIN),
                    d if d > THIRD_PERSON_MAX => THIRD_PERSON_MAX,
                    d if d > THIRD_PERSON_MIN => (d / ZOOM_STEP).max(THIRD_PERSON_MIN),
                    _ => 0.0,
                }
            };
        }
    }

    /// Coloca al jugador con los ojos en `eye` (al aparecer o al teletransportarlo).
    pub fn set_eye(&mut self, eye: DVec3) {
        self.body.set_eye(eye);
    }

    /// Chunk en el que está el jugador: el centro de lo que el servidor le envía.
    pub fn chunk_pos(&self) -> IVec3 {
        (self.body.pos / CHUNK_SIZE as f64).floor().as_ivec3()
    }

    pub fn forward(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
    }

    /// Punto alrededor del que orbita la cámara y desde el que sale el rayo de edición.
    /// Como la cámara mira justo a este punto, el punto de mira apunta a lo mismo en
    /// primera y en tercera persona.
    pub fn pivot(&self, world: &World) -> DVec3 {
        let eye = self.body.visual_eye();
        if self.distance <= 0.0 {
            return eye;
        }
        let lift = PIVOT_LIFT * (self.distance as f64 / THIRD_PERSON_MIN as f64).min(1.0);
        let lifted = eye + DVec3::new(0.0, lift, 0.0);
        // Bajo un techo no se levanta: el punto quedaría dentro del bloque de arriba.
        let above = (lifted + DVec3::new(0.0, 0.1, 0.0)).floor().as_ivec3();
        if world.block_at(above).is_some() && world.is_opaque(above) {
            eye
        } else {
            lifted
        }
    }

    /// Avanza un frame: mirada, movimiento con colisiones y distancia de cámara.
    pub fn update(&mut self, input: &mut Input, dt: f32, world: &World) {
        let view = self.view();
        self.yaw = (self.yaw + input.mouse_dx * self.sensitivity).rem_euclid(TAU);
        self.pitch = (self.pitch - input.mouse_dy * self.sensitivity).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        input.mouse_dx = 0.0;
        input.mouse_dy = 0.0;
        if view == ViewKind::Map && self.pitch > MAP_MAX_PITCH {
            // Al entrar en el mapa la cámara se inclina sola hasta mirar hacia abajo.
            self.pitch = (self.pitch - 2.5 * dt).max(MAP_MAX_PITCH);
        }

        // WASD es siempre relativo a la cámara.
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let axis = |positive: bool, negative: bool| (positive as i32 - negative as i32) as f32;
        let (along, across) = (axis(input.forward, input.back), axis(input.right, input.left));
        let wish = Vec3::new(
            sin_yaw * along + cos_yaw * across,
            0.0,
            -cos_yaw * along + sin_yaw * across,
        )
        .normalize_or_zero();
        let move_input = MoveInput {
            wish: [wish.x as f64, wish.z as f64],
            up: input.up,
            down: input.down,
            sprint: input.sprint,
        };

        // Sin el chunk de los pies no hay suelo que pisar: el cuerpo espera a que llegue.
        // Lo que aún no se ha recibido cuenta como sólido, para no caer por un borde.
        let feet = self.body.pos.floor().as_ivec3();
        if self.mode == MoveMode::Ghost || world.block_at(feet).is_some() {
            physics::step(&mut self.body, self.mode, &move_input, dt as f64, world);
        }
        // El borde de la zona jugable es una pared invisible.
        let inside = self.bounds.clamp(self.body.pos, PLAYER_WIDTH / 2.0);
        for axis in [0, 2] {
            if inside[axis] != self.body.pos[axis] {
                self.body.pos[axis] = inside[axis];
                self.body.vel[axis] = 0.0;
            }
        }

        // Cerca, el personaje mira con la cámara; lejos, hacia donde se mueve.
        let turn_to = if self.zoom_target <= FACE_CAMERA_DISTANCE {
            Some(self.yaw)
        } else {
            let (vx, vz) = (self.body.vel.x as f32, self.body.vel.z as f32);
            (vx * vx + vz * vz > 0.25).then(|| vx.atan2(-vz))
        };
        if let Some(target) = turn_to {
            let turn = (target - self.body_yaw + PI).rem_euclid(TAU) - PI;
            let blend = if self.zoom_target <= 0.0 { 1.0 } else { 1.0 - (-TURN_SMOOTHING * dt).exp() };
            self.body_yaw = (self.body_yaw + turn * blend).rem_euclid(TAU);
        }

        self.zoom += (self.zoom_target - self.zoom) * (1.0 - (-ZOOM_SMOOTHING * dt).exp());
        if (self.zoom_target - self.zoom).abs() < 0.01 {
            self.zoom = self.zoom_target;
        }
        // La cámara no atraviesa el terreno: si algo corta la línea hasta el jugador se
        // acerca de golpe, y cuando deja de estorbar se aleja con suavidad. La vista de
        // mapa y el espectador van por libre.
        let mut allowed = self.zoom;
        if self.zoom > 0.0 && view != ViewKind::Map && self.mode != MoveMode::Ghost {
            self.distance = self.distance.min(self.zoom);
            let pivot = self.pivot(world);
            if let Some(hit) = world.raycast_distance(pivot, -self.forward(), self.zoom as f64 + CAMERA_MARGIN) {
                allowed = allowed.min((hit - CAMERA_MARGIN).max(0.0) as f32);
            }
        }
        self.distance = if allowed <= self.distance {
            allowed
        } else {
            let eased = self.distance + (allowed - self.distance) * (1.0 - (-ZOOM_SMOOTHING * dt).exp());
            if allowed - eased < 0.01 { allowed } else { eased }
        };
    }

    /// Coloca la cámara de dibujo según la vista actual.
    pub fn place_camera(&self, camera: &mut Camera, world: &World) {
        camera.pos = self.pivot(world) - self.forward().as_dvec3() * self.distance as f64;
        camera.yaw = self.yaw;
        camera.pitch = self.pitch;
    }

    /// El propio jugador se ve (la cámara está fuera de su cabeza).
    pub fn draws_self(&self) -> bool {
        self.distance > SELF_VISIBLE_DISTANCE
    }
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kuravoxel_common::block::Block;
    use kuravoxel_common::chunk::Chunk;

    use super::*;

    /// Mundo de 3×3×3 chunks con suelo macizo por debajo de y = 0.
    fn flat_world() -> World {
        let mut world = World::new();
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    let block = if y < 0 { Block(1) } else { Block::AIR };
                    world.insert_chunk(IVec3::new(x, y, z), Arc::new(Chunk::Uniform(block)));
                }
            }
        }
        world
    }

    fn simulate(player: &mut Player, input: &mut Input, world: &World, seconds: f32) {
        for _ in 0..(seconds * 60.0) as u32 {
            player.update(input, 1.0 / 60.0, world);
        }
    }

    #[test]
    fn zoom_goes_from_first_person_to_the_map_and_back() {
        let mut player = Player::new();
        assert_eq!(player.view(), ViewKind::FirstPerson);
        player.zoom(1);
        assert_eq!((player.view(), player.camera_distance()), (ViewKind::ThirdPerson, THIRD_PERSON_MIN));
        player.zoom(40);
        assert_eq!((player.view(), player.camera_distance()), (ViewKind::Map, MAP_MAX));

        // De vuelta: el mapa, luego el tope de la tercera persona, y al final los ojos.
        let mut seen = Vec::new();
        while player.camera_distance() > 0.0 {
            player.zoom(-1);
            seen.push(player.camera_distance());
        }
        assert!(seen.contains(&MAP_MIN) && seen.contains(&THIRD_PERSON_MAX));
        assert!(seen.iter().all(|&d| d <= THIRD_PERSON_MAX || d >= MAP_MIN), "{seen:?}");
        assert!(seen.windows(2).all(|pair| pair[1] < pair[0]));

        player.set_camera_distance(50.0);
        assert_eq!(player.camera_distance(), MAP_MIN);
        player.set_camera_distance(0.5);
        assert_eq!(player.camera_distance(), THIRD_PERSON_MIN);
        player.set_camera_distance(-3.0);
        assert_eq!(player.view(), ViewKind::FirstPerson);
    }

    #[test]
    fn the_player_falls_walks_and_is_followed_by_the_camera() {
        let world = flat_world();
        let mut player = Player::new();
        player.set_eye(DVec3::new(0.5, 6.0, 0.5));
        let mut input = Input::default();
        simulate(&mut player, &mut input, &world, 2.0);
        assert!(player.body.on_ground && player.body.pos.y.abs() < 1e-9);

        // Con yaw 0 "adelante" es -Z.
        input.forward = true;
        simulate(&mut player, &mut input, &world, 1.0);
        assert!(player.body.pos.z < -4.0 && player.body.pos.x == 0.5);

        let mut camera = Camera::new(DVec3::ZERO);
        player.place_camera(&mut camera, &world);
        assert_eq!(camera.pos, player.body.eye(), "first person looks from the eyes");
        assert!(!player.draws_self());
    }

    #[test]
    fn nothing_moves_until_the_ground_has_arrived() {
        let world = World::new();
        let mut player = Player::new();
        player.set_eye(DVec3::new(0.5, 6.0, 0.5));
        simulate(&mut player, &mut Input::default(), &world, 1.0);
        assert_eq!(player.body.eye(), DVec3::new(0.5, 6.0, 0.5));
    }

    #[test]
    fn the_third_person_camera_stays_out_of_the_ground() {
        let world = flat_world();
        let mut player = Player::new();
        player.set_eye(DVec3::new(0.5, physics::EYE_HEIGHT, 0.5));
        player.set_camera_distance(12.0);
        let mut input = Input::default();

        // Mirando hacia abajo la cámara queda arriba, a la distancia pedida.
        player.pitch = -0.8;
        simulate(&mut player, &mut input, &world, 2.0);
        assert!((player.shown_distance() - 12.0).abs() < 1e-3);
        assert!(player.draws_self());

        // Mirando hacia arriba la cámara iría bajo tierra: se queda pegada al suelo.
        player.pitch = 0.8;
        simulate(&mut player, &mut input, &world, 0.1);
        let mut camera = Camera::new(DVec3::ZERO);
        player.place_camera(&mut camera, &world);
        assert!(player.shown_distance() < 3.0, "distance {}", player.shown_distance());
        assert!(camera.pos.y > 0.0, "camera at {}", camera.pos.y);
    }

    #[test]
    fn the_character_faces_the_camera_up_close_and_its_path_from_afar() {
        let world = flat_world();
        let mut player = Player::new();
        player.set_eye(DVec3::new(0.5, physics::EYE_HEIGHT, 0.5));
        let mut input = Input { right: true, ..Input::default() };

        // Cerca: aunque camine de lado, mira hacia donde mira la cámara.
        player.set_camera_distance(4.0);
        simulate(&mut player, &mut input, &world, 0.5);
        assert!(player.body_yaw.abs() < 1e-3 || (player.body_yaw - TAU).abs() < 1e-3);

        // Lejos: gira hacia donde camina (+X es un cuarto de vuelta a la derecha).
        player.set_camera_distance(20.0);
        simulate(&mut player, &mut input, &world, 0.5);
        assert!((player.body_yaw - PI / 2.0).abs() < 0.02, "yaw {}", player.body_yaw);
    }

    #[test]
    fn the_player_cannot_leave_the_world() {
        let world = flat_world();
        let mut player = Player::new();
        player.bounds = WorldBounds { min: [-4.0, -4.0], max: [4.0, 4.0] };
        player.set_eye(DVec3::new(0.5, physics::EYE_HEIGHT, 0.5));
        let mut input = Input { forward: true, right: true, ..Input::default() };
        simulate(&mut player, &mut input, &world, 3.0);
        let half = PLAYER_WIDTH / 2.0;
        assert_eq!((player.body.pos.x, player.body.pos.z), (4.0 - half, -4.0 + half));
    }

    #[test]
    fn the_map_view_looks_down_and_ignores_the_terrain() {
        let world = flat_world();
        let mut player = Player::new();
        player.set_eye(DVec3::new(0.5, physics::EYE_HEIGHT, 0.5));
        player.pitch = 0.5;
        player.set_camera_distance(MAP_MIN);
        simulate(&mut player, &mut Input::default(), &world, 3.0);
        assert_eq!(player.view(), ViewKind::Map);
        assert_eq!(player.pitch, MAP_MAX_PITCH);
        assert!((player.shown_distance() - MAP_MIN).abs() < 1e-3);
    }
}
