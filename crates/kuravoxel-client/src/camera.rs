use glam::{DVec3, IVec3, Mat4, Vec3, Vec4};

use kuravoxel_common::chunk::CHUNK_SIZE;

const CHUNK_F64: f64 = CHUNK_SIZE as f64;
/// Radianes de giro por unidad de movimiento del ratón, con sensibilidad 1.
pub const MOUSE_SENSITIVITY: f32 = 0.0022;

/// Estado de entrada acumulado entre frames.
#[derive(Default)]
pub struct Input {
    pub forward: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    /// Saltar, o subir volando.
    pub up: bool,
    /// Bajar volando.
    pub down: bool,
    /// Correr, o volar deprisa.
    pub sprint: bool,
    pub mouse_dx: f32,
    pub mouse_dy: f32,
}

/// Punto de vista desde el que se dibuja. Dónde se coloca lo decide `player`.
///
/// La posición vive en f64 y a la GPU solo llega relativa al chunk de la cámara, así
/// que la precisión es la misma en el origen que a millones de voxeles de distancia.
#[derive(Clone)]
pub struct Camera {
    pub pos: DVec3,
    /// 0 mira hacia -Z; crece girando a la derecha.
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub near: f32,
}

impl Camera {
    pub fn new(pos: DVec3) -> Self {
        Self {
            pos,
            yaw: 0.0,
            pitch: -0.3,
            fov_y: 70f32.to_radians(),
            near: 0.1,
        }
    }

    pub fn forward(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
    }

    /// Chunk en el que está la cámara.
    pub fn chunk_pos(&self) -> IVec3 {
        (self.pos / CHUNK_F64).floor().as_ivec3()
    }

    /// Posición relativa al origen de su chunk (siempre en [0, 32)).
    pub fn local_pos(&self) -> Vec3 {
        (self.pos - self.chunk_pos().as_dvec3() * CHUNK_F64).as_vec3()
    }

    /// Matriz vista-proyección en el espacio relativo al chunk de la cámara.
    /// Z invertida con plano lejano infinito: máxima precisión de profundidad.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = Mat4::look_to_rh(self.local_pos(), self.forward(), Vec3::Y);
        let proj = Mat4::perspective_infinite_reverse_rh(self.fov_y, aspect, self.near);
        proj * view
    }
}

/// Los cuatro planos laterales del frustum (el lejano es infinito y el cercano no
/// descarta nada útil a escala de chunk).
pub struct Frustum {
    planes: [Vec4; 4],
}

impl Frustum {
    pub fn from_view_proj(m: Mat4) -> Self {
        let (x, y, w) = (m.row(0), m.row(1), m.row(3));
        Self {
            planes: [w + x, w - x, w + y, w - y],
        }
    }

    /// `false` solo si la caja queda totalmente fuera de algún plano.
    pub fn intersects_aabb(&self, min: Vec3, max: Vec3) -> bool {
        self.planes.iter().all(|plane| {
            let normal = plane.truncate();
            // Esquina de la caja más avanzada en la dirección de la normal.
            let corner = Vec3::select(normal.cmpge(Vec3::ZERO), max, min);
            normal.dot(corner) + plane.w >= 0.0
        })
    }
}
