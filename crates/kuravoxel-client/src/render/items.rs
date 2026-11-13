//! Dibujo de los ítems sueltos: su forma en 3D (la misma de su icono), flotando y girando
//! como al soltar algo en Minecraft.

use std::num::NonZeroU64;

use glam::{Mat4, Vec3};
use rustc_hash::FxHashMap;
use wgpu::util::DeviceExt;

use kuravoxel_common::content::{ContentData, ItemId};
use kuravoxel_common::shape::BlockTable;

use crate::icons::{self, ItemShape};
use crate::mesher::MeshTable;

/// Bytes por cara: voxel (x, y, z) y cara en 4 bytes, y la entrada de paleta.
const FACE_BYTES: u64 = 8;
const ITEM_UNIFORM_BYTES: u64 = 64;
/// Ítems sueltos que se dibujan como mucho (los más cercanos).
pub const MAX_DRAWN: usize = 256;
/// Lado de un bloque suelto, en metros (un cubito, como en Minecraft). Los modelos de
/// bloque (una escalera, una puerta) usan la misma escala: 1/4 de su tamaño.
const BLOCK_SIZE: f32 = 0.25;
/// Metros por mini-voxel de un modelo suelto (la mitad de su tamaño real), y lado máximo.
const MODEL_SCALE: f32 = 1.0 / 16.0;
const MODEL_MAX: f32 = 0.6;

struct GpuShape {
    buffer: wgpu::Buffer,
    faces: u32,
    /// Tamaño en voxeles de la forma.
    size: Vec3,
    /// Metros por voxel de la forma.
    scale: f32,
}

/// Un ítem suelto a dibujar este frame.
pub struct ItemInstance {
    pub item: ItemId,
    /// Centro de su base, relativo al chunk de la cámara.
    pub base: Vec3,
    /// Giro sobre el eje vertical, en radianes.
    pub spin: f32,
}

pub struct ItemRenderer {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    stride: u64,
    shapes: FxHashMap<ItemId, GpuShape>,
    /// (forma, posición en el buffer de uniforms) de los que se dibujan este frame.
    drawn: Vec<(ItemId, u32)>,
}

impl ItemRenderer {
    pub fn new(
        device: &wgpu::Device,
        globals_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("item shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/item.wgsl").into()),
        });
        let item_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("item layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(ITEM_UNIFORM_BYTES),
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("item pipeline layout"),
            bind_group_layouts: &[globals_layout, &item_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("item pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: FACE_BYTES,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint8x4, offset: 0, shader_location: 0 },
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 4, shader_location: 1 },
                    ],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Greater,
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
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        // Una matriz por ítem en un único buffer con offset dinámico (como los chunks).
        let stride = (device.limits().min_uniform_buffer_offset_alignment as u64).max(ITEM_UNIFORM_BYTES);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("item uniforms"),
            size: stride * MAX_DRAWN as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("item bind group"),
            layout: &item_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniforms,
                    offset: 0,
                    size: NonZeroU64::new(ITEM_UNIFORM_BYTES),
                }),
            }],
        });
        Self { pipeline, uniforms, bind_group, stride, shapes: FxHashMap::default(), drawn: Vec::new() }
    }

    /// Prepara la forma de cada ítem del contenido (al recibirlo).
    pub fn set_shapes(&mut self, device: &wgpu::Device, content: &ContentData, blocks: &BlockTable, mesh: &MeshTable) {
        self.shapes.clear();
        for (id, item) in content.numbered_items() {
            let Some(shape) = icons::item_shape(content, blocks, mesh, item) else { continue };
            if let Some(gpu) = upload(device, &shape) {
                self.shapes.insert(id, gpu);
            }
        }
    }

    pub fn clear(&mut self) {
        self.shapes.clear();
        self.drawn.clear();
    }

    /// Coloca los ítems de este frame.
    pub fn set(&mut self, queue: &wgpu::Queue, items: &[ItemInstance]) {
        self.drawn.clear();
        let mut bytes = Vec::new();
        for instance in items.iter().take(MAX_DRAWN) {
            let Some(shape) = self.shapes.get(&instance.item) else { continue };
            // Base centrada en el origen, girada y escalada, y llevada a su sitio.
            let model = Mat4::from_translation(instance.base)
                * Mat4::from_rotation_y(instance.spin)
                * Mat4::from_scale(Vec3::splat(shape.scale))
                * Mat4::from_translation(Vec3::new(-shape.size.x / 2.0, 0.0, -shape.size.z / 2.0));
            let slot = self.drawn.len() as u32;
            bytes.resize(slot as usize * self.stride as usize, 0);
            bytes.extend_from_slice(bytemuck::bytes_of(&model.to_cols_array()));
            self.drawn.push((instance.item, slot));
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.uniforms, 0, &bytes);
        }
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup) -> u32 {
        if self.drawn.is_empty() {
            return 0;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, globals, &[]);
        for &(item, slot) in &self.drawn {
            let shape = &self.shapes[&item];
            pass.set_bind_group(1, &self.bind_group, &[(slot as u64 * self.stride) as u32]);
            pass.set_vertex_buffer(0, shape.buffer.slice(..));
            pass.draw(0..4, 0..shape.faces);
        }
        self.drawn.len() as u32
    }
}

fn upload(device: &wgpu::Device, shape: &ItemShape) -> Option<GpuShape> {
    if shape.faces.is_empty() {
        return None;
    }
    let mut bytes = Vec::with_capacity(shape.faces.len() * FACE_BYTES as usize);
    for face in &shape.faces {
        bytes.extend_from_slice(&[face.voxel[0], face.voxel[1], face.voxel[2], face.face]);
        bytes.extend_from_slice(&face.palette.to_le_bytes());
    }
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("item shape"),
        contents: &bytes,
        usage: wgpu::BufferUsages::VERTEX,
    });
    let extent = shape.extent();
    let largest = extent.iter().copied().fold(0.0, f32::max);
    let meters_per_mini = if shape.block {
        BLOCK_SIZE / 8.0
    } else {
        MODEL_SCALE.min(MODEL_MAX / largest)
    };
    Some(GpuShape {
        buffer,
        faces: shape.faces.len() as u32,
        size: Vec3::from_array(shape.size.map(|s| s as f32)),
        scale: meters_per_mini * shape.voxel,
    })
}
