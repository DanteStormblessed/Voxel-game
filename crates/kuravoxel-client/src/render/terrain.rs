//! Dibujo del terreno: pipeline, mallas de chunk en GPU, culling y orden de dibujo.

use std::num::NonZeroU64;

use glam::{IVec3, Vec2, Vec3};
use rustc_hash::FxHashMap;
use wgpu::util::DeviceExt;

use crate::camera::Frustum;
use crate::mesher::ChunkMesh;
use voxel_common::chunk::CHUNK_SIZE_I;
use crate::world::MeshKey;

const QUAD_BYTES: u64 = 8;
const CHUNK_UNIFORM_BYTES: u64 = 16;
const INITIAL_SLOTS: u32 = 1024;

struct GpuChunk {
    key: MeshKey,
    /// Esquina mínima en chunks de 32 voxeles.
    origin: IVec3,
    /// Tamaño de celda en voxeles: 1 para chunks reales, 2, 4... para terreno lejano.
    scale: i32,
    buffer: wgpu::Buffer,
    face_ranges: [(u32, u32); 6],
    /// Posición de su origen dentro del buffer de uniforms compartido.
    slot: u32,
    quads: u32,
}

struct Visible {
    dist_sq: f32,
    index: u32,
    /// Bit por cara (+X, -X, +Y, -Y, +Z, -Z) que puede estar mirando a la cámara.
    faces: u8,
}

pub struct TerrainRenderer {
    pipeline: wgpu::RenderPipeline,

    // Origen de cada chunk: un único buffer de uniforms direccionado con offset dinámico.
    // Funciona igual en Vulkan y en GLES 3 (sin push constants ni storage buffers).
    chunk_layout: wgpu::BindGroupLayout,
    chunk_uniforms: wgpu::Buffer,
    chunk_bind_group: wgpu::BindGroup,
    slot_stride: u64,
    slot_capacity: u32,
    next_slot: u32,
    free_slots: Vec<u32>,
    slot_origins: Vec<[i32; 4]>,

    chunks: Vec<GpuChunk>,
    index: FxHashMap<MeshKey, usize>,
    visible: Vec<Visible>,
    total_quads: u64,
}

impl TerrainRenderer {
    pub fn new(
        device: &wgpu::Device,
        globals_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/terrain.wgsl").into()),
        });

        let chunk_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(CHUNK_UNIFORM_BYTES),
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain pipeline layout"),
            bind_group_layouts: &[globals_layout, &chunk_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: QUAD_BYTES,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Uint32x2,
                        offset: 0,
                        shader_location: 0,
                    }],
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
                // Z invertida: lo más cercano tiene el valor más alto.
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

        let slot_stride =
            (device.limits().min_uniform_buffer_offset_alignment as u64).max(CHUNK_UNIFORM_BYTES);
        let (chunk_uniforms, chunk_bind_group) =
            create_chunk_uniforms(device, &chunk_layout, INITIAL_SLOTS, slot_stride);

        Self {
            pipeline,
            chunk_layout,
            chunk_uniforms,
            chunk_bind_group,
            slot_stride,
            slot_capacity: INITIAL_SLOTS,
            next_slot: 0,
            free_slots: Vec::new(),
            slot_origins: vec![[0; 4]; INITIAL_SLOTS as usize],
            chunks: Vec::new(),
            index: FxHashMap::default(),
            visible: Vec::new(),
            total_quads: 0,
        }
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }

    pub fn total_quads(&self) -> u64 {
        self.total_quads
    }

    /// Sube (o reemplaza) la malla de un chunk.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: MeshKey,
        mesh: &ChunkMesh,
    ) {
        self.remove(key);
        if mesh.quads.is_empty() {
            return;
        }

        let (origin, scale) = (key.origin(), key.scale());
        let slot = self.alloc_slot(device, queue, origin, scale);
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&mesh.quads),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let quads = mesh.quads.len() as u32;
        self.total_quads += quads as u64;
        self.index.insert(key, self.chunks.len());
        self.chunks.push(GpuChunk {
            key,
            origin,
            scale,
            buffer,
            face_ranges: mesh.face_ranges,
            slot,
            quads,
        });
    }

    pub fn remove(&mut self, key: MeshKey) {
        let Some(i) = self.index.remove(&key) else { return };
        let removed = self.chunks.swap_remove(i);
        self.free_slots.push(removed.slot);
        self.total_quads -= removed.quads as u64;
        if let Some(moved) = self.chunks.get(i) {
            self.index.insert(moved.key, i);
        }
    }

    fn alloc_slot(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        origin: IVec3,
        scale: i32,
    ) -> u32 {
        let slot = match self.free_slots.pop() {
            Some(slot) => slot,
            None => {
                if self.next_slot == self.slot_capacity {
                    self.grow_slots(device, queue);
                }
                self.next_slot += 1;
                self.next_slot - 1
            }
        };
        let origin = [origin.x, origin.y, origin.z, scale];
        self.slot_origins[slot as usize] = origin;
        queue.write_buffer(
            &self.chunk_uniforms,
            slot as u64 * self.slot_stride,
            bytemuck::bytes_of(&origin),
        );
        slot
    }

    /// Duplica el buffer de uniforms de chunk y vuelve a escribir todos los orígenes.
    fn grow_slots(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.slot_capacity *= 2;
        self.slot_origins.resize(self.slot_capacity as usize, [0; 4]);
        let (buffer, bind_group) =
            create_chunk_uniforms(device, &self.chunk_layout, self.slot_capacity, self.slot_stride);

        let stride = self.slot_stride as usize;
        let mut bytes = vec![0u8; self.slot_capacity as usize * stride];
        for (slot, origin) in self.slot_origins.iter().enumerate() {
            let start = slot * stride;
            bytes[start..start + CHUNK_UNIFORM_BYTES as usize]
                .copy_from_slice(bytemuck::bytes_of(origin));
        }
        queue.write_buffer(&buffer, 0, &bytes);

        self.chunk_uniforms = buffer;
        self.chunk_bind_group = bind_group;
    }

    /// Decide qué chunks se dibujan este frame y en qué orden.
    ///
    /// `cam_local` es la posición de la cámara relativa al origen de `cam_chunk`, el mismo
    /// espacio en el que está construido el frustum.
    ///
    /// `clip` es la distancia horizontal a la que los voxeles reales ceden el paso al
    /// terreno lejano; `band` es el solape entre ambos.
    pub fn cull(
        &mut self,
        frustum: &Frustum,
        cam_chunk: IVec3,
        cam_local: Vec3,
        clip: f32,
        band: f32,
    ) {
        self.visible.clear();
        for (index, chunk) in self.chunks.iter().enumerate() {
            let extent = Vec3::splat((CHUNK_SIZE_I * chunk.scale) as f32);
            let min = ((chunk.origin - cam_chunk) * CHUNK_SIZE_I).as_vec3();
            let max = min + extent;

            // Lo que el shader descartaría entero por distancia ni se dibuja.
            let near = (cam_local.clamp(min, max) - cam_local).truncate_y();
            let far = (min - cam_local).abs().max((max - cam_local).abs()).truncate_y();
            let skip = if chunk.scale == 1 {
                near.length_squared() > clip * clip
            } else {
                far.length() < clip - band
            };
            if skip || !frustum.intersects_aabb(min, max) {
                continue;
            }

            // Una cara +X solo puede verse desde su lado positivo, etc. Con la cámara
            // fuera del chunk esto descarta la mitad de los quads sin tocar la GPU.
            let faces = (cam_local.x > min.x) as u8
                | ((cam_local.x < max.x) as u8) << 1
                | ((cam_local.y > min.y) as u8) << 2
                | ((cam_local.y < max.y) as u8) << 3
                | ((cam_local.z > min.z) as u8) << 4
                | ((cam_local.z < max.z) as u8) << 5;

            self.visible.push(Visible {
                dist_sq: (min + extent * 0.5).distance_squared(cam_local),
                index: index as u32,
                faces,
            });
        }
        // De cerca a lejos: el z-buffer descarta pronto lo tapado (menos sobredibujado,
        // que es lo que más cuesta en GPUs modestas y móviles).
        self.visible
            .sort_unstable_by(|a, b| a.dist_sq.total_cmp(&b.dist_sq));
    }

    /// Dibuja los chunks elegidos por `cull`. Devuelve el número de draw calls.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup) -> u32 {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, globals, &[]);

        let mut draw_calls = 0;
        for visible in &self.visible {
            let chunk = &self.chunks[visible.index as usize];
            let mut bound = false;
            // Las caras visibles contiguas en el buffer se dibujan de una vez.
            let mut run: Option<(u32, u32)> = None;
            for face in 0..=6 {
                let range = (face < 6 && visible.faces & (1 << face) != 0)
                    .then(|| chunk.face_ranges[face])
                    .filter(|(start, end)| end > start);
                match (run, range) {
                    (Some((start, end)), Some((next_start, next_end))) if end == next_start => {
                        run = Some((start, next_end));
                    }
                    (current, next) => {
                        if let Some((start, end)) = current {
                            if !bound {
                                let offset = (chunk.slot as u64 * self.slot_stride) as u32;
                                pass.set_bind_group(1, &self.chunk_bind_group, &[offset]);
                                bound = true;
                            }
                            // El rango va en el offset del buffer y no en `first_instance`,
                            // que en GLES no está disponible.
                            let bytes = start as u64 * QUAD_BYTES..end as u64 * QUAD_BYTES;
                            pass.set_vertex_buffer(0, chunk.buffer.slice(bytes));
                            pass.draw(0..4, 0..end - start);
                            draw_calls += 1;
                        }
                        run = next;
                    }
                }
            }
        }
        draw_calls
    }
}

trait TruncateY {
    /// Componentes horizontales (X, Z).
    fn truncate_y(self) -> Vec2;
}

impl TruncateY for Vec3 {
    fn truncate_y(self) -> Vec2 {
        Vec2::new(self.x, self.z)
    }
}

fn create_chunk_uniforms(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    slots: u32,
    stride: u64,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("chunk uniforms"),
        size: slots as u64 * stride,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("chunk bind group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: NonZeroU64::new(CHUNK_UNIFORM_BYTES),
            }),
        }],
    });
    (buffer, bind_group)
}
