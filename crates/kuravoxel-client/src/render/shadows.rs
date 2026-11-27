//! Sombras en cascada: tres mapas de profundidad vistos desde la luz, cada uno cubriendo
//! una zona mayor alrededor de la cámara con menos detalle.
//!
//! Cada cascada es una esfera delante de la cámara, de radio fijo: su mapa no cambia de
//! tamaño al girar la vista y su centro se mueve de texel en texel, así que las sombras no
//! bailan al moverse. El shader usa la primera cascada que contiene el punto.

use glam::{DVec3, Mat4, Vec3};

use crate::camera::Camera;
use kuravoxel_common::chunk::CHUNK_SIZE;

pub const CASCADES: usize = 3;
/// Radio de cada cascada como fracción de la distancia de sombras.
const RADIUS: [f32; CASCADES] = [0.085, 0.29, 1.0];
/// Cuánto se adelanta el centro de la esfera respecto a la cámara, en radios: casi todo
/// queda delante, con algo de margen detrás.
const FORWARD: f32 = 0.7;
/// Metros hacia la luz, más allá de la esfera, en los que algo todavía da sombra.
const CASTER_RANGE: f32 = 320.0;
/// Un mapa ya dibujado se reutiliza mientras su centro no se aleje del que tocaría más de
/// esta fracción del radio (cubre casi lo mismo)...
const REUSE_CENTER: f32 = 0.03;
/// ...y la luz no haya girado más de este ángulo (seno de ~0,05°). Con el día en marcha,
/// el sol avanza así a saltos invisibles en vez de obligar a redibujar cada frame.
const REUSE_LIGHT_SIN: f32 = 8.7e-4;
/// Separación (m) que se resta al comparar profundidades.
const DEPTH_BIAS: f32 = 0.03;

pub const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Una cascada, en el espacio relativo al chunk de la cámara.
#[derive(Clone, Copy, Debug)]
pub struct Cascade {
    pub view_proj: Mat4,
    center: Vec3,
    radius: f32,
    right: Vec3,
    up: Vec3,
    /// Hacia la luz.
    light: Vec3,
    /// Metros por texel.
    pub texel: f32,
}

impl Cascade {
    /// Esta cascada, ya dibujada, puede usarse en lugar de `wanted`: mismo tamaño, casi la
    /// misma luz y casi el mismo centro. El shader la lee con su propia matriz, así que la
    /// sombra sigue siendo exacta; solo cubre un poco menos por un lado.
    pub fn can_stand_in_for(&self, wanted: &Cascade) -> bool {
        self.radius == wanted.radius
            && self.texel == wanted.texel
            && self.light.cross(wanted.light).length() <= REUSE_LIGHT_SIN
            && self.light.dot(wanted.light) > 0.0
            && self.center.distance(wanted.center) <= self.radius * REUSE_CENTER
    }

    /// Una esfera puede dar sombra dentro de la cascada.
    pub fn touches(&self, center: Vec3, radius: f32) -> bool {
        let d = center - self.center;
        let reach = self.radius + radius;
        let toward = d.dot(self.light);
        d.dot(self.right).abs() < reach
            && d.dot(self.up).abs() < reach
            && toward > -reach
            && toward < reach + CASTER_RANGE
    }
}

/// Profundidad (m) que abarca cada mapa: la misma en todos, para que el sesgo valga igual.
fn depth_span(distance: f32) -> f32 {
    2.0 * distance + CASTER_RANGE
}

/// Sesgo de profundidad, en unidades del mapa.
pub fn depth_bias(distance: f32) -> f32 {
    DEPTH_BIAS / depth_span(distance)
}

/// Las cascadas de un frame. `light` apunta hacia la luz y `axis` es perpendicular a ella
/// (el eje sobre el que gira el sol): da una orientación estable a los mapas.
pub fn cascades(camera: &Camera, light: Vec3, axis: Vec3, distance: f32, resolution: u32) -> [Cascade; CASCADES] {
    let origin = camera.chunk_pos().as_dvec3() * CHUNK_SIZE as f64;
    let forward = camera.forward();
    let right = (-light).cross(axis).normalize();
    let up = right.cross(-light).normalize();
    let span = depth_span(distance);

    std::array::from_fn(|i| {
        let radius = distance * RADIUS[i];
        let texel = 2.0 * radius / resolution as f32;
        let mut center = camera.local_pos() + forward * radius * FORWARD;
        // El centro se ajusta a la rejilla de texels del mapa (medida en el mundo, no
        // relativa a la cámara): al moverse, las sombras no tiemblan.
        let world: DVec3 = origin + center.as_dvec3();
        for side in [right, up] {
            let along = world.dot(side.as_dvec3());
            let snapped = (along / texel as f64).floor() * texel as f64;
            center += side * (snapped - along) as f32;
        }
        let eye = center + light * (radius + CASTER_RANGE);
        let view = Mat4::look_to_rh(eye, -light, up);
        let proj = Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.0, span);
        Cascade { view_proj: proj * view, center, radius, right, up, light, texel }
    })
}

/// Uniforms de una pasada de sombras. Debe coincidir con `ShadowGlobals` en shadow.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShadowGlobals {
    pub light_view_proj: [[f32; 4]; 4],
    pub cam_chunk: [i32; 4],
    pub cam_local: [f32; 4],
}

/// Los mapas de sombras en la GPU y lo que hace falta para dibujar en ellos.
pub struct ShadowMaps {
    pub resolution: u32,
    /// Las tres capas, para leerlas desde la escena.
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    layers: [wgpu::TextureView; CASCADES],
    buffers: [wgpu::Buffer; CASCADES],
    bind_groups: [wgpu::BindGroup; CASCADES],
}

/// Grupo 0 de las pasadas de sombras: solo sus uniforms.
pub fn create_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shadow layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

/// Estado de profundidad de una pasada de sombras.
pub fn depth_state() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: SHADOW_FORMAT,
        depth_write_enabled: true,
        depth_compare: wgpu::CompareFunction::LessEqual,
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

impl ShadowMaps {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, resolution: u32) -> Self {
        let resolution = resolution.min(device.limits().max_texture_dimension_2d);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow maps"),
            size: wgpu::Extent3d { width: resolution, height: resolution, depth_or_array_layers: CASCADES as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let layers = std::array::from_fn(|layer| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer as u32,
                array_layer_count: Some(1),
                ..Default::default()
            })
        });
        // Comparación con filtro lineal: cada muestra ya mezcla sus cuatro texels.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let buffers: [wgpu::Buffer; CASCADES] = std::array::from_fn(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shadow globals"),
                size: std::mem::size_of::<ShadowGlobals>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let bind_groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shadow bind group"),
                layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffers[i].as_entire_binding() }],
            })
        });
        Self { resolution, view, sampler, layers, buffers, bind_groups }
    }

    pub fn write(&self, queue: &wgpu::Queue, cascade: usize, globals: &ShadowGlobals) {
        queue.write_buffer(&self.buffers[cascade], 0, bytemuck::bytes_of(globals));
    }

    pub fn bind_group(&self, cascade: usize) -> &wgpu::BindGroup {
        &self.bind_groups[cascade]
    }

    /// Empieza la pasada de una cascada, con el mapa vacío.
    pub fn begin<'a>(
        &'a self,
        encoder: &'a mut wgpu::CommandEncoder,
        cascade: usize,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'a>>,
    ) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.layers[cascade],
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        let mut camera = Camera::new(DVec3::new(1000.3, 70.2, -500.7));
        camera.yaw = 0.6;
        camera
    }

    #[test]
    fn cascades_cover_what_is_in_front_of_the_camera() {
        let camera = camera();
        let light = Vec3::new(0.4, 0.8, -0.3).normalize();
        let axis = light.cross(Vec3::X).normalize();
        let all = cascades(&camera, light, axis, 200.0, 2048);
        let eye = camera.local_pos();
        for (i, cascade) in all.iter().enumerate() {
            let radius = 200.0 * RADIUS[i];
            for distance in [0.0, radius * 0.5, radius * 1.4] {
                let point = eye + camera.forward() * distance;
                let clip = cascade.view_proj * point.extend(1.0);
                assert!(clip.x.abs() < 1.0 && clip.y.abs() < 1.0, "cascade {i} misses {distance} m: {clip}");
                assert!(clip.z > 0.0 && clip.z < 1.0, "cascade {i}: depth {}", clip.z);
            }
            // Lo que está hacia la luz queda más cerca en el mapa.
            let near = cascade.view_proj * (eye + light * 50.0).extend(1.0);
            let far = cascade.view_proj * eye.extend(1.0);
            assert!(near.z < far.z);
            assert!(cascade.touches(eye + light * 200.0, 28.0), "a caster far towards the light counts");
            assert!(!cascade.touches(eye - light * (radius * 3.0 + 60.0), 28.0));
        }
        assert!(all[0].texel < all[1].texel && all[1].texel < all[2].texel);
    }

    #[test]
    fn the_maps_move_in_whole_texels() {
        let light = Vec3::new(0.4, 0.8, -0.3).normalize();
        let axis = light.cross(Vec3::X).normalize();
        let a = cascades(&camera(), light, axis, 200.0, 2048)[1];
        let mut moved = camera();
        moved.pos += DVec3::new(0.013, 0.0, 0.021);
        let b = cascades(&moved, light, axis, 200.0, 2048)[1];
        let shift = b.center - a.center;
        for side in [a.right, a.up] {
            let texels = shift.dot(side) / a.texel;
            assert!((texels - texels.round()).abs() < 0.02, "moved {texels} texels");
        }
    }

    #[test]
    fn a_drawn_map_stands_in_while_little_changes() {
        let light = Vec3::new(0.4, 0.8, -0.3).normalize();
        let axis = light.cross(Vec3::X).normalize();
        let drawn = cascades(&camera(), light, axis, 200.0, 2048);
        assert!(drawn.iter().all(|cascade| cascade.can_stand_in_for(cascade)));

        // Unos centímetros: el mapa grande sigue valiendo, el pequeño también.
        let mut moved = camera();
        moved.pos += DVec3::new(0.2, 0.0, 0.1);
        let wanted = cascades(&moved, light, axis, 200.0, 2048);
        assert!(drawn[2].can_stand_in_for(&wanted[2]));
        assert!(drawn[0].can_stand_in_for(&wanted[0]));
        // Diez metros: ya no.
        moved.pos += DVec3::new(10.0, 0.0, 0.0);
        let wanted = cascades(&moved, light, axis, 200.0, 2048);
        assert!(!drawn[0].can_stand_in_for(&wanted[0]) && !drawn[2].can_stand_in_for(&wanted[2]));

        // La luz: una centésima de grado vale; medio grado, no.
        let turn = |degrees: f32| glam::Quat::from_axis_angle(axis, degrees.to_radians()) * light;
        let wanted = cascades(&camera(), turn(0.01), axis, 200.0, 2048);
        assert!(drawn[1].can_stand_in_for(&wanted[1]));
        let wanted = cascades(&camera(), turn(0.5), axis, 200.0, 2048);
        assert!(!drawn[1].can_stand_in_for(&wanted[1]));
        // Otra calidad (otro tamaño de texel): no.
        let wanted = cascades(&camera(), light, axis, 200.0, 4096);
        assert!(!drawn[1].can_stand_in_for(&wanted[1]));
    }
}
