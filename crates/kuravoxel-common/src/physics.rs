//! Física del jugador: una caja que se mueve por un mundo de voxeles.
//!
//! Vive en `common` para que cliente y servidor compartan exactamente el mismo código:
//! hoy la ejecuta el cliente (el servidor solo comprueba la velocidad), y el día que el
//! servidor quiera re-simular el movimiento lo hará con esta misma función.
//!
//! No sabe de chunks ni de red: el mundo es un `Collider`, que dice con qué forma choca
//! cada voxel (nada, el cubo entero o unas cajas en mini-voxeles: losas, escaleras). Una
//! función `Fn(IVec3) -> bool` también sirve (cubos enteros).

use glam::{DVec3, IVec3};
use serde::{Deserialize, Serialize};

use crate::model::{CellBox, CELL};
use crate::shape::Collision;

/// Forma con la que choca un voxel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape<'a> {
    Empty,
    Full,
    /// Cajas en mini-voxeles (octavos de bloque) dentro del voxel.
    Boxes(&'a [CellBox]),
}

impl Collision {
    pub fn shape(&self) -> Shape<'_> {
        match self {
            Collision::Empty => Shape::Empty,
            Collision::Full => Shape::Full,
            Collision::Boxes(boxes) => Shape::Boxes(boxes),
        }
    }
}

/// El mundo tal como lo ve la física.
pub trait Collider {
    fn shape(&self, voxel: IVec3) -> Shape<'_>;
}

impl<F: Fn(IVec3) -> bool> Collider for F {
    fn shape(&self, voxel: IVec3) -> Shape<'_> {
        if self(voxel) { Shape::Full } else { Shape::Empty }
    }
}

const FULL_BOX: [CellBox; 1] = [[0, 0, 0, CELL as u8, CELL as u8, CELL as u8]];

/// Cajas del voxel en coordenadas del mundo, como (mínimo, máximo).
fn voxel_boxes(world: &impl Collider, voxel: IVec3, mut visit: impl FnMut(DVec3, DVec3) -> bool) -> bool {
    let boxes: &[CellBox] = match world.shape(voxel) {
        Shape::Empty => return false,
        Shape::Full => &FULL_BOX,
        Shape::Boxes(boxes) => boxes,
    };
    let origin = voxel.as_dvec3();
    let eighth = 1.0 / CELL as f64;
    boxes.iter().any(|b| {
        let min = origin + DVec3::new(b[0] as f64, b[1] as f64, b[2] as f64) * eighth;
        let max = origin + DVec3::new(b[3] as f64, b[4] as f64, b[5] as f64) * eighth;
        visit(min, max)
    })
}

/// Ancho (y fondo) de la caja del jugador, en metros.
pub const PLAYER_WIDTH: f64 = 0.6;
pub const PLAYER_HEIGHT: f64 = 1.8;
/// Altura de los ojos sobre los pies.
pub const EYE_HEIGHT: f64 = 1.62;

/// Escalón más alto que se sube andando, sin saltar.
pub const STEP_HEIGHT: f64 = 1.0;

/// Velocidades a pie, en metros por segundo. Minecraft anda a 4,3 y corre a 5,6; Veloren
/// ronda los 9-10. Esto queda entre medias, más cerca de Hytale.
pub const WALK_SPEED: f64 = 5.6;
pub const SPRINT_SPEED: f64 = 8.2;
/// Altura del salto: algo más de un bloque, para subir uno con holgura.
pub const JUMP_HEIGHT: f64 = 1.3;
pub const GRAVITY: f64 = 30.0;
const TERMINAL_SPEED: f64 = 70.0;
/// Aceleraciones horizontales (m/s²). En el suelo la respuesta es casi inmediata: es lo
/// que hace que el personaje se sienta ágil. En el aire se conserva la inercia.
const GROUND_ACCEL: f64 = 75.0;
const GROUND_FRICTION: f64 = 60.0;
const AIR_ACCEL: f64 = 22.0;
const AIR_FRICTION: f64 = 4.0;

/// Vuelo (con o sin colisiones).
pub const FLY_SPEED: f64 = 12.0;
pub const FLY_SPRINT_SPEED: f64 = 32.0;
pub const GHOST_SPEED: f64 = 14.0;
pub const GHOST_SPRINT_SPEED: f64 = 112.0;
const FLY_ACCEL: f64 = 90.0;

/// Rapidez con la que la vista alcanza al cuerpo tras subir un escalón (1/s).
const STEP_SMOOTHING: f64 = 16.0;
/// Velocidad a la que un cuerpo atrapado dentro de bloques sale hacia arriba.
const UNSTUCK_SPEED: f64 = 12.0;
/// Paso de simulación más largo; un frame lento se parte en varios.
const MAX_STEP: f64 = 1.0 / 120.0;
/// Tolerancia de las comparaciones con los planos de la rejilla.
const EPSILON: f64 = 1e-7;

/// Cómo se mueve el cuerpo. El motor no les pone nombre de modo de juego: eso lo hace
/// el juego (en Tokikura: supervivencia, creativo y espectador).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum MoveMode {
    /// A pie: gravedad, salto, colisiones y subida de escalones.
    #[default]
    Walk,
    /// Vuelo con colisiones.
    Fly,
    /// Vuelo sin colisiones: atraviesa los bloques.
    Ghost,
}

/// Lo que el jugador quiere hacer en un paso.
#[derive(Clone, Copy, Debug, Default)]
pub struct MoveInput {
    /// Dirección deseada en el plano horizontal (x, z) del mundo; longitud ≤ 1.
    pub wish: [f64; 2],
    /// Saltar (a pie) o subir (volando).
    pub up: bool,
    /// Bajar (solo volando).
    pub down: bool,
    pub sprint: bool,
}

/// El cuerpo del jugador.
#[derive(Clone, Copy, Debug, Default)]
pub struct Body {
    /// Centro de la base de la caja (los pies).
    pub pos: DVec3,
    pub vel: DVec3,
    pub on_ground: bool,
    /// Desfase vertical solo visual (≤ 0): al subir un escalón el cuerpo sube de golpe y
    /// la vista le sigue suavemente.
    pub step_offset: f64,
}

impl Body {
    pub fn new(pos: DVec3) -> Self {
        Self { pos, ..Self::default() }
    }

    /// Posición de los ojos (la que se envía al servidor).
    pub fn eye(&self) -> DVec3 {
        self.pos + DVec3::new(0.0, EYE_HEIGHT, 0.0)
    }

    /// Posición de los ojos para dibujar, con el suavizado de los escalones.
    pub fn visual_eye(&self) -> DVec3 {
        self.pos + DVec3::new(0.0, EYE_HEIGHT + self.step_offset, 0.0)
    }

    pub fn set_eye(&mut self, eye: DVec3) {
        self.pos = eye - DVec3::new(0.0, EYE_HEIGHT, 0.0);
        self.vel = DVec3::ZERO;
        self.step_offset = 0.0;
        self.on_ground = false;
    }

    /// Esquinas mínima y máxima de la caja.
    pub fn aabb(&self) -> (DVec3, DVec3) {
        aabb(self.pos)
    }

    /// La caja del jugador toca el voxel `voxel`.
    pub fn touches(&self, voxel: IVec3) -> bool {
        let (min, max) = self.aabb();
        let (lo, hi) = (voxel.as_dvec3(), voxel.as_dvec3() + DVec3::ONE);
        (0..3).all(|axis| min[axis] < hi[axis] - EPSILON && max[axis] > lo[axis] + EPSILON)
    }
}

/// Ancho y alto de una caja que se mueve (centrada en X/Z, apoyada en su base).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxSize {
    pub width: f64,
    pub height: f64,
}

pub const PLAYER_BOX: BoxSize = BoxSize { width: PLAYER_WIDTH, height: PLAYER_HEIGHT };
/// Un ítem suelto choca como un cubito de 25 cm.
pub const ITEM_BOX: BoxSize = BoxSize { width: 0.25, height: 0.25 };

fn aabb(pos: DVec3) -> (DVec3, DVec3) {
    sized_aabb(pos, PLAYER_BOX)
}

fn sized_aabb(pos: DVec3, size: BoxSize) -> (DVec3, DVec3) {
    let half = size.width / 2.0;
    (
        DVec3::new(pos.x - half, pos.y, pos.z - half),
        DVec3::new(pos.x + half, pos.y + size.height, pos.z + half),
    )
}

/// Voxeles que ocupa un intervalo `[min, max]` en un eje.
fn span(min: f64, max: f64) -> std::ops::RangeInclusive<i32> {
    (min + EPSILON).floor() as i32..=(max - EPSILON).floor() as i32
}

/// Las cajas se solapan de verdad (no basta con tocarse) en el eje `axis`.
fn overlap_1d(min_a: f64, max_a: f64, min_b: f64, max_b: f64) -> bool {
    min_a < max_b - EPSILON && max_a > min_b + EPSILON
}

/// La caja en `pos` se solapa con algún bloque sólido.
fn overlaps(pos: DVec3, world: &impl Collider) -> bool {
    overlaps_sized(pos, PLAYER_BOX, world)
}

fn overlaps_sized(pos: DVec3, size: BoxSize, world: &impl Collider) -> bool {
    let (min, max) = sized_aabb(pos, size);
    span(min.z, max.z).any(|z| {
        span(min.y, max.y).any(|y| {
            span(min.x, max.x).any(|x| {
                voxel_boxes(world, IVec3::new(x, y, z), |lo, hi| {
                    (0..3).all(|axis| overlap_1d(min[axis], max[axis], lo[axis], hi[axis]))
                })
            })
        })
    })
}

/// Mueve la caja `delta` metros a lo largo de un eje hasta donde dejen los bloques.
/// Devuelve `true` si chocó. Solo cuentan las cajas que están por delante del cuerpo,
/// así que un cuerpo que ya está dentro de un bloque puede salir de él.
fn move_axis(pos: &mut DVec3, axis: usize, delta: f64, world: &impl Collider) -> bool {
    move_axis_sized(pos, PLAYER_BOX, axis, delta, world)
}

fn move_axis_sized(pos: &mut DVec3, size: BoxSize, axis: usize, delta: f64, world: &impl Collider) -> bool {
    if delta == 0.0 {
        return false;
    }
    let (min, max) = sized_aabb(*pos, size);
    let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
    // Capas de voxeles que barre el movimiento, incluida la que ya se pisa (una losa
    // puede estar en la misma capa que los pies).
    let (lead, layers) = if delta > 0.0 {
        let lead = max[axis];
        (lead, (lead - EPSILON).floor() as i32..=(lead + delta - EPSILON).floor() as i32)
    } else {
        let lead = min[axis];
        (lead, (lead + delta + EPSILON).floor() as i32..=(lead + EPSILON).floor() as i32)
    };
    let mut allowed = delta;
    let mut hit = false;
    for layer in layers {
        for i in span(min[a], max[a]) {
            for j in span(min[b], max[b]) {
                let mut voxel = IVec3::ZERO;
                voxel[axis] = layer;
                voxel[a] = i;
                voxel[b] = j;
                voxel_boxes(world, voxel, |lo, hi| {
                    if !overlap_1d(min[a], max[a], lo[a], hi[a]) || !overlap_1d(min[b], max[b], lo[b], hi[b]) {
                        return false;
                    }
                    if delta > 0.0 && lo[axis] >= lead - EPSILON {
                        let gap = (lo[axis] - lead).max(0.0);
                        if gap < allowed {
                            allowed = gap;
                            hit = true;
                        }
                    } else if delta < 0.0 && hi[axis] <= lead + EPSILON {
                        let gap = (hi[axis] - lead).min(0.0);
                        if gap > allowed {
                            allowed = gap;
                            hit = true;
                        }
                    }
                    false
                });
            }
        }
    }
    pos[axis] += allowed;
    hit
}

/// Acerca `current` a `target` como mucho `max_delta` (en el plano o en el espacio).
fn approach(current: DVec3, target: DVec3, max_delta: f64) -> DVec3 {
    let diff = target - current;
    let distance = diff.length();
    if distance <= max_delta || distance == 0.0 {
        target
    } else {
        current + diff / distance * max_delta
    }
}

/// Avanza el cuerpo `dt` segundos contra el mundo `world`.
pub fn step(
    body: &mut Body,
    mode: MoveMode,
    input: &MoveInput,
    dt: f64,
    world: &impl Collider,
) {
    let steps = (dt / MAX_STEP).ceil().max(1.0);
    let sub = dt / steps;
    for _ in 0..steps as u32 {
        match mode {
            MoveMode::Walk => step_walk(body, input, sub, world),
            MoveMode::Fly | MoveMode::Ghost => step_fly(body, mode, input, sub, world),
        }
    }
}

fn wish_vector(input: &MoveInput) -> DVec3 {
    let wish = DVec3::new(input.wish[0], 0.0, input.wish[1]);
    if wish.length_squared() > 1.0 {
        wish.normalize()
    } else {
        wish
    }
}

fn step_walk(body: &mut Body, input: &MoveInput, dt: f64, world: &impl Collider) {
    body.step_offset *= (-STEP_SMOOTHING * dt).exp();

    // Atrapado dentro de bloques (apareció ahí, o le pusieron uno encima): sale hacia arriba.
    if overlaps(body.pos, world) {
        body.pos.y += UNSTUCK_SPEED * dt;
        body.vel = DVec3::ZERO;
        body.on_ground = false;
        return;
    }

    // Horizontal: la velocidad persigue a la deseada.
    let wish = wish_vector(input);
    let speed = if input.sprint { SPRINT_SPEED } else { WALK_SPEED };
    let moving = wish != DVec3::ZERO;
    let accel = match (body.on_ground, moving) {
        (true, true) => GROUND_ACCEL,
        (true, false) => GROUND_FRICTION,
        (false, true) => AIR_ACCEL,
        (false, false) => AIR_FRICTION,
    };
    let horizontal = DVec3::new(body.vel.x, 0.0, body.vel.z);
    let horizontal = approach(horizontal, wish * speed, accel * dt);
    body.vel.x = horizontal.x;
    body.vel.z = horizontal.z;

    // Vertical: salto y gravedad. Mantener el salto pulsado encadena saltos.
    if input.up && body.on_ground {
        body.vel.y = (2.0 * GRAVITY * JUMP_HEIGHT).sqrt();
        body.on_ground = false;
    }
    let fall = body.vel.y * dt - 0.5 * GRAVITY * dt * dt;
    body.vel.y = (body.vel.y - GRAVITY * dt).max(-TERMINAL_SPEED);

    let (dx, dz) = (body.vel.x * dt, body.vel.z * dt);
    let start = body.pos;
    let mut walked = start;
    let hit_x = move_axis(&mut walked, 0, dx, world);
    let hit_z = move_axis(&mut walked, 2, dz, world);

    // Escalón: si un bloque corta el paso y se está en el suelo, se prueba el mismo
    // movimiento desde un bloque más arriba. Solo vale si así se avanza más.
    let mut stepped = false;
    if (hit_x || hit_z) && body.on_ground && body.vel.y <= 0.0 {
        let mut raised = start;
        move_axis(&mut raised, 1, STEP_HEIGHT, world);
        let rise = raised.y - start.y;
        move_axis(&mut raised, 0, dx, world);
        move_axis(&mut raised, 2, dz, world);
        move_axis(&mut raised, 1, -rise, world);
        let progress = |pos: DVec3| (pos.x - start.x).powi(2) + (pos.z - start.z).powi(2);
        if progress(raised) > progress(walked) + 1e-12 {
            body.step_offset -= raised.y - start.y;
            walked = raised;
            stepped = true;
        }
    }
    // Sin escalón, chocar frena; con escalón, la carrera no se interrumpe.
    if !stepped {
        if hit_x {
            body.vel.x = 0.0;
        }
        if hit_z {
            body.vel.z = 0.0;
        }
    }
    body.pos = walked;

    let hit_y = move_axis(&mut body.pos, 1, fall, world);
    body.on_ground = hit_y && fall < 0.0;
    if hit_y {
        body.vel.y = 0.0;
    }
}

fn step_fly(
    body: &mut Body,
    mode: MoveMode,
    input: &MoveInput,
    dt: f64,
    world: &impl Collider,
) {
    body.step_offset = 0.0;
    body.on_ground = false;
    let ghost = mode == MoveMode::Ghost;
    let speed = match (ghost, input.sprint) {
        (false, false) => FLY_SPEED,
        (false, true) => FLY_SPRINT_SPEED,
        (true, false) => GHOST_SPEED,
        (true, true) => GHOST_SPRINT_SPEED,
    };
    let mut target = wish_vector(input) * speed;
    target.y = (input.up as i32 - input.down as i32) as f64 * speed;
    // El espectador responde al instante (es una cámara); el vuelo tiene algo de inercia.
    body.vel = if ghost { target } else { approach(body.vel, target, FLY_ACCEL * dt) };

    let delta = body.vel * dt;
    // Volando con colisiones, quien está dentro de un bloque puede salir de él igualmente.
    if ghost || overlaps(body.pos, world) {
        body.pos += delta;
        return;
    }
    for axis in [1, 0, 2] {
        if move_axis(&mut body.pos, axis, delta[axis], world) {
            body.vel[axis] = 0.0;
        }
    }
}

/// Un objeto suelto (un ítem que se ha tirado): cae, rebota un poco en las paredes y se
/// frena al tocar el suelo. No sube escalones.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Loose {
    /// Centro de la base de su caja.
    pub pos: DVec3,
    pub vel: DVec3,
    pub on_ground: bool,
}

/// Rozamiento horizontal de un objeto suelto (1/s) en el suelo y en el aire.
const LOOSE_GROUND_FRICTION: f64 = 10.0;
const LOOSE_AIR_FRICTION: f64 = 0.5;

/// Avanza un objeto suelto `dt` segundos.
pub fn step_loose(body: &mut Loose, size: BoxSize, dt: f64, world: &impl Collider) {
    let steps = (dt / MAX_STEP).ceil().max(1.0);
    let sub = dt / steps;
    for _ in 0..steps as u32 {
        // Dentro de un bloque (le han puesto uno encima): sale hacia arriba.
        if overlaps_sized(body.pos, size, world) {
            body.pos.y += UNSTUCK_SPEED * sub;
            body.vel = DVec3::ZERO;
            body.on_ground = false;
            continue;
        }
        let friction = if body.on_ground { LOOSE_GROUND_FRICTION } else { LOOSE_AIR_FRICTION };
        let keep = (-friction * sub).exp();
        body.vel.x *= keep;
        body.vel.z *= keep;
        body.vel.y = (body.vel.y - GRAVITY * sub).max(-TERMINAL_SPEED);
        for axis in [0, 2] {
            if move_axis_sized(&mut body.pos, size, axis, body.vel[axis] * sub, world) {
                body.vel[axis] = 0.0;
            }
        }
        let fall = body.vel.y * sub;
        let hit = move_axis_sized(&mut body.pos, size, 1, fall, world);
        body.on_ground = hit && fall < 0.0;
        if hit {
            body.vel.y = 0.0;
        }
        // Posado y casi quieto: quieto del todo (así deja de enviarse su posición).
        if body.on_ground && body.vel.x.hypot(body.vel.z) < 0.01 {
            body.vel.x = 0.0;
            body.vel.z = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1.0 / 60.0;

    /// Suelo macizo por debajo de y = 0, más los bloques sueltos que se indiquen.
    fn world(extra: &'static [[i32; 3]]) -> impl Fn(IVec3) -> bool {
        move |voxel| voxel.y < 0 || extra.contains(&voxel.to_array())
    }

    fn standing(x: f64, z: f64) -> Body {
        Body { pos: DVec3::new(x, 0.0, z), on_ground: true, ..Body::default() }
    }

    fn run(body: &mut Body, mode: MoveMode, input: MoveInput, seconds: f64, world: &impl Collider) {
        for _ in 0..(seconds / DT).round() as u32 {
            step(body, mode, &input, DT, world);
        }
    }

    const EAST: MoveInput = MoveInput { wish: [1.0, 0.0], up: false, down: false, sprint: false };

    #[test]
    fn a_falling_body_lands_on_the_ground() {
        let solid = world(&[]);
        let mut body = Body::new(DVec3::new(0.5, 10.0, 0.5));
        run(&mut body, MoveMode::Walk, MoveInput::default(), 3.0, &solid);
        assert!(body.on_ground);
        assert!(body.pos.y.abs() < 1e-9, "feet at {}", body.pos.y);
        assert_eq!(body.vel, DVec3::ZERO);
    }

    #[test]
    fn walking_and_sprinting_reach_their_speeds() {
        let solid = world(&[]);
        let mut body = standing(0.5, 0.5);
        run(&mut body, MoveMode::Walk, EAST, 2.0, &solid);
        assert!((body.vel.x - WALK_SPEED).abs() < 1e-9);
        // Casi toda la distancia a velocidad punta: arranca en una décima de segundo.
        assert!(body.pos.x > 0.5 + WALK_SPEED * 1.9, "walked to {}", body.pos.x);

        run(&mut body, MoveMode::Walk, MoveInput { sprint: true, ..EAST }, 1.0, &solid);
        assert!((body.vel.x - SPRINT_SPEED).abs() < 1e-9);
        // Al soltar las teclas se para enseguida.
        run(&mut body, MoveMode::Walk, MoveInput::default(), 0.25, &solid);
        assert_eq!(body.vel.x, 0.0);
    }

    #[test]
    fn a_wall_stops_the_body_flush_against_it() {
        // Muro de dos bloques de alto en x = 3.
        let solid = world(&[[3, 0, 0], [3, 1, 0]]);
        let mut body = standing(0.5, 0.5);
        run(&mut body, MoveMode::Walk, EAST, 2.0, &solid);
        assert!((body.pos.x - (3.0 - PLAYER_WIDTH / 2.0)).abs() < 1e-9, "at {}", body.pos.x);
        assert_eq!(body.pos.y, 0.0);
        assert!(body.on_ground);
    }

    #[test]
    fn one_block_steps_are_climbed_without_jumping() {
        // Escalera de tres peldaños hacia el este.
        let solid = world(&[
            [3, 0, 0], [4, 0, 0], [5, 0, 0], [6, 0, 0], [7, 0, 0], [8, 0, 0],
            [5, 1, 0], [6, 1, 0], [7, 1, 0], [8, 1, 0],
            [7, 2, 0], [8, 2, 0],
        ]);
        let mut body = standing(0.5, 0.5);
        run(&mut body, MoveMode::Walk, EAST, 1.6, &solid);
        assert!((body.pos.y - 3.0).abs() < 1e-9, "feet at {}", body.pos.y);
        // Subir no frena: recorre casi lo mismo que en llano.
        assert!(body.pos.x > 0.5 + WALK_SPEED * 1.4, "reached {}", body.pos.x);
        // La vista sube suavemente detrás del cuerpo y acaba alcanzándolo.
        assert!(body.step_offset <= 0.0);
        run(&mut body, MoveMode::Walk, MoveInput::default(), 1.0, &solid);
        assert!(body.step_offset.abs() < 1e-3);
    }

    #[test]
    fn steps_need_headroom_and_are_one_block_at_most() {
        // Un bloque con techo justo encima de la cabeza: no cabe al subir.
        let low_ceiling = world(&[[3, 0, 0], [2, 2, 0], [3, 2, 0]]);
        let mut body = standing(0.5, 0.5);
        run(&mut body, MoveMode::Walk, EAST, 2.0, &low_ceiling);
        assert_eq!(body.pos.y, 0.0);
        assert!(body.pos.x < 3.0);

        // En el aire no se suben escalones: hay que llegar saltando.
        let ledge = world(&[[3, 0, 0], [3, 1, 0]]);
        let mut body = standing(0.5, 0.5);
        run(&mut body, MoveMode::Walk, EAST, 2.0, &ledge);
        assert_eq!(body.pos.y, 0.0, "a two-block wall is not a step");
    }

    #[test]
    fn a_jump_clears_one_block_but_not_two() {
        let solid = world(&[]);
        let mut body = standing(0.5, 0.5);
        let mut top: f64 = 0.0;
        for _ in 0..120 {
            let up = body.pos.y == 0.0 && top == 0.0;
            step(&mut body, MoveMode::Walk, &MoveInput { up, ..MoveInput::default() }, DT, &solid);
            top = top.max(body.pos.y);
        }
        assert!((top - JUMP_HEIGHT).abs() < 0.02, "jumped {top}");
        assert!(body.on_ground);
    }

    #[test]
    fn a_ceiling_stops_a_jump() {
        let solid = world(&[[0, 2, 0]]);
        let mut body = standing(0.5, 0.5);
        let jump = MoveInput { up: true, ..MoveInput::default() };
        let mut top: f64 = 0.0;
        for _ in 0..60 {
            step(&mut body, MoveMode::Walk, &jump, DT, &solid);
            top = top.max(body.pos.y);
        }
        assert!(top <= 2.0 - PLAYER_HEIGHT + 1e-9, "head went through: {top}");
    }

    #[test]
    fn a_body_inside_blocks_rises_out_of_them() {
        let solid = world(&[[0, 0, 0], [0, 1, 0], [0, 2, 0]]);
        let mut body = Body::new(DVec3::new(0.5, 0.0, 0.5));
        run(&mut body, MoveMode::Walk, MoveInput::default(), 2.0, &solid);
        assert!((body.pos.y - 3.0).abs() < 1e-9, "feet at {}", body.pos.y);
        assert!(body.on_ground);
    }

    #[test]
    fn flying_collides_and_the_ghost_does_not() {
        let solid = world(&[[3, 0, 0], [3, 1, 0], [3, 2, 0]]);
        let mut body = Body::new(DVec3::new(0.5, 0.5, 0.5));
        run(&mut body, MoveMode::Fly, EAST, 2.0, &solid);
        assert!((body.pos.x - (3.0 - PLAYER_WIDTH / 2.0)).abs() < 1e-9);
        assert!((body.pos.y - 0.5).abs() < 1e-9, "flying has no gravity");

        // Bajando se posa en el suelo sin atravesarlo.
        run(&mut body, MoveMode::Fly, MoveInput { down: true, ..MoveInput::default() }, 1.0, &solid);
        assert!(body.pos.y.abs() < 1e-9);

        run(&mut body, MoveMode::Ghost, EAST, 1.0, &solid);
        assert!(body.pos.x > 10.0);
        run(&mut body, MoveMode::Ghost, MoveInput { down: true, ..MoveInput::default() }, 1.0, &solid);
        assert!(body.pos.y < -10.0);
    }

    /// Suelo macizo y una fila de losas (medio bloque) en x = 3..=5.
    struct Slabs;

    impl Collider for Slabs {
        fn shape(&self, voxel: IVec3) -> Shape<'_> {
            const SLAB: [CellBox; 1] = [[0, 0, 0, 8, 4, 8]];
            match voxel.to_array() {
                [_, y, _] if y < 0 => Shape::Full,
                [3..=5, 0, _] => Shape::Boxes(&SLAB),
                _ => Shape::Empty,
            }
        }
    }

    #[test]
    fn slabs_are_half_a_step_and_can_be_stood_on() {
        let mut body = standing(0.5, 0.5);
        run(&mut body, MoveMode::Walk, EAST, 0.6, &Slabs);
        assert!(body.pos.x > 3.5, "walked onto the slabs: {}", body.pos.x);
        assert!((body.pos.y - 0.5).abs() < 1e-9, "feet at {}", body.pos.y);
        assert!(body.on_ground);

        // Al bajar de ellas se cae hasta el suelo de verdad.
        run(&mut body, MoveMode::Walk, EAST, 1.0, &Slabs);
        assert!(body.pos.x > 6.0 && body.pos.y.abs() < 1e-9, "at {:?}", body.pos);
    }

    #[test]
    fn a_body_inside_a_slab_walks_out_sideways() {
        // Metido hasta los tobillos en una losa (p. ej. la colocó otro jugador).
        let mut body = Body { pos: DVec3::new(3.5, 0.25, 0.5), ..Body::default() };
        assert!(overlaps(body.pos, &Slabs));
        run(&mut body, MoveMode::Walk, MoveInput::default(), 0.5, &Slabs);
        assert!((body.pos.y - 0.5).abs() < 1e-9, "rose to the slab top: {}", body.pos.y);
    }

    #[test]
    fn loose_items_fall_slide_and_rest_on_slabs() {
        // Lanzado hacia el este desde lo alto: cae, choca con la pared y se posa.
        let solid = world(&[[3, 0, 0], [3, 1, 0]]);
        let mut item = Loose { pos: DVec3::new(0.5, 2.0, 0.5), vel: DVec3::new(6.0, 2.0, 0.0), ..Loose::default() };
        for _ in 0..180 {
            step_loose(&mut item, ITEM_BOX, DT, &solid);
        }
        assert!(item.on_ground && item.pos.y.abs() < 1e-9, "at {:?}", item.pos);
        assert!(item.pos.x <= 3.0 - ITEM_BOX.width / 2.0 + 1e-9, "went through the wall: {}", item.pos.x);
        assert!(item.vel.length() < 1e-6);

        // Sobre una losa se queda a media altura.
        let mut item = Loose { pos: DVec3::new(4.5, 1.5, 0.5), ..Loose::default() };
        for _ in 0..120 {
            step_loose(&mut item, ITEM_BOX, DT, &Slabs);
        }
        assert!((item.pos.y - 0.5).abs() < 1e-9 && item.on_ground);
    }

    #[test]
    fn touches_tells_which_voxels_the_body_occupies() {
        let body = standing(0.5, 0.5);
        assert!(body.touches(IVec3::new(0, 0, 0)));
        assert!(body.touches(IVec3::new(0, 1, 0)));
        assert!(!body.touches(IVec3::new(0, 2, 0)));
        assert!(!body.touches(IVec3::new(0, -1, 0)), "the floor is only touched, not occupied");
        assert!(!body.touches(IVec3::new(1, 0, 0)));
    }
}
