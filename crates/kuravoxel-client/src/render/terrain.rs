//! Dibujo del terreno: pipeline, mallas de chunk en GPU, culling y orden de dibujo.

use std::num::NonZeroU64;

use glam::{IVec3, Vec2, Vec3};
use rustc_hash::FxHashMap;
use wgpu::util::DeviceExt;

use super::shadows::{self, Cascade};
use crate::camera::Frustum;
use crate::mesher::ChunkMesh;
use kuravoxel_common::chunk::CHUNK_SIZE_I;
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
    /// Quads translúcidos (al final del buffer).
    transparent: (u32, u32),
    /// Posición de su origen dentro del buffer de uniforms compartido.
    slot: u32,
    quads: u32,
}

struct Visible {
    dist_sq: f32,
    index: u32,
    /// Bit por cara (+X, -X, +Y, -Y, +Z, -Z) que puede estar mirando a la cámara.
    faces: u8,
    /// Cruza el relevo entre voxeles y terreno lejano: hay que recortarlo píxel a píxel.
    clipped: bool,
}

pub struct TerrainRenderer {
    /// Lo opaco, sin `discard` (la GPU descarta lo tapado antes de sombrearlo).
    pipeline: wgpu::RenderPipeline,
    /// Lo opaco que cruza el relevo entre voxeles y terreno lejano: recorta por píxel.
    clipped_pipeline: wgpu::RenderPipeline,
    /// Segundo pase: agua y cristal, con mezcla y sin escribir profundidad.
    translucent_pipeline: wgpu::RenderPipeline,
    /// Mapas de sombras: solo profundidad, vista desde la luz.
    shadow_pipeline: wgpu::RenderPipeline,

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
    /// Sube con cada malla de voxeles reales que entra o sale (lo que da sombra).
    shadow_version: u64,
}

impl TerrainRenderer {
    pub fn new(
        device: &wgpu::Device,
        globals_layout: &wgpu::BindGroupLayout,
        shadow_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = super::scene_shader(device, "terrain shader", include_str!("shaders/terrain.wgsl"));

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

        let pipeline_for = |label: &str, translucent: bool, fragment: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
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
                    // El agua y el cristal se ven también desde dentro.
                    cull_mode: if translucent { None } else { Some(wgpu::Face::Back) },
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    // Lo translúcido no tapa: lo que hay detrás ya está dibujado.
                    depth_write_enabled: !translucent,
                    // Z invertida: lo más cercano tiene el valor más alto.
                    depth_compare: wgpu::CompareFunction::Greater,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: translucent.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            })
        };
        let pipeline = pipeline_for("terrain pipeline", false, "fs_main");
        let clipped_pipeline = pipeline_for("clipped terrain pipeline", false, "fs_clipped");
        // Lo translúcido no escribe profundidad: recortar siempre no le quita nada.
        let translucent_pipeline = pipeline_for("translucent terrain pipeline", true, "fs_clipped");

        let quad_buffers = [wgpu::VertexBufferLayout {
            array_stride: QUAD_BYTES,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32x2,
                offset: 0,
                shader_location: 0,
            }],
        }];
        let shadow_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain shadow pipeline layout"),
            bind_group_layouts: &[shadow_layout, &chunk_layout],
            push_constant_ranges: &[],
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain shadow pipeline"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &super::shadow_shader(device),
                entry_point: Some("vs_terrain"),
                compilation_options: Default::default(),
                buffers: &quad_buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                front_face: wgpu::FrontFace::Ccw,
                // Qué caras se dibujan lo elige `draw_shadow`.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(shadows::depth_state()),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview: None,
            cache: None,
        });

        let slot_stride =
            (device.limits().min_uniform_buffer_offset_alignment as u64).max(CHUNK_UNIFORM_BYTES);
        let (chunk_uniforms, chunk_bind_group) =
            create_chunk_uniforms(device, &chunk_layout, INITIAL_SLOTS, slot_stride);

        Self {
            pipeline,
            clipped_pipeline,
            translucent_pipeline,
            shadow_pipeline,
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
            shadow_version: 0,
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

    /// Sube cada vez que cambia algo que da sombra (una malla de voxeles reales).
    pub fn shadow_version(&self) -> u64 {
        self.shadow_version
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
        if scale == 1 {
            self.shadow_version += 1;
        }
        self.index.insert(key, self.chunks.len());
        self.chunks.push(GpuChunk {
            key,
            origin,
            scale,
            buffer,
            face_ranges: mesh.face_ranges,
            transparent: mesh.transparent,
            slot,
            quads,
        });
    }

    /// Suelta todas las mallas.
    pub fn clear(&mut self) {
        for chunk in self.chunks.drain(..) {
            self.free_slots.push(chunk.slot);
        }
        self.index.clear();
        self.visible.clear();
        self.total_quads = 0;
        self.shadow_version += 1;
    }

    pub fn remove(&mut self, key: MeshKey) {
        let Some(i) = self.index.remove(&key) else { return };
        let removed = self.chunks.swap_remove(i);
        self.free_slots.push(removed.slot);
        self.total_quads -= removed.quads as u64;
        if removed.scale == 1 {
            self.shadow_version += 1;
        }
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
    /// terreno lejano y `voxel_band` las alturas (relativas al chunk de la cámara) entre
    /// las que hay voxeles; `band` es el solape entre ambos. `curvature` es el radio con el
    /// que el shader curva el terreno hacia abajo: las cajas se alargan para cubrirlo.
    pub fn cull(
        &mut self,
        frustum: &Frustum,
        cam_chunk: IVec3,
        cam_local: Vec3,
        clip: f32,
        voxel_band: [f32; 2],
        band: f32,
        curvature: f32,
    ) {
        self.visible.clear();
        for (index, chunk) in self.chunks.iter().enumerate() {
            let extent = Vec3::splat((CHUNK_SIZE_I * chunk.scale) as f32);
            let mut min = ((chunk.origin - cam_chunk) * CHUNK_SIZE_I).as_vec3();
            let max = min + extent;

            // Lo que el shader descartaría entero por distancia ni se dibuja.
            let near = (cam_local.clamp(min, max) - cam_local).truncate_y();
            let far = (min - cam_local).abs().max((max - cam_local).abs()).truncate_y();
            min.y -= far.length_squared() / (2.0 * curvature);
            let (skip, clipped) = if chunk.scale == 1 {
                // Voxeles: se recortan si parte del chunk queda más allá de `clip`.
                (near.length_squared() > clip * clip, far.length_squared() > clip * clip)
            } else {
                // Terreno lejano tapado del todo por los voxeles: dentro del círculo y
                // dentro de las capas de chunks que hay cargadas. Si solo una parte lo
                // está, se recorta píxel a píxel.
                let below = min.y >= voxel_band[0] + band && max.y <= voxel_band[1] - band;
                let touches = near.length() < clip - band
                    && max.y > voxel_band[0] + band
                    && min.y < voxel_band[1] - band;
                (far.length() < clip - band && below, touches)
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
                clipped,
            });
        }
        // De cerca a lejos: el z-buffer descarta pronto lo tapado (menos sobredibujado,
        // que es lo que más cuesta en GPUs modestas y móviles).
        self.visible
            .sort_unstable_by(|a, b| a.dist_sq.total_cmp(&b.dist_sq));
    }

    /// Dibuja los chunks elegidos por `cull`. Devuelve el número de draw calls.
    /// Primero lo que no cruza el relevo (sin `discard`) y después lo que sí, cada grupo
    /// de cerca a lejos.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup) -> u32 {
        let mut draw_calls = 0;
        for (pipeline, clipped) in [(&self.pipeline, false), (&self.clipped_pipeline, true)] {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, globals, &[]);
            for visible in self.visible.iter().filter(|visible| visible.clipped == clipped) {
                draw_calls += self.draw_faces(pass, &self.chunks[visible.index as usize], visible.faces);
            }
        }
        draw_calls
    }

    /// Dibuja las caras pedidas (un bit por cara) de un chunk. Devuelve las draw calls.
    fn draw_faces(&self, pass: &mut wgpu::RenderPass<'_>, chunk: &GpuChunk, faces: u8) -> u32 {
        let mut draw_calls = 0;
        // Las caras contiguas en el buffer se dibujan de una vez.
        let mut run: Option<(u32, u32)> = None;
        for face in 0..=6 {
            let range = (face < 6 && faces & (1 << face) != 0)
                .then(|| chunk.face_ranges[face])
                .filter(|(start, end)| end > start);
            match (run, range) {
                (Some((start, end)), Some((next_start, next_end))) if end == next_start => {
                    run = Some((start, next_end));
                }
                (current, next) => {
                    if let Some((start, end)) = current {
                        if draw_calls == 0 {
                            let offset = (chunk.slot as u64 * self.slot_stride) as u32;
                            pass.set_bind_group(1, &self.chunk_bind_group, &[offset]);
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
        draw_calls
    }

    /// Dibuja en un mapa de sombras los chunks de voxeles reales que pueden dar sombra
    /// dentro de `cascade`. Solo las caras que dan la espalda a la luz (`light` apunta
    /// hacia ella): lo iluminado nunca se compara consigo mismo. Lo translúcido no da
    /// sombra. Devuelve el número de draw calls.
    pub fn draw_shadow(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        globals: &wgpu::BindGroup,
        cam_chunk: IVec3,
        cascade: &Cascade,
        light: Vec3,
    ) -> u32 {
        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_bind_group(0, globals, &[]);
        // Caras +X, -X, +Y, -Y, +Z, -Z.
        let faces = (light.x < 0.0) as u8
            | ((light.x > 0.0) as u8) << 1
            | ((light.y < 0.0) as u8) << 2
            | ((light.y > 0.0) as u8) << 3
            | ((light.z < 0.0) as u8) << 4
            | ((light.z > 0.0) as u8) << 5;
        let half = CHUNK_SIZE_I as f32 * 0.5;
        // Radio de la esfera que envuelve un chunk.
        let radius = half * 3f32.sqrt();
        let mut draw_calls = 0;
        for chunk in self.chunks.iter().filter(|chunk| chunk.scale == 1) {
            let center = ((chunk.origin - cam_chunk) * CHUNK_SIZE_I).as_vec3() + Vec3::splat(half);
            if cascade.touches(center, radius) {
                draw_calls += self.draw_faces(pass, chunk, faces);
            }
        }
        draw_calls
    }

    /// Segundo pase: lo translúcido de los chunks elegidos por `cull`, de lejos a cerca
    /// para que la mezcla salga bien entre chunks. Devuelve el número de draw calls.
    pub fn draw_translucent(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup) -> u32 {
        let mut draw_calls = 0;
        for visible in self.visible.iter().rev() {
            let chunk = &self.chunks[visible.index as usize];
            let (start, end) = chunk.transparent;
            if end <= start {
                continue;
            }
            if draw_calls == 0 {
                pass.set_pipeline(&self.translucent_pipeline);
                pass.set_bind_group(0, globals, &[]);
            }
            let offset = (chunk.slot as u64 * self.slot_stride) as u32;
            pass.set_bind_group(1, &self.chunk_bind_group, &[offset]);
            pass.set_vertex_buffer(0, chunk.buffer.slice(start as u64 * QUAD_BYTES..end as u64 * QUAD_BYTES));
            pass.draw(0..4, 0..end - start);
            draw_calls += 1;
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
