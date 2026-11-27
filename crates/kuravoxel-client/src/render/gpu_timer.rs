//! Medición del tiempo de GPU de cada pase (desarrollo: `KURAVOXEL_GPU_TIMING=1`).
//!
//! Escribe marcas de tiempo al empezar y acabar cada pase, las copia a un buffer de
//! lectura sin esperar a la GPU (anillo de varios frames) y cada dos segundos registra
//! la media en milisegundos. Si el adaptador no admite marcas de tiempo, no hace nada.

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Marcas por frame: inicio/fin de cada cascada, inicio/fin del pase principal, el
/// final de la interfaz y dos dentro del pase principal (tras lo opaco y tras el cielo).
pub const SHADOW_BEGIN: u32 = 0; // 0..6: cascada c → 2c (inicio), 2c + 1 (fin)
pub const MAIN_BEGIN: u32 = 6;
pub const MAIN_END: u32 = 7;
pub const UI_END: u32 = 8;
pub const AFTER_OPAQUE: u32 = 9;
pub const AFTER_SKY: u32 = 10;
const QUERIES: u32 = 11;
/// Frames en vuelo cuyas marcas aún se están leyendo.
const RING: usize = 4;
const REPORT_EVERY: Duration = Duration::from_secs(2);

const FREE: u8 = 0;
const MAPPING: u8 = 1;
const READY: u8 = 2;

struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// Cascadas dibujadas en ese frame (un bit cada una); las otras se reutilizaron.
    cascades: u8,
}

#[derive(Default)]
struct Sums {
    frames: u32,
    cascades: [f64; 3],
    /// Frames en los que se dibujó cada cascada.
    drawn: [u32; 3],
    opaque: f64,
    sky: f64,
    translucent: f64,
    ui: f64,
    total: f64,
}

pub struct GpuTimer {
    query_set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    ring: Vec<Readback>,
    /// Hueco del anillo que usa el frame en curso (si hay uno libre).
    current: Option<usize>,
    next: usize,
    inside_passes: bool,
    /// Cascadas dibujadas en el frame en curso.
    written: Cell<u8>,
    period_ns: f64,
    sums: Sums,
    cpu_frames: u32,
    since: Instant,
}

impl GpuTimer {
    /// Las características que hay que pedir al dispositivo para medir, si el
    /// adaptador las tiene.
    pub fn features(adapter: &wgpu::Adapter) -> wgpu::Features {
        let wanted = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES;
        adapter.features() & wanted
    }

    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            log::warn!("GPU timing unavailable: the adapter has no timestamp queries");
            return None;
        }
        let size = QUERIES as u64 * 8;
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gpu timer"),
            ty: wgpu::QueryType::Timestamp,
            count: QUERIES,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu timer resolve"),
            size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let ring = (0..RING)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("gpu timer readback"),
                    size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
                cascades: 0,
            })
            .collect();
        log::info!("GPU timing enabled");
        Some(Self {
            query_set,
            resolve,
            ring,
            current: None,
            next: 0,
            inside_passes: device.features().contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES),
            written: Cell::new(0),
            period_ns: queue.get_timestamp_period() as f64,
            sums: Sums::default(),
            cpu_frames: 0,
            since: Instant::now(),
        })
    }

    /// Empieza un frame: recoge lo que ya ha leído la GPU y reserva un hueco si queda.
    pub fn begin_frame(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);
        for index in 0..RING {
            if self.ring[index].state.load(Ordering::Acquire) == READY {
                self.collect(index);
            }
        }
        self.cpu_frames += 1;
        self.current = None;
        self.written.set(0);
        let slot = &mut self.ring[self.next];
        if slot.state.load(Ordering::Acquire) == FREE {
            self.current = Some(self.next);
            self.next = (self.next + 1) % RING;
        }
        if self.since.elapsed() >= REPORT_EVERY {
            self.report();
        }
    }

    /// Marcas de inicio y fin de un pase, si este frame se mide.
    pub fn pass(&self, begin: u32, end: Option<u32>) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.current?;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(begin),
            end_of_pass_write_index: end,
        })
    }

    /// Este frame se dibuja la cascada `index` (si no, se reutiliza la anterior).
    pub fn note_cascade(&self, index: usize) {
        self.written.set(self.written.get() | 1 << index);
    }

    /// Marca a mitad de un pase.
    pub fn mark(&self, pass: &mut wgpu::RenderPass<'_>, index: u32) {
        if self.current.is_some() && self.inside_passes {
            pass.write_timestamp(&self.query_set, index);
        }
    }

    /// Copia las marcas del frame al buffer de lectura (al final del último encoder).
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(index) = self.current else { return };
        encoder.resolve_query_set(&self.query_set, 0..QUERIES, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.ring[index].buffer, 0, QUERIES as u64 * 8);
    }

    /// Tras enviar el frame: pide leer sus marcas.
    pub fn after_submit(&mut self) {
        let Some(index) = self.current.take() else { return };
        self.ring[index].cascades = self.written.get();
        let state = self.ring[index].state.clone();
        state.store(MAPPING, Ordering::Release);
        self.ring[index].buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            state.store(if result.is_ok() { READY } else { FREE }, Ordering::Release);
        });
    }

    fn collect(&mut self, index: usize) {
        let slot = &self.ring[index];
        let ticks: Vec<u64> = {
            let data = slot.buffer.slice(..).get_mapped_range();
            bytemuck::cast_slice::<u8, u64>(&data).to_vec()
        };
        slot.buffer.unmap();
        slot.state.store(FREE, Ordering::Release);

        let ms = |from: u32, to: u32| {
            let (a, b) = (ticks[from as usize], ticks[to as usize]);
            b.saturating_sub(a) as f64 * self.period_ns / 1e6
        };
        let sums = &mut self.sums;
        sums.frames += 1;
        let mut shadows = 0.0;
        for cascade in 0..3 {
            if slot.cascades & 1 << cascade != 0 {
                let begin = SHADOW_BEGIN + cascade * 2;
                let time = ms(begin, begin + 1);
                sums.cascades[cascade as usize] += time;
                sums.drawn[cascade as usize] += 1;
                shadows += time;
            }
        }
        if self.inside_passes {
            sums.opaque += ms(MAIN_BEGIN, AFTER_OPAQUE);
            sums.sky += ms(AFTER_OPAQUE, AFTER_SKY);
            sums.translucent += ms(AFTER_SKY, MAIN_END);
        } else {
            sums.opaque += ms(MAIN_BEGIN, MAIN_END);
        }
        sums.ui += ms(MAIN_END, UI_END);
        sums.total += shadows + ms(MAIN_BEGIN, UI_END);
    }

    fn report(&mut self) {
        let elapsed = self.since.elapsed().as_secs_f64();
        let fps = self.cpu_frames as f64 / elapsed;
        let sums = std::mem::take(&mut self.sums);
        self.cpu_frames = 0;
        self.since = Instant::now();
        if sums.frames == 0 {
            return;
        }
        let n = sums.frames as f64;
        let [c0, c1, c2] = sums.cascades.map(|value| value / n);
        let redrawn = sums.drawn.map(|drawn| 100.0 * drawn as f64 / n);
        log::info!(
            "gpu {:.0} fps ({:.2} ms/frame cpu) | gpu total {:.2} ms: shadows {:.2} ({:.2} + {:.2} + {:.2}; redrawn {:.0}/{:.0}/{:.0}% of frames), \
             opaque {:.2}, sky {:.2}, translucent+marks {:.2}, ui {:.2} | busy {:.0}%",
            fps,
            1000.0 / fps,
            sums.total / n,
            c0 + c1 + c2,
            c0,
            c1,
            c2,
            redrawn[0],
            redrawn[1],
            redrawn[2],
            sums.opaque / n,
            sums.sky / n,
            sums.translucent / n,
            sums.ui / n,
            (sums.total / n) * fps / 10.0,
        );
    }
}
