//! Renderer: dispositivo wgpu, superficie y pase de dibujo.

mod entities;
mod environment;
mod gpu_timer;
mod items;
mod shadows;
mod terrain;

pub use environment::{sun_direction, Environment, Lighting, ShadowQuality, SkyColors};
pub use items::ItemInstance;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::{DVec3, Vec3};
use winit::window::Window;

use crate::camera::{Camera, Frustum};
use crate::mesher::{ChunkMesh, MeshTable};
use kuravoxel_common::content::{ContentData, ItemId};
use kuravoxel_common::shape::BlockTable;
use kuravoxel_common::chunk::CHUNK_SIZE;

use entities::{EntityInstance, EntityRenderer};
use gpu_timer::GpuTimer;
use shadows::{ShadowGlobals, ShadowMaps, CASCADES};
use crate::world::MeshKey;
use terrain::TerrainRenderer;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Exponente del degradado del cielo: cuanto mayor, más fina la franja clara del horizonte.
const SKY_GRADIENT: f32 = 2.6;
/// Hora que se ve cuando no hay partida (el fondo de los menús).
const IDLE_TIME: f32 = 10.0;
/// Radio de voxeles reales (m) por debajo del cual las sombras se van apagando: camino de
/// la vista de mapa ya no hay voxeles que las proyecten.
const SHADOW_MIN_CLIP: f32 = 96.0;
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

/// Rectángulo en píxeles (x, y, ancho, alto) de una región en fracciones de la ventana;
/// `None` si queda vacía (no hay nada que recortar: se dibuja todo).
fn scissor_rect([x0, y0, x1, y1]: [f32; 4], width: u32, height: u32) -> Option<[u32; 4]> {
    let px = |fraction: f32, size: u32| ((fraction * size as f32).round() as u32).min(size);
    let (left, top) = (px(x0, width), px(y0, height));
    let (right, bottom) = (px(x1, width), px(y1, height));
    (right > left && bottom > top).then(|| [left, top, right - left, bottom - top])
}

/// Matriz que lleva la región `[x0, y0, x1, y1]` de la pantalla a todo el espacio de
/// recorte: multiplicada delante de la de vista y proyección da la pirámide de esa zona.
fn region_crop([x0, y0, x1, y1]: [f32; 4]) -> glam::Mat4 {
    // En el espacio de recorte la X va de -1 a 1 hacia la derecha y la Y hacia arriba.
    let (left, right) = (x0 * 2.0 - 1.0, x1 * 2.0 - 1.0);
    let (bottom, top) = (1.0 - y1 * 2.0, 1.0 - y0 * 2.0);
    let half = glam::Vec2::new((right - left).max(1e-4) * 0.5, (top - bottom).max(1e-4) * 0.5);
    let center = glam::Vec2::new((right + left) * 0.5, (top + bottom) * 0.5);
    glam::Mat4::from_cols(
        glam::Vec4::new(1.0 / half.x, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, 1.0 / half.y, 0.0, 0.0),
        glam::Vec4::Z,
        glam::Vec4::new(-center.x / half.x, -center.y / half.y, 0.0, 1.0),
    )
}

/// Un mapa de sombras dibujado: con qué cascada y para qué escena (versión de las mallas,
/// de los jugadores y chunk de la cámara, al que van referidas las posiciones).
#[derive(Clone, Copy)]
struct CachedCascade {
    cascade: shadows::Cascade,
    scene: (u64, u64, glam::IVec3),
}

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
    pub shadows: ShadowQuality,
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
    /// Hora del día (0-24): de ella salen el sol, la luna y los colores del cielo.
    pub time: f32,
    /// Edad de la luna (0 = nueva, 0,5 = llena): lo oscura que es la noche.
    pub moon_age: f32,
}

impl SceneView {
    /// Sin voxeles reales: todo es terreno lejano.
    pub fn empty(focus: DVec3) -> Self {
        Self { clip: 0.0, voxel_band: [0.0, 0.0], focus, time: IDLE_TIME, moon_age: 0.5 }
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
    /// Draw calls de los mapas de sombras (aparte de `draw_calls`).
    pub shadow_draw_calls: u32,
}

/// Uniforms por frame. Debe coincidir con `Globals` en common.wgsl.
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
    sky_zenith: [f32; 4],
    sky_glow: [f32; 4],
    sun_pos: [f32; 4],
    sun_disc: [f32; 4],
    moon_disc: [f32; 4],
    moon_phase: [f32; 4],
    cam_right: [f32; 4],
    cam_up: [f32; 4],
    cam_forward: [f32; 4],
    shadow_mats: [[[f32; 4]; 4]; CASCADES],
    shadow_params: [f32; 4],
    shadow_texel: [f32; 4],
}

/// Shader de la escena: los uniforms, la luz y las tablas de caras comunes van delante.
fn scene_shader(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    let source =
        [include_str!("shaders/common.wgsl"), include_str!("shaders/faces.wgsl"), source].join("\n");
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

/// Shader de las pasadas de sombras.
fn shadow_shader(device: &wgpu::Device) -> wgpu::ShaderModule {
    let source = [include_str!("shaders/faces.wgsl"), include_str!("shaders/shadow.wgsl")].join("\n");
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shadow shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

pub struct Renderer {
    /// Se conserva para crear las superficies de otras ventanas (`create_window_surface`).
    instance: wgpu::Instance,
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
    /// Marcas del juego (selección): translúcidas y visibles a través de los bloques.
    marks: EntityRenderer,
    items: items::ItemRenderer,
    entity_scratch: Vec<EntityInstance>,
    /// Los jugadores (lo que da sombra) del último frame, y cuántas veces han cambiado.
    casters: Vec<EntityInstance>,
    caster_version: u64,
    /// Cada mapa de sombras tal como se dibujó: se reutiliza mientras valga.
    shadow_cache: [Option<CachedCascade>; CASCADES],
    sky_pipeline: wgpu::RenderPipeline,
    shadow_layout: wgpu::BindGroupLayout,
    shadows: ShadowMaps,
    shadow_quality: ShadowQuality,
    environment: Environment,
    /// Draw calls de las pasadas de sombras del último frame.
    shadow_draw_calls: u32,
    /// Distancia (m) hasta la que hay terreno: ahí la bruma termina de fundirlo con el cielo.
    haze_distance: f32,
    draw_calls: u32,
    screenshot: Option<PathBuf>,
    /// Desarrollo: tiempo de GPU por pase (`KURAVOXEL_GPU_TIMING=1`).
    timer: Option<GpuTimer>,
    /// Parte de la ventana que se ve (fracciones `[x0, y0, x1, y1]`); `None` = toda.
    scene_region: Option<[f32; 4]>,
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

        // Límites "downlevel": lo que se use aquí también existe en GLES 3 / móvil. Las
        // marcas de tiempo solo se piden para medir (desarrollo).
        let timing = std::env::var("KURAVOXEL_GPU_TIMING").is_ok_and(|value| value != "0");
        let required_features =
            if timing { GpuTimer::features(&adapter) } else { wgpu::Features::empty() };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("device"),
                required_features,
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
                // Mapas de sombras: una capa por cascada, leídas con comparación.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let shadow_layout = shadows::create_layout(&device);
        let shadows = ShadowMaps::new(&device, &shadow_layout, settings.shadows.resolution());
        let globals_bind_group =
            create_globals_bind_group(&device, &globals_layout, &globals_buffer, &palette, &shadows);

        let terrain = TerrainRenderer::new(&device, &globals_layout, &shadow_layout, format, DEPTH_FORMAT);
        let entities =
            EntityRenderer::new(&device, &globals_layout, &shadow_layout, format, DEPTH_FORMAT, false);
        let marks = EntityRenderer::new(&device, &globals_layout, &shadow_layout, format, DEPTH_FORMAT, true);
        let items = items::ItemRenderer::new(&device, &globals_layout, format, DEPTH_FORMAT);
        let sky_pipeline = create_sky_pipeline(&device, &globals_layout, format);


        let timer = if timing { GpuTimer::new(&device, &queue) } else { None };
        Ok(Self {
            instance,
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
            marks,
            items,
            entity_scratch: Vec::new(),
            casters: Vec::new(),
            caster_version: 0,
            shadow_cache: [None; CASCADES],
            sky_pipeline,
            shadow_layout,
            shadows,
            shadow_quality: settings.shadows,
            environment: Environment::default(),
            shadow_draw_calls: 0,
            haze_distance: haze_distance(settings.far_distance),
            draw_calls: 0,
            screenshot: None,
            timer,
            scene_region: None,
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

    /// Cambia la calidad de las sombras (recrea sus mapas).
    pub fn set_shadow_quality(&mut self, quality: ShadowQuality) {
        if quality == self.shadow_quality {
            return;
        }
        self.shadow_quality = quality;
        self.shadows = ShadowMaps::new(&self.device, &self.shadow_layout, quality.resolution());
        self.shadow_cache = [None; CASCADES];
        self.rebind_globals();
    }

    /// Ajustes del ciclo de día y noche y de las sombras; valen desde el siguiente frame.
    /// Limita el dibujo de la escena a una parte de la ventana (ver `Command::SetSceneRegion`).
    pub fn set_scene_region(&mut self, region: Option<[f32; 4]>) {
        self.scene_region = region.map(|[x0, y0, x1, y1]| {
            let x0 = x0.clamp(0.0, 1.0);
            let y0 = y0.clamp(0.0, 1.0);
            [x0, y0, x1.clamp(x0, 1.0), y1.clamp(y0, 1.0)]
        });
    }

    pub fn set_environment(&mut self, environment: Environment) {
        self.environment = environment.sanitized();
    }

    fn rebind_globals(&mut self) {
        self.globals_bind_group = create_globals_bind_group(
            &self.device,
            &self.globals_layout,
            &self.globals_buffer,
            &self.palette,
            &self.shadows,
        );
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
        self.entities.set(&self.queue, &[], 0);
        self.marks.set(&self.queue, &[], 0);
        self.items.clear();
    }

    /// Paleta de GPU (RGBA8 sRGB): ver `mesher::build_tables`.
    pub fn set_palette(&mut self, palette: &[[u8; 4]]) {
        let rows = (palette.len() as u32).div_ceil(PALETTE_WIDTH).max(1);
        if rows > self.palette.height() {
            self.palette = create_palette(&self.device, rows);
            self.rebind_globals();
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
        // Solo los jugadores dan sombra: lo demás son marcas (el bloque apuntado, la
        // vista previa de un prefab).
        let casters = self.entity_scratch.len();
        let moved: &[u8] = bytemuck::cast_slice(&self.entity_scratch[..casters]);
        if moved != bytemuck::cast_slice::<EntityInstance, u8>(&self.casters) {
            self.casters.clear();
            self.casters.extend_from_slice(&self.entity_scratch[..casters]);
            self.caster_version += 1;
        }
        for &(center, size, color) in boxes {
            let rel = (center - origin).as_vec3();
            self.entity_scratch.push(EntityInstance::cuboid(rel, size, color));
        }
        self.entities.set(&self.queue, &self.entity_scratch, casters);
    }

    /// Marcas de este frame: cajas (centro, tamaño, color con opacidad, con rejilla o
    /// sin ella) que se dibujan encima de todo, también de lo que las tapa.
    pub fn set_marks(&mut self, camera: &Camera, boxes: &[(DVec3, Vec3, [f32; 4], bool)]) {
        let origin = camera.chunk_pos().as_dvec3() * CHUNK_SIZE as f64;
        let instances: Vec<EntityInstance> = boxes
            .iter()
            .map(|&(center, size, color, grid)| {
                EntityInstance::panel((center - origin).as_vec3(), size, color, grid)
            })
            .collect();
        self.marks.set(&self.queue, &instances, 0);
    }

    /// Ancho entre alto de la imagen.
    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
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
            shadow_draw_calls: self.shadow_draw_calls,
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
        // Ángulo que cubre un píxel: tamaño en mundo de un píxel a distancia 1.
        let pixel_angle = 2.0 * (camera.fov_y * 0.5).tan() / self.config.height as f32;

        // --- Luz y cielo de esta hora -------------------------------------------------
        let environment = &self.environment;
        let light = Lighting::at(environment, scene.time, scene.moon_age);
        let [hr, hg, hb] = light.horizon;
        let rgb = |[r, g, b]: [f32; 3], w: f32| [r, g, b, w];
        // Ejes de la cámara para el rayo de cada píxel del cielo.
        let forward = camera.forward();
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward);
        let tan_y = (camera.fov_y * 0.5).tan();

        // --- Sombras ------------------------------------------------------------------
        // Sin voxeles reales (vista de mapa) no hay quien las proyecte.
        let shadow_strength = if self.shadow_quality == ShadowQuality::Off {
            0.0
        } else {
            environment.shadow_strength * light.light_fade * (clip / SHADOW_MIN_CLIP).clamp(0.0, 1.0)
        };
        let distance = environment.shadow_distance.min(clip.max(1.0));
        let wanted =
            shadows::cascades(camera, light.light_dir, light.axis, distance, self.shadows.resolution);
        // Un mapa ya dibujado vale mientras no cambie lo que da sombra y la cascada apenas
        // se mueva: quieto (o con el sol avanzando despacio) no se redibuja nada.
        let scene = (self.terrain.shadow_version(), self.caster_version, cam_chunk);
        let mut cascades = wanted;
        let mut redraw = [false; CASCADES];
        if shadow_strength > 0.0 {
            for (i, cached) in self.shadow_cache.iter_mut().enumerate() {
                match cached {
                    Some(old) if old.scene == scene && old.cascade.can_stand_in_for(&wanted[i]) => {
                        cascades[i] = old.cascade;
                    }
                    _ => {
                        *cached = Some(CachedCascade { cascade: wanted[i], scene });
                        redraw[i] = true;
                    }
                }
            }
        }
        let curvature = 1.0 / (2.0 * CURVATURE_RADIUS);

        let globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            cam_chunk: [cam_chunk.x, cam_chunk.y, cam_chunk.z, 0],
            cam_local: [cam_local.x, cam_local.y, cam_local.z, 0.0],
            sun_dir: light.light_dir.extend(shadow_strength).to_array(),
            sun_color: rgb(light.light_color, 0.0),
            sky_ambient: rgb(light.sky_ambient, 0.0),
            ground_ambient: rgb(light.ground_ambient, 0.0),
            fog_color: rgb(light.horizon, 1.0),
            fog_params: [self.haze_distance, curvature, LOD_OVERLAP, HAZE_EDGE],
            params: [BEVEL_RADIUS, AO_MIN, pixel_angle * QUAD_GROW_PIXELS, clip],
            focus: [focus.x, focus.y, focus.z, HAZE_STRENGTH],
            voxel_band: [band[0], band[1], 0.0, 0.0],
            sky_zenith: rgb(light.zenith, SKY_GRADIENT),
            sky_glow: light.glow,
            sun_pos: light.sun.extend(0.0).to_array(),
            sun_disc: light.sun_disc,
            moon_disc: light.moon_disc,
            moon_phase: light.moon_phase,
            cam_right: (right * tan_y * aspect).extend(0.0).to_array(),
            cam_up: (up * tan_y).extend(0.0).to_array(),
            cam_forward: forward.extend(0.0).to_array(),
            shadow_mats: cascades.map(|cascade| cascade.view_proj.to_cols_array_2d()),
            shadow_params: [
                1.0 / self.shadows.resolution as f32,
                self.shadow_quality.filter_half() as f32,
                environment.shadow_softness,
                shadows::depth_bias(distance),
            ],
            shadow_texel: [cascades[0].texel, cascades[1].texel, cascades[2].texel, 0.0],
        };
        self.queue
            .write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));

        // Con una pantalla opaca delante solo se dibuja la parte que queda a la vista: el
        // descarte usa el trozo de pirámide de visión de esa zona y el pase la recorta.
        let scissor = self.scene_region.and_then(|region| scissor_rect(region, self.config.width, self.config.height));
        let cull_view_proj = match self.scene_region {
            Some(region) => region_crop(region) * view_proj,
            None => view_proj,
        };
        self.terrain
            .cull(
                &Frustum::from_view_proj(cull_view_proj),
                cam_chunk,
                cam_local,
                clip,
                band,
                LOD_OVERLAP,
                CURVATURE_RADIUS,
            );

        if let Some(timer) = self.timer.as_mut() {
            timer.begin_frame(&self.device);
        }
        let timer = self.timer.as_ref();

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        // Mapas de sombras: la escena desde la luz, una pasada por cascada.
        self.shadow_draw_calls = 0;
        if shadow_strength > 0.0 {
            for (index, cascade) in cascades.iter().enumerate() {
                if !redraw[index] {
                    continue;
                }
                if let Some(timer) = timer {
                    timer.note_cascade(index);
                }
                self.shadows.write(
                    &self.queue,
                    index,
                    &ShadowGlobals {
                        light_view_proj: cascade.view_proj.to_cols_array_2d(),
                        cam_chunk: [cam_chunk.x, cam_chunk.y, cam_chunk.z, 0],
                        cam_local: [cam_local.x, cam_local.y, cam_local.z, curvature],
                    },
                );
                let bind_group = self.shadows.bind_group(index);
                let begin = gpu_timer::SHADOW_BEGIN + index as u32 * 2;
                let writes = timer.and_then(|timer| timer.pass(begin, Some(begin + 1)));
                let mut pass = self.shadows.begin(&mut encoder, index, writes);
                self.shadow_draw_calls +=
                    self.terrain.draw_shadow(&mut pass, bind_group, cam_chunk, cascade, light.light_dir);
                self.entities.draw_shadow(&mut pass, bind_group);
            }
        }
        {
            let (r, g, b) = (hr, hg, hb);
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
                timestamp_writes: timer
                    .and_then(|timer| timer.pass(gpu_timer::MAIN_BEGIN, Some(gpu_timer::MAIN_END))),
                ..Default::default()
            });
            if let Some([x, y, width, height]) = scissor {
                pass.set_scissor_rect(x, y, width, height);
            }
            self.draw_calls = self.terrain.draw(&mut pass, &self.globals_bind_group);
            self.entities.draw(&mut pass, &self.globals_bind_group);
            self.draw_calls += self.items.draw(&mut pass, &self.globals_bind_group);
            if let Some(timer) = timer {
                timer.mark(&mut pass, gpu_timer::AFTER_OPAQUE);
            }
            // El cielo, solo donde no se ha dibujado nada.
            pass.set_pipeline(&self.sky_pipeline);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.draw(0..3, 0..1);
            if let Some(timer) = timer {
                timer.mark(&mut pass, gpu_timer::AFTER_SKY);
            }
            // Lo translúcido al final, sobre todo lo opaco (jugadores incluidos).
            self.draw_calls += self.terrain.draw_translucent(&mut pass, &self.globals_bind_group);
            self.marks.draw(&mut pass, &self.globals_bind_group);
        }

        // La escena se envía antes de que la interfaz pinte encima con sus propios comandos.
        self.queue.submit(Some(encoder.finish()));
        overlay(&view, OverlayTarget { width: self.config.width, height: self.config.height });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        if let Some(timer) = self.timer.as_ref() {
            // Un pase vacío marca cuándo ha terminado la interfaz.
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gpu timer: end of ui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                timestamp_writes: timer.pass(gpu_timer::UI_END, None),
                ..Default::default()
            });
            timer.resolve(&mut encoder);
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
        if let Some(timer) = self.timer.as_mut() {
            timer.after_submit();
        }
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
        save_capture(&self.device, &self.config, path, buffer, bytes_per_row)
    }

    /// Superficie para dibujar en otra ventana del juego con el mismo dispositivo. Usa el
    /// mismo formato de imagen que la ventana principal si puede (así la interfaz sirve
    /// igual en las dos).
    pub fn create_window_surface(&self, window: Arc<Window>) -> anyhow::Result<WindowSurface> {
        let size = window.inner_size();
        let surface = self.instance.create_surface(window)?;
        let caps = surface.get_capabilities(&self.adapter);
        let format = if caps.formats.contains(&self.config.format) {
            self.config.format
        } else {
            caps.formats.iter().copied().find(|format| format.is_srgb()).unwrap_or(caps.formats[0])
        };
        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if self.config.usage.contains(wgpu::TextureUsages::COPY_SRC) && caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            usage |= wgpu::TextureUsages::COPY_SRC;
        }
        let config = wgpu::SurfaceConfiguration {
            usage,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            // Una ventana de herramientas no necesita más frames que el monitor.
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&self.device, &config);
        Ok(WindowSurface { surface, config, screenshot: None })
    }

    /// El dispositivo, con el formato de imagen de otra superficie.
    pub fn gpu_for(&self, surface: &WindowSurface) -> Gpu<'_> {
        Gpu { format: surface.config.format, ..self.gpu() }
    }
}

/// Lee una captura ya copiada a `buffer` y la guarda como PPM.
fn save_capture(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    path: &Path,
    buffer: &wgpu::Buffer,
    bytes_per_row: usize,
) -> anyhow::Result<()> {
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely())?;
    let data = slice.get_mapped_range();

    let (width, height) = (config.width as usize, config.height as usize);
    let bgra = matches!(config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
    let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
    out.reserve(width * height * 3);
    for row in data.chunks(bytes_per_row).take(height) {
        for pixel in row[..width * 4].chunks_exact(4) {
            let rgb = if bgra { [pixel[2], pixel[1], pixel[0]] } else { [pixel[0], pixel[1], pixel[2]] };
            out.extend_from_slice(&rgb);
        }
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// Copia la imagen de un frame a un buffer legible desde la CPU.
fn copy_frame(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    config: &wgpu::SurfaceConfiguration,
) -> (wgpu::Buffer, usize) {
    let bytes_per_row = (config.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot"),
        size: bytes_per_row as u64 * config.height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: None },
        },
        wgpu::Extent3d { width: config.width, height: config.height, depth_or_array_layers: 1 },
    );
    (buffer, bytes_per_row as usize)
}

/// Otra ventana del juego (un editor): una superficie sobre el mismo dispositivo, sin
/// escena. Se limpia a un color y encima pinta quien la usa (su interfaz).
pub struct WindowSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    screenshot: Option<PathBuf>,
}

impl WindowSurface {
    pub fn resize(&mut self, renderer: &Renderer, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&renderer.device, &self.config);
    }

    /// Guarda el siguiente frame de esta ventana como PPM (desarrollo).
    pub fn request_screenshot(&mut self, path: PathBuf) {
        if self.config.usage.contains(wgpu::TextureUsages::COPY_SRC) {
            self.screenshot = Some(path);
        } else {
            log::warn!("screenshots unavailable for this window: the surface does not allow reading the frame back");
        }
    }

    /// Dibuja un frame: fondo liso y, encima, lo que pinte `draw`.
    pub fn render(
        &mut self,
        renderer: &Renderer,
        clear: [f64; 3],
        draw: &mut dyn FnMut(&wgpu::TextureView, OverlayTarget),
    ) -> FrameStatus {
        let device = &renderer.device;
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(device, &self.config);
                return FrameStatus::Reconfigure;
            }
            Err(_) => return FrameStatus::Skipped,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let [r, g, b] = clear;
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("window clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        renderer.queue.submit(Some(encoder.finish()));
        draw(&view, OverlayTarget { width: self.config.width, height: self.config.height });
        if let Some(path) = self.screenshot.take() {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            let (buffer, bytes_per_row) = copy_frame(device, &mut encoder, &frame.texture, &self.config);
            renderer.queue.submit(Some(encoder.finish()));
            match save_capture(device, &self.config, &path, &buffer, bytes_per_row) {
                Ok(()) => log::info!("screenshot saved to {}", path.display()),
                Err(error) => log::error!("failed to save the screenshot: {error:#}"),
            }
        }
        frame.present();
        FrameStatus::Presented
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
    shadows: &ShadowMaps,
) -> wgpu::BindGroup {
    let view = palette.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("globals bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&shadows.view) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&shadows.sampler) },
        ],
    })
}

/// El cielo: un triángulo a pantalla completa en el fondo de la profundidad.
fn create_sky_pipeline(
    device: &wgpu::Device,
    globals_layout: &wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = scene_shader(device, "sky shader", include_str!("shaders/sky.wgsl"));
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sky pipeline layout"),
        bind_group_layouts: &[globals_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            // Z invertida: el cielo está en 0 y solo pasa donde la profundidad sigue vacía.
            depth_compare: wgpu::CompareFunction::GreaterEqual,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_crop_maps_the_region_to_the_whole_clip_space() {
        // La quinta parte derecha de la pantalla, entera en vertical.
        let crop = region_crop([0.8, 0.0, 1.0, 1.0]);
        let at = |x: f32, y: f32| crop * glam::Vec4::new(x, y, 0.5, 1.0);
        assert!((at(0.6, 0.0).x - -1.0).abs() < 1e-5, "the left edge of the region");
        assert!((at(1.0, 0.0).x - 1.0).abs() < 1e-5, "the right edge of the screen");
        assert!((at(0.8, 0.3).x).abs() < 1e-5 && (at(0.8, 0.3).y - 0.3).abs() < 1e-5);
        // Con w ≠ 1 (perspectiva) la proporción se mantiene.
        let point = crop * glam::Vec4::new(1.6, 0.0, 1.0, 2.0);
        assert!((point.x / point.w).abs() < 1e-5);
    }

    #[test]
    fn scissor_rect_rounds_to_pixels_and_skips_empty_regions() {
        assert_eq!(scissor_rect([0.79, 0.0, 1.0, 1.0], 1920, 1080), Some([1517, 0, 403, 1080]));
        assert_eq!(scissor_rect([0.0, 0.0, 1.0, 1.0], 800, 600), Some([0, 0, 800, 600]));
        assert_eq!(scissor_rect([0.5, 0.0, 0.5, 1.0], 800, 600), None);
    }
}
