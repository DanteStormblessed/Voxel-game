//! Renderer: dispositivo wgpu, superficie y pase de dibujo.

mod entities;
mod terrain;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::{DVec3, Vec3};
use winit::window::Window;

use crate::camera::{Camera, Frustum};
use crate::mesher::ChunkMesh;
use voxel_common::block::hex_to_linear;
use voxel_common::chunk::CHUNK_SIZE;

use entities::{EntityInstance, EntityRenderer};
use crate::world::MeshKey;
use terrain::TerrainRenderer;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SKY_COLOR: u32 = 0x9CC8EC;
/// Ancho de la franja redondeada junto a cada arista, en voxeles.
const BEVEL_RADIUS: f32 = 0.12;
/// Brillo de una esquina totalmente ocluida.
const AO_MIN: f32 = 0.5;
/// Franja (en voxeles) en la que voxeles reales y terreno lejano se dibujan a la vez,
/// para que la unión entre ambos no deje rendijas.
const LOD_OVERLAP: f32 = 8.0;
/// Cuánto crece cada quad por su contorno, en píxeles de pantalla. El mallado greedy deja
/// uniones en T entre quads vecinos y, sin este solape, el rasterizador abre en ellas
/// agujeros de un píxel por los que se ve el cielo.
const QUAD_GROW_PIXELS: f32 = 0.75;

pub struct RenderSettings {
    /// Radio total visible en chunks, terreno lejano incluido (la niebla se ajusta a él).
    pub far_distance: i32,
    pub vsync: bool,
    /// Permite leer el frame de vuelta para `request_screenshot` (solo desarrollo).
    pub allow_screenshots: bool,
}

pub struct RenderStats {
    pub chunks: usize,
    pub visible: usize,
    pub quads: u64,
    pub draw_calls: u32,
}

/// Uniforms por frame. Debe coincidir con `Globals` en terrain.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    cam_chunk: [i32; 4],
    cam_local: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    sky_ambient: [f32; 4],
    ground_ambient: [f32; 4],
    fog_color: [f32; 4],
    fog_params: [f32; 4],
    params: [f32; 4],
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    palette_buffer: wgpu::Buffer,
    terrain: TerrainRenderer,
    entities: EntityRenderer,
    entity_scratch: Vec<EntityInstance>,
    fog_color: [f32; 4],
    fog_start: f32,
    fog_end: f32,
    draw_calls: u32,
    screenshot: Option<PathBuf>,
}

impl Renderer {
    pub async fn new(window: Arc<Window>, settings: &RenderSettings) -> anyhow::Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let surface = instance.create_surface(window)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await?;
        let info = adapter.get_info();
        log::info!("GPU: {} ({:?}, {})", info.name, info.backend, info.driver_info);

        // Límites "downlevel": lo que se use aquí también existe en GLES 3 / móvil.
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(caps.formats[0]);
        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if settings.allow_screenshots && caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            usage |= wgpu::TextureUsages::COPY_SRC;
        }
        let config = wgpu::SurfaceConfiguration {
            usage,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: if settings.vsync {
                wgpu::PresentMode::AutoVsync
            } else {
                wgpu::PresentMode::AutoNoVsync
            },
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let depth_view = create_depth_view(&device, &config);

        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // La paleta la rellena el servidor al conectar (`set_palette`).
        let palette_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("palette"),
            size: 256 * 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_entry = |binding: u32, visibility: wgpu::ShaderStages| {
            wgpu::BindGroupLayoutEntry {
                binding,
                visibility,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }
        };
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals layout"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                uniform_entry(1, wgpu::ShaderStages::VERTEX),
            ],
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals bind group"),
            layout: &globals_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: palette_buffer.as_entire_binding(),
                },
            ],
        });

        let terrain = TerrainRenderer::new(&device, &globals_layout, format, DEPTH_FORMAT);
        let entities = EntityRenderer::new(&device, &globals_layout, format, DEPTH_FORMAT);

        // La niebla termina justo antes del borde del terreno lejano.
        let fog_end = ((settings.far_distance - 1).max(2) * CHUNK_SIZE as i32) as f32;

        Ok(Self {
            surface,
            device,
            queue,
            config,
            depth_view,
            globals_buffer,
            globals_bind_group,
            palette_buffer,
            terrain,
            entities,
            entity_scratch: Vec::new(),
            fog_color: hex_to_linear(SKY_COLOR),
            fog_start: fog_end * 0.45,
            fog_end,
            draw_calls: 0,
            screenshot: None,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.reconfigure();
    }

    /// Reaplica la configuración de la superficie (tras un resize o si se pierde).
    pub fn reconfigure(&mut self) {
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth_view(&self.device, &self.config);
    }

    /// Colores de los bloques (RGBA lineal), indexados por número de bloque.
    pub fn set_palette(&mut self, palette: &[[f32; 4]; 256]) {
        self.queue
            .write_buffer(&self.palette_buffer, 0, bytemuck::cast_slice(&palette[..]));
    }

    /// Ajusta la niebla al radio total visible (lo concede el servidor al conectar).
    pub fn set_far_distance(&mut self, chunks: i32) {
        self.fog_end = ((chunks - 1).max(2) * CHUNK_SIZE as i32) as f32;
        self.fog_start = self.fog_end * 0.45;
    }

    /// Entidades a dibujar este frame: jugadores (posición de los ojos, yaw, id) y cajas
    /// sueltas (centro, tamaño, color) para marcadores como el bloque apuntado.
    pub fn set_entities(
        &mut self,
        camera: &Camera,
        players: impl Iterator<Item = (DVec3, f32, u32)>,
        boxes: &[(DVec3, Vec3, [f32; 3])],
    ) {
        let origin = camera.chunk_pos().as_dvec3() * CHUNK_SIZE as f64;
        self.entity_scratch.clear();
        for (eyes, yaw, id) in players {
            let rel = (eyes - origin).as_vec3();
            self.entity_scratch.extend(EntityInstance::player(rel, yaw, id));
        }
        for &(center, size, color) in boxes {
            let rel = (center - origin).as_vec3();
            self.entity_scratch.push(EntityInstance::cuboid(rel, size, color));
        }
        self.entities.set(&self.queue, &self.entity_scratch);
    }

    pub fn upload_chunk(&mut self, key: MeshKey, mesh: &ChunkMesh) {
        self.terrain.upload(&self.device, &self.queue, key, mesh);
    }

    pub fn remove_chunk(&mut self, key: MeshKey) {
        self.terrain.remove(key);
    }

    /// Guarda el siguiente frame como imagen PPM. Herramienta de desarrollo.
    pub fn request_screenshot(&mut self, path: PathBuf) {
        if self.config.usage.contains(wgpu::TextureUsages::COPY_SRC) {
            self.screenshot = Some(path);
        } else {
            log::warn!("screenshots unavailable: the surface does not allow reading the frame back");
        }
    }

    pub fn stats(&self) -> RenderStats {
        RenderStats {
            chunks: self.terrain.chunk_count(),
            visible: self.terrain.visible_count(),
            quads: self.terrain.total_quads(),
            draw_calls: self.draw_calls,
        }
    }

    /// `clip` es la distancia horizontal (en voxeles) hasta la que se dibujan los voxeles
    /// reales; a partir de ahí toma el relevo el terreno lejano.
    pub fn render(&mut self, camera: &Camera, clip: f32) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let aspect = self.config.width as f32 / self.config.height as f32;
        let view_proj = camera.view_proj(aspect);
        let cam_chunk = camera.chunk_pos();
        let cam_local = camera.local_pos();
        let sun_dir = Vec3::new(0.45, 0.8, 0.35).normalize();
        // Ángulo que cubre un píxel: tamaño en mundo de un píxel a distancia 1.
        let pixel_angle = 2.0 * (camera.fov_y * 0.5).tan() / self.config.height as f32;

        let globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            cam_chunk: [cam_chunk.x, cam_chunk.y, cam_chunk.z, 0],
            cam_local: [cam_local.x, cam_local.y, cam_local.z, 0.0],
            sun_dir: [sun_dir.x, sun_dir.y, sun_dir.z, 0.0],
            sun_color: [0.78, 0.72, 0.62, 0.0],
            sky_ambient: [0.38, 0.44, 0.55, 0.0],
            ground_ambient: [0.20, 0.19, 0.17, 0.0],
            fog_color: self.fog_color,
            fog_params: [
                self.fog_start,
                1.0 / (self.fog_end - self.fog_start),
                LOD_OVERLAP,
                0.0,
            ],
            params: [BEVEL_RADIUS, AO_MIN, pixel_angle * QUAD_GROW_PIXELS, clip],
        };
        self.queue
            .write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));

        self.terrain
            .cull(
                &Frustum::from_view_proj(view_proj),
                cam_chunk,
                cam_local,
                clip,
                LOD_OVERLAP,
            );

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let [r, g, b, _] = self.fog_color;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: r as f64,
                            g: g as f64,
                            b: b as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        // Z invertida: el fondo es 0.
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            self.draw_calls = self.terrain.draw(&mut pass, &self.globals_bind_group);
            self.entities.draw(&mut pass, &self.globals_bind_group);
        }

        let capture = self.screenshot.take().map(|path| {
            let bytes_per_row =
                (self.config.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("screenshot"),
                size: bytes_per_row as u64 * self.config.height as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &frame.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: self.config.width,
                    height: self.config.height,
                    depth_or_array_layers: 1,
                },
            );
            (path, buffer, bytes_per_row as usize)
        });

        self.queue.submit(Some(encoder.finish()));
        if let Some((path, buffer, bytes_per_row)) = capture {
            match self.save_capture(&path, &buffer, bytes_per_row) {
                Ok(()) => log::info!("screenshot saved to {}", path.display()),
                Err(error) => log::error!("failed to save the screenshot: {error:#}"),
            }
        }
        frame.present();
        Ok(())
    }

    fn save_capture(
        &self,
        path: &Path,
        buffer: &wgpu::Buffer,
        bytes_per_row: usize,
    ) -> anyhow::Result<()> {
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::Wait)?;
        let data = slice.get_mapped_range();

        let (width, height) = (self.config.width as usize, self.config.height as usize);
        let bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
        out.reserve(width * height * 3);
        for row in data.chunks(bytes_per_row).take(height) {
            for pixel in row[..width * 4].chunks_exact(4) {
                let rgb = if bgra {
                    [pixel[2], pixel[1], pixel[0]]
                } else {
                    [pixel[0], pixel[1], pixel[2]]
                };
                out.extend_from_slice(&rgb);
            }
        }
        std::fs::write(path, out)?;
        Ok(())
    }
}

fn create_depth_view(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}
