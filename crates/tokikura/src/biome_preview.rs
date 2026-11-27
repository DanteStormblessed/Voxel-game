//! Hilo de las vistas previas del editor de biomas: calcula la miniatura de cada nodo y el
//! mapa de la zona sin frenar el hilo principal (que nunca genera terreno).
//!
//! Solo cuenta el último encargo: si llegan varios mientras calcula uno, salta a lo más
//! reciente.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kuravoxel_common::content::ContentData;
use kuravoxel_common::icon::IconImage;
use kuravoxel_server::biomes::WorldSource;
use kuravoxel_server::preview::{self, Grid, Window};

/// Lo que hay que calcular.
#[derive(Clone)]
pub struct Job {
    /// Número del encargo: el resultado lo lleva, para saber si sigue valiendo.
    pub generation: u64,
    pub source: Arc<WorldSource>,
    pub content: Arc<ContentData>,
    pub seed: u32,
    /// Bioma que se edita; `None` es `world.toml` (nodos comunes, mapa del mundo).
    pub biome: Option<String>,
    pub nodes: Vec<String>,
    pub window: Window,
    /// Lado de las miniaturas y del mapa, en píxeles.
    pub thumb: u32,
    pub map: u32,
}

pub struct Done {
    pub generation: u64,
    /// Zona que enseñan las miniaturas y el mapa.
    pub window: Window,
    pub nodes: Vec<(String, Result<Grid, String>)>,
    pub map: Result<IconImage, String>,
    pub elapsed: Duration,
}

pub fn run(job: &Job) -> Done {
    let start = Instant::now();
    let grids = preview::node_grids(&job.source, job.biome.as_deref(), &job.nodes, job.seed, job.window, job.thumb);
    let map = preview::window_map(&job.source, &job.content, job.seed, job.biome.as_deref(), job.window, job.map)
        .map_err(|error| format!("{error:#}"));
    Done {
        generation: job.generation,
        window: job.window,
        nodes: job.nodes.iter().cloned().zip(grids).collect(),
        map,
        elapsed: start.elapsed(),
    }
}

pub struct PreviewWorker {
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

impl PreviewWorker {
    pub fn spawn() -> Self {
        let (jobs, inbox) = channel::<Job>();
        let (outbox, done) = channel();
        let spawned = std::thread::Builder::new().name("biome-preview".into()).spawn(move || {
            while let Ok(mut job) = inbox.recv() {
                while let Ok(newer) = inbox.try_recv() {
                    job = newer;
                }
                if outbox.send(run(&job)).is_err() {
                    break;
                }
            }
        });
        if let Err(error) = spawned {
            log::error!("could not start the biome preview thread: {error}");
        }
        Self { jobs, done }
    }

    pub fn request(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// El resultado más reciente que haya llegado.
    pub fn poll(&self) -> Option<Done> {
        self.done.try_iter().last()
    }
}
