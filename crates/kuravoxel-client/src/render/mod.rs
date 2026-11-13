//! Renderer: dispositivo wgpu, superficie y pase de dibujo.

mod entities;
mod items;
mod terrain;

pub use items::ItemInstance;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::{DVec3, Vec3};
use winit::window::Window;

use crate::camera::{Camera, Frustum};
use crate::mesher::{ChunkMesh, MeshTable};
use kuravoxel_common::content::{ContentData, ItemId};
use kuravoxel_common::shape::BlockTable;
use kuravoxel_common::block::hex_to_linear;
use kuravoxel_common::chunk::CHUNK_SIZE;

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
/// Radio del "planeta", en metros. El terreno baja `d² / 2R` con la distancia horizontal
/// a la cámara: las montañas lejanas se hunden un poco y el borde del terreno cargado
/// cae hacia el horizonte. Sutil a propósito (a 5 km baja ~60 m); más pequeño parece un
/// planeta diminuto.
const CURVATURE_RADIUS: f32 = 200_000.0;
/// Perspectiva aérea: cuánto se tiñe del color del cielo lo que está en el borde del
/// terreno visible, sin contar el fundido final. Lo cercano apenas cambia.
const HAZE_STRENGTH: f32 = 0.45;
/// Fracción de la distancia visible a partir de la que el terreno se funde del todo con
/// el cielo, para que el borde de lo cargado no se vea como un corte.
const HAZE_EDGE: f32 = 0.86;

fn present_mode(vsync: bool) -> wgpu::PresentMode {
    if vsync {
        wgpu::PresentMode::AutoVsync
    } else {
        wgpu::PresentMode::AutoNoVsync
    }
}

pub struct RenderSettings {
    /// Radio total visible en chunks, terreno lejano incluido (la bruma se ajusta a él).
    pub far_distance: i32,
    pub vsync: bool,
    /// Permite leer el frame de vuelta para `request_screenshot` (solo desarrollo).
    pub allow_screenshots: bool,
}

/// Qué se dibuja de cada forma y desde dónde se mide la bruma.
#[derive(Clone, Copy, Debug)]
pub struct SceneView {
    /// Distancia horizontal a la cámara (m) hasta la que se dibujan voxeles reales.
    pub clip: f32,
    /// Alturas (m) entre las que hay voxeles reales; fuera, dibuja el terreno lejano.
    pub voxel_band: [f64; 2],
    /// Punto en el que se centra la vista (el jugador): la bruma se mide desde ahí, así
    /// que en la vista de mapa el terreno del jugador se ve nítido aunque la cámara esté
    /// a kilómetros.
    pub focus: DVec3,
}

impl SceneView {
    /// Sin voxeles reales: todo es terreno lejano.
    pub fn empty(focus: DVec3) -> Self {
        Self { clip: 0.0, voxel_band: [0.0, 0.0], focus }
    }
}

/// Qué ha pasado con un frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameStatus {
    Presented,
    /// La superficie ya no vale (cambio de tamaño, etc.): hay que llamar a `reconfigure`.
    Reconfigure,
    /// No había dónde dibujar (ventana oculta, espera agotada): se prueba en el siguiente.
    Skipped,
}

/// Dispositivo gráfico del motor, prestado a quien dibuja encima de la escena.
#[derive(Clone, Copy)]
pub struct Gpu<'a> {
    pub adapter: &'a wgpu::Adapter,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// Formato de la imagen final (la superficie de la ventana).
    pub format: wgpu::TextureFormat,
}

/// Tamaño en píxeles de la imagen sobre la que se dibuja la interfaz.
#[derive(Clone, Copy)]
pub struct OverlayTarget {
    pub width: u32,
    pub height: u32,
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
    focus: [f32; 4],
    voxel_band: [f32; 4],
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    globals_buffer: wgpu::Buffer,
    globals_layout: wgpu::BindGroupLayout,
    globals_bind_group: wgpu::BindGroup,
    /// Color de cada número de bloque: textura de `PALETTE_WIDTH` × filas, que crece si
    /// el contenido no cabe.
    palette: wgpu::Texture,
    terrain: TerrainRenderer,
    entities: EntityRenderer,
    items: items::ItemRenderer,
    entity_scratch: Vec<EntityInstance>,
    fog_color: [f32; 4],
    /// Distancia (m) hasta la que hay terreno: ahí la bruma termina de fundirlo con el cielo.
    haze_distance: f32,
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
            present_mode: present_mode(settings.vsync),
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
        let palette = create_palette(&device, 1);

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
                // Leer una textura en el vertex shader está dentro de los límites de GLES 3.
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let globals_bind_group =
            create_globals_bind_group(&device, &globals_layout, &globals_buffer, &palette);

        let terrain = TerrainRenderer::new(&device, &globals_layout, format, DEPTH_FORMAT);
        let entities = EntityRenderer::new(&device, &globals_layout, format, DEPTH_FORMAT);
        let items = items::ItemRenderer::new(&device, &globals_layout, format, DEPTH_FORMAT);


        Ok(Self {
            surface,
            adapter,
            device,
            queue,
            config,
            depth_view,
            globals_buffer,
            globals_layout,
            globals_bind_group,
            palette,
            terrain,
            entities,
            items,
            entity_scratch: Vec::new(),
            fog_color: hex_to_linear(SKY_COLOR),
            haze_distance: haze_distance(settings.far_distance),
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

    /// Activa o desactiva la sincronización vertical sin recrear nada más.
    pub fn set_vsync(&mut self, vsync: bool) {
        let mode = present_mode(vsync);
        if self.config.present_mode != mode {
            self.config.present_mode = mode;
            self.reconfigure();
        }
    }

    /// Acceso al dispositivo gráfico para quien dibuje encima de la escena (la interfaz).
    pub fn gpu(&self) -> Gpu<'_> {
        Gpu {
            adapter: &self.adapter,
            device: &self.device,
            queue: &self.queue,
            format: self.config.format,
        }
    }

    /// Olvida todo lo que se estaba dibujando del mundo (al salir de una partida).
    pub fn clear_world(&mut self) {
        self.terrain.clear();
        self.entities.set(&self.queue, &[]);
        self.items.clear();
    }

    /// Paleta de GPU (RGBA8 sRGB): ver `mesher::build_tables`.
    pub fn set_palette(&mut self, palette: &[[u8; 4]]) {
        let rows = (palette.len() as u32).div_ceil(PALETTE_WIDTH).max(1);
        if rows > self.palette.height() {
            self.palette = create_palette(&self.device, rows);
            self.globals_bind_group = create_globals_bind_group(
                &self.device,
                &self.globals_layout,
                &self.globals_buffer,
                &self.palette,
            );
        }
        let mut texels = palette.to_vec();
        texels.resize((rows * PALETTE_WIDTH) as usize, [0; 4]);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.palette,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(PALETTE_WIDTH * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d { width: PALETTE_WIDTH, height: rows, depth_or_array_layers: 1 },
        );
    }

    /// Ajusta la bruma al radio total visible (lo concede el servidor al conectar).
    pub fn set_far_distance(&mut self, chunks: i32) {
        self.haze_distance = haze_distance(chunks);
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

    /// Formas 3D de los ítems del contenido, para dibujarlos sueltos.
    pub fn set_item_shapes(&mut self, content: &ContentData, blocks: &BlockTable, mesh: &MeshTable) {
        self.items.set_shapes(&self.device, content, blocks, mesh);
    }

    /// Ítems sueltos de este frame: (ítem, centro de su base, giro).
    pub fn set_drops(&mut self, camera: &Camera, drops: impl Iterator<Item = (ItemId, DVec3, f32)>) {
        let origin = camera.chunk_pos().as_dvec3() * CHUNK_SIZE as f64;
        let instances: Vec<ItemInstance> = drops
            .map(|(item, base, spin)| ItemInstance { item, base: (base - origin).as_vec3(), spin })
            .collect();
        self.items.set(&self.queue, &instances);
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

    /// `scene` dice dónde hay voxeles reales (el resto lo dibuja el terreno lejano) y
    /// desde dónde se mide la bruma.
    ///
    /// `overlay` se llama con la escena ya dibujada y recibe la imagen final: es donde la
    /// interfaz pinta encima. El renderer no sabe nada de interfaces.
    pub fn render(
        &mut self,
        camera: &Camera,
        scene: &SceneView,
        overlay: &mut dyn FnMut(&wgpu::TextureView, OverlayTarget),
    ) -> FrameStatus {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                return FrameStatus::Reconfigure;
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                log::error!("the GPU is out of memory");
                return FrameStatus::Skipped;
            }
            Err(_) => return FrameStatus::Skipped,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let aspect = self.config.width as f32 / self.config.height as f32;
        let view_proj = camera.view_proj(aspect);
        let cam_chunk = camera.chunk_pos();
        let cam_local = camera.local_pos();
        let origin = cam_chunk.as_dvec3() * CHUNK_SIZE as f64;
        let focus = (scene.focus - origin).as_vec3();
        let clip = scene.clip;
        let band = [
            (scene.voxel_band[0] - origin.y) as f32,
            (scene.voxel_band[1] - origin.y) as f32,
        ];
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
                self.haze_distance,
                1.0 / (2.0 * CURVATURE_RADIUS),
                LOD_OVERLAP,
                HAZE_EDGE,
            ],
            params: [BEVEL_RADIUS, AO_MIN, pixel_angle * QUAD_GROW_PIXELS, clip],
            focus: [focus.x, focus.y, focus.z, HAZE_STRENGTH],
            voxel_band: [band[0], band[1], 0.0, 0.0],
        };
        self.queue
            .write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));

        self.terrain
            .cull(
                &Frustum::from_view_proj(view_proj),
                cam_chunk,
                cam_local,
                clip,
                band,
                LOD_OVERLAP,
                CURVATURE_RADIUS,
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
            self.draw_calls += self.items.draw(&mut pass, &self.globals_bind_group);
            // Lo translúcido al final, sobre todo lo opaco (jugadores incluidos).
            self.draw_calls += self.terrain.draw_translucent(&mut pass, &self.globals_bind_group);
        }

        // La escena se envía antes de que la interfaz pinte encima con sus propios comandos.
        self.queue.submit(Some(encoder.finish()));
        overlay(&view, OverlayTarget { width: self.config.width, height: self.config.height });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
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
        FrameStatus::Presented
    }

    fn save_capture(
        &self,
        path: &Path,
        buffer: &wgpu::Buffer,
        bytes_per_row: usize,
    ) -> anyhow::Result<()> {
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
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

/// Ancho de la textura de paleta: la entrada `n` está en (n % ancho, n / ancho). Con 8
/// entradas por bloque, 65 536 bloques caben en 256 filas. Debe coincidir con el shader.
const PALETTE_WIDTH: u32 = 2048;

fn create_palette(device: &wgpu::Device, rows: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("palette"),
        size: wgpu::Extent3d { width: PALETTE_WIDTH, height: rows, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // sRGB: el shader recibe el color ya en lineal, que es como ilumina.
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn create_globals_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    palette: &wgpu::Texture,
) -> wgpu::BindGroup {
    let view = palette.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("globals bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
        ],
    })
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

/// Distancia de la bruma para un radio visible de `chunks`: termina justo antes del borde
/// del terreno lejano.
fn haze_distance(chunks: i32) -> f32 {
    ((chunks - 1).max(2) * CHUNK_SIZE as i32) as f32
}
