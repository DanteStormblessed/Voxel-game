//! Copia del registro (`log`) para la consola de desarrollador.
//!
//! El registro sigue saliendo por la terminal como siempre; además, lo que pasa el
//! filtro se guarda aquí hasta que el juego lo recoge para mostrarlo en la consola.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use log::{Level, Log, Metadata, Record};

/// Líneas que se guardan como mucho si nadie las recoge.
const MAX_PENDING: usize = 512;

type Lines = Arc<Mutex<VecDeque<(Level, String)>>>;

/// Lado del juego: de aquí se recogen las líneas nuevas.
#[derive(Clone, Default)]
pub struct LogCapture {
    lines: Lines,
}

impl LogCapture {
    /// Líneas registradas desde la última llamada, en orden.
    pub fn drain(&self) -> Vec<(Level, String)> {
        self.lines.lock().map(|mut lines| lines.drain(..).collect()).unwrap_or_default()
    }
}

struct Tee {
    inner: env_logger::Logger,
    lines: Lines,
}

impl Log for Tee {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if !self.inner.matches(record) {
            return;
        }
        if let Ok(mut lines) = self.lines.lock() {
            if lines.len() >= MAX_PENDING {
                lines.pop_front();
            }
            lines.push_back((record.level(), record.args().to_string()));
        }
        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Instala el registro del juego: `env_logger` (con `RUST_LOG` o, si no, `default_filter`)
/// más la copia para la consola.
pub fn init(default_filter: &str) -> LogCapture {
    let inner =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_filter))
            .build();
    let capture = LogCapture::default();
    let level = inner.filter();
    if log::set_boxed_logger(Box::new(Tee { inner, lines: capture.lines.clone() })).is_ok() {
        log::set_max_level(level);
    }
    capture
}
