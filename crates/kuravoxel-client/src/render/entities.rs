//! Dibujo de entidades (de momento, otros jugadores) como cajas de color.

use glam::Vec3;

/// Cajas por frame. La mayoría son de la vista previa de un prefab (una por voxel visible).
pub const MAX_INSTANCES: usize = 16384;

/// Una caja orientada. Debe coincidir con las entradas de entity.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EntityInstance {
    /// xyz: centro relativo al chunk de la cámara; w: giro sobre Y.
    center_yaw: [f32; 4],
    size: [f32; 4],
    color: [f32; 4],
}

const SHIRTS: [[f32; 3]; 6] = [
    [0.80, 0.10, 0.10],
    [0.10, 0.25, 0.80],
    [0.90, 0.55, 0.05],
    [0.45, 0.10, 0.70],
    [0.05, 0.55, 0.55],
    [0.85, 0.15, 0.50],
];
const SKIN: [f32; 3] = [0.87, 0.60, 0.42];

impl EntityInstance {
    /// Caja alineada con los ejes, de un color.
    pub fn cuboid(center: Vec3, size: Vec3, color: [f32; 3]) -> EntityInstance {
        EntityInstance {
            center_yaw: [center.x, center.y, center.z, 0.0],
            size: [size.x, size.y, size.z, 0.0],
            color: [color[0], color[1], color[2], 1.0],
        }
    }

    /// Caja translúcida de las marcas: color con opacidad y, si se pide, rejilla de bloques.
    pub fn panel(center: Vec3, size: Vec3, color: [f32; 4], grid: bool) -> EntityInstance {
        EntityInstance {
            center_yaw: [center.x, center.y, center.z, 0.0],
            size: [size.x, size.y, size.z, if grid { 1.0 } else { 0.0 }],
            color,
        }
    }

    /// Cuerpo y cabeza de un jugador cuyos ojos están en `eyes`.
    pub fn player(eyes: Vec3, yaw: f32, id: u32) -> [EntityInstance; 2] {
        let shirt = SHIRTS[id as usize % SHIRTS.len()];
        let part = |offset_y: f32, size: [f32; 3], color: [f32; 3]| EntityInstance {
            center_yaw: [eyes.x, eyes.y + offset_y, eyes.z, yaw],
            size: [size[0], size[1], size[2], 0.0],
            color: [color[0], color[1], color[2], 1.0],
        };
        [
            part(-0.95, [0.7, 1.3, 0.4], shirt),
            part(-0.05, [0.5, 0.5, 0.5], SKIN),
        ]
    }
}

pub struct EntityRenderer {
    pipeline: wgpu::RenderPipeline,
    /// Mapas de sombras (las marcas no dan sombra: no lo tienen).
    shadow_pipeline: Option<wgpu::RenderPipeline>,
    instances: wgpu::Buffer,
    count: u32,
    /// Las primeras `casters` cajas dan sombra (los jugadores).
    casters: u32,
}

impl EntityRenderer {
    /// Con `overlay`, las cajas son marcas: translúcidas, sin prueba de profundidad (se
    /// ven a través de los bloques) y con las dos caras.
    pub fn new(
        device: &wgpu::Device,
        globals_layout: &wgpu::BindGroupLayout,
        shadow_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        overlay: bool,
    ) -> Self {
        let source = if overlay { include_str!("shaders/overlay.wgsl") } else { include_str!("shaders/entity.wgsl") };
        let shader = super::scene_shader(device, "entity shader", source);
        let instance_buffers = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<EntityInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
        }];
        let shadow_pipeline = (!overlay).then(|| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("entity shadow pipeline layout"),
                bind_group_layouts: &[shadow_layout],
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("entity shadow pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &super::shadow_shader(device),
                    entry_point: Some("vs_entity"),
                    compilation_options: Default::default(),
                    buffers: &instance_buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    // Como el terreno: solo las caras que dan la espalda a la luz.
                    cull_mode: Some(wgpu::Face::Front),
                    ..Default::default()
                },
                depth_stencil: Some(super::shadows::depth_state()),
                multisample: wgpu::MultisampleState::default(),
                fragment: None,
                multiview: None,
                cache: None,
            })
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("entity pipeline layout"),
            bind_group_layouts: &[globals_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("entity pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<EntityInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: (!overlay).then_some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: !overlay,
                depth_compare: if overlay { wgpu::CompareFunction::Always } else { wgpu::CompareFunction::Greater },
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: overlay.then_some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("entity instances"),
            size: (MAX_INSTANCES * std::mem::size_of::<EntityInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, shadow_pipeline, instances, count: 0, casters: 0 }
    }

    /// Cajas de este frame; las primeras `casters` dan sombra.
    pub fn set(&mut self, queue: &wgpu::Queue, instances: &[EntityInstance], casters: usize) {
        let instances = &instances[..instances.len().min(MAX_INSTANCES)];
        self.count = instances.len() as u32;
        self.casters = casters.min(instances.len()) as u32;
        if !instances.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(instances));
        }
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..36, 0..self.count);
    }

    /// Dibuja en un mapa de sombras las cajas que dan sombra.
    pub fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup) {
        let Some(pipeline) = self.shadow_pipeline.as_ref().filter(|_| self.casters > 0) else { return };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..36, 0..self.casters);
    }
}
