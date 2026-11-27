//! Ajustes del cielo, la luz y las sombras: los valores que se retocan a ojo con el
//! panel de `screens/sky.rs` o con `env` en la consola, y su archivo (`environment.toml`).
//!
//! El motor solo conoce `Environment`; aquí están sus nombres, sus límites y cómo se
//! guarda. Los colores se escriben como `#RRGGBB`.

use std::ops::RangeInclusive;
use std::path::Path;

use kuravoxel_client::render::{Environment, SkyColors};
use serde::{Deserialize, Serialize};

/// Un ajuste numérico del ambiente.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scalar {
    SunTilt,
    Sun,
    Moon,
    Ambient,
    ShadowStrength,
    ShadowSoftness,
    ShadowDistance,
}

impl Scalar {
    pub const ALL: [Scalar; 7] = [
        Scalar::SunTilt,
        Scalar::Sun,
        Scalar::Moon,
        Scalar::Ambient,
        Scalar::ShadowStrength,
        Scalar::ShadowSoftness,
        Scalar::ShadowDistance,
    ];

    /// Nombre en la consola y en el archivo.
    pub fn key(self) -> &'static str {
        match self {
            Scalar::SunTilt => "sun_tilt",
            Scalar::Sun => "sun",
            Scalar::Moon => "moon",
            Scalar::Ambient => "ambient",
            Scalar::ShadowStrength => "shadow_strength",
            Scalar::ShadowSoftness => "shadow_softness",
            Scalar::ShadowDistance => "shadow_distance",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scalar::SunTilt => "Sun tilt",
            Scalar::Sun => "Sun light",
            Scalar::Moon => "Moon light",
            Scalar::Ambient => "Ambient light",
            Scalar::ShadowStrength => "Shadow strength",
            Scalar::ShadowSoftness => "Shadow softness",
            Scalar::ShadowDistance => "Shadow distance",
        }
    }

    /// Límites del deslizador (el motor admite algo más por consola).
    pub fn range(self) -> RangeInclusive<f32> {
        match self {
            Scalar::SunTilt => 0.0..=70.0,
            Scalar::Sun | Scalar::Moon | Scalar::Ambient => 0.0..=2.0,
            Scalar::ShadowStrength => 0.0..=1.0,
            Scalar::ShadowSoftness => 0.0..=0.4,
            Scalar::ShadowDistance => 32.0..=384.0,
        }
    }

    pub fn step(self) -> f32 {
        match self {
            Scalar::SunTilt => 1.0,
            Scalar::ShadowDistance => 8.0,
            _ => 0.01,
        }
    }

    pub fn get(self, environment: &Environment) -> f32 {
        match self {
            Scalar::SunTilt => environment.sun_tilt,
            Scalar::Sun => environment.sun_intensity,
            Scalar::Moon => environment.moon_intensity,
            Scalar::Ambient => environment.ambient_intensity,
            Scalar::ShadowStrength => environment.shadow_strength,
            Scalar::ShadowSoftness => environment.shadow_softness,
            Scalar::ShadowDistance => environment.shadow_distance,
        }
    }

    pub fn set(self, environment: &mut Environment, value: f32) {
        *match self {
            Scalar::SunTilt => &mut environment.sun_tilt,
            Scalar::Sun => &mut environment.sun_intensity,
            Scalar::Moon => &mut environment.moon_intensity,
            Scalar::Ambient => &mut environment.ambient_intensity,
            Scalar::ShadowStrength => &mut environment.shadow_strength,
            Scalar::ShadowSoftness => &mut environment.shadow_softness,
            Scalar::ShadowDistance => &mut environment.shadow_distance,
        } = value;
    }

    /// El valor como se enseña (con su unidad).
    pub fn show(self, value: f32) -> String {
        match self {
            Scalar::SunTilt => format!("{value:.0}°"),
            Scalar::ShadowDistance => format!("{value:.0} m"),
            Scalar::ShadowSoftness => format!("{:.0} cm", value * 100.0),
            _ => format!("{value:.2}"),
        }
    }
}

/// Momentos del día con colores propios, y los colores de cada uno. `night` es la noche
/// con luna llena y `moonless` la de luna nueva; cada noche mezcla las dos según su luna.
pub const PHASES: [&str; 4] = ["day", "dusk", "night", "moonless"];
pub const SLOTS: [&str; 4] = ["zenith", "horizon", "light", "ambient"];
pub const SLOT_LABELS: [&str; 4] = ["Sky top", "Horizon", "Light", "Ambient"];

fn phase(environment: &Environment, index: usize) -> &SkyColors {
    [&environment.day, &environment.dusk, &environment.night, &environment.moonless][index]
}

/// Color de un momento del día (índices en `PHASES` y `SLOTS`), como 0xRRGGBB.
pub fn color(environment: &Environment, phase_index: usize, slot: usize) -> u32 {
    let colors = phase(environment, phase_index);
    [colors.zenith, colors.horizon, colors.light, colors.ambient][slot]
}

pub fn set_color(environment: &mut Environment, phase_index: usize, slot: usize, value: u32) {
    let colors = [&mut environment.day, &mut environment.dusk, &mut environment.night, &mut environment.moonless]
        .into_iter()
        .nth(phase_index)
        .expect("phase index");
    *[&mut colors.zenith, &mut colors.horizon, &mut colors.light, &mut colors.ambient]
        .into_iter()
        .nth(slot)
        .expect("slot index") = value;
}

pub fn hex(color: u32) -> String {
    format!("#{:06X}", color & 0xFF_FFFF)
}

/// Interpreta `#RRGGBB` (la almohadilla es opcional).
pub fn parse_hex(text: &str) -> Option<u32> {
    let digits = text.trim().trim_start_matches('#');
    (digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten()
}

/// Los ajustes como texto `clave = valor`, con las claves que acepta `set`.
pub fn describe(environment: &Environment) -> Vec<String> {
    let mut lines: Vec<String> =
        Scalar::ALL.iter().map(|scalar| format!("{} = {:.2}", scalar.key(), scalar.get(environment))).collect();
    for (p, phase_name) in PHASES.iter().enumerate() {
        for (s, slot) in SLOTS.iter().enumerate() {
            lines.push(format!("{phase_name}.{slot} = {}", hex(color(environment, p, s))));
        }
    }
    lines
}

/// Cambia un ajuste a partir de texto (consola). El error es el mensaje para quien lo
/// escribió; el valor final queda dentro de sus límites.
pub fn set(environment: &mut Environment, key: &str, value: &str) -> Result<(), String> {
    let mut changed = environment.clone();
    if let Some(scalar) = Scalar::ALL.into_iter().find(|scalar| scalar.key() == key) {
        let number: f32 = value.parse().map_err(|_| format!("'{value}' is not a valid number."))?;
        scalar.set(&mut changed, number);
    } else {
        let slot = key.split_once('.').and_then(|(phase_name, slot)| {
            Some((PHASES.iter().position(|p| *p == phase_name)?, SLOTS.iter().position(|s| *s == slot)?))
        });
        let Some((p, s)) = slot else {
            return Err(format!("Unknown value '{key}'. Type 'env' for a list."));
        };
        let color = parse_hex(value).ok_or_else(|| format!("'{value}' is not a color like #9CC8EC."))?;
        set_color(&mut changed, p, s, color);
    }
    *environment = changed.sanitized();
    Ok(())
}

// --- Archivo ---

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct PhaseFile {
    zenith: String,
    horizon: String,
    light: String,
    ambient: String,
}

impl PhaseFile {
    fn from_colors(colors: &SkyColors) -> Self {
        Self {
            zenith: hex(colors.zenith),
            horizon: hex(colors.horizon),
            light: hex(colors.light),
            ambient: hex(colors.ambient),
        }
    }

    /// Los colores que no se entienden se quedan como estaban en `fallback`.
    fn colors(&self, fallback: SkyColors) -> SkyColors {
        SkyColors {
            zenith: parse_hex(&self.zenith).unwrap_or(fallback.zenith),
            horizon: parse_hex(&self.horizon).unwrap_or(fallback.horizon),
            light: parse_hex(&self.light).unwrap_or(fallback.light),
            ambient: parse_hex(&self.ambient).unwrap_or(fallback.ambient),
        }
    }
}

impl Default for PhaseFile {
    fn default() -> Self {
        Self { zenith: String::new(), horizon: String::new(), light: String::new(), ambient: String::new() }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct File {
    sun_tilt: f32,
    sun: f32,
    moon: f32,
    ambient: f32,
    shadow_strength: f32,
    shadow_softness: f32,
    shadow_distance: f32,
    day: PhaseFile,
    dusk: PhaseFile,
    night: PhaseFile,
    moonless: PhaseFile,
}

impl File {
    fn from_environment(environment: &Environment) -> Self {
        Self {
            sun_tilt: environment.sun_tilt,
            sun: environment.sun_intensity,
            moon: environment.moon_intensity,
            ambient: environment.ambient_intensity,
            shadow_strength: environment.shadow_strength,
            shadow_softness: environment.shadow_softness,
            shadow_distance: environment.shadow_distance,
            day: PhaseFile::from_colors(&environment.day),
            dusk: PhaseFile::from_colors(&environment.dusk),
            night: PhaseFile::from_colors(&environment.night),
            moonless: PhaseFile::from_colors(&environment.moonless),
        }
    }

    fn environment(&self) -> Environment {
        let defaults = Environment::default();
        Environment {
            sun_tilt: self.sun_tilt,
            sun_intensity: self.sun,
            moon_intensity: self.moon,
            ambient_intensity: self.ambient,
            shadow_strength: self.shadow_strength,
            shadow_softness: self.shadow_softness,
            shadow_distance: self.shadow_distance,
            day: self.day.colors(defaults.day),
            dusk: self.dusk.colors(defaults.dusk),
            night: self.night.colors(defaults.night),
            moonless: self.moonless.colors(defaults.moonless),
        }
        .sanitized()
    }
}

impl Default for File {
    fn default() -> Self {
        Self::from_environment(&Environment::default())
    }
}

/// Lee el ambiente guardado; si el archivo no existe o no se entiende, el de por defecto.
pub fn load(path: &Path) -> Environment {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Environment::default(),
        Err(error) => {
            log::warn!("could not read {}: {error}", path.display());
            return Environment::default();
        }
    };
    match toml::from_str::<File>(&text) {
        Ok(file) => file.environment(),
        Err(error) => {
            log::warn!("ignoring {}: {error}", path.display());
            Environment::default()
        }
    }
}

pub fn save(environment: &Environment, path: &Path) -> anyhow::Result<()> {
    let text = toml::to_string_pretty(&File::from_environment(environment))?;
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_set_by_name_and_kept_in_range() {
        let mut environment = Environment::default();
        set(&mut environment, "sun_tilt", "45").unwrap();
        set(&mut environment, "shadow_strength", "7").unwrap();
        set(&mut environment, "night.zenith", "#102030").unwrap();
        set(&mut environment, "dusk.light", "ff8800").unwrap();
        assert_eq!(environment.sun_tilt, 45.0);
        assert_eq!(environment.shadow_strength, 1.0);
        assert_eq!(environment.night.zenith, 0x102030);
        assert_eq!(environment.dusk.light, 0xFF8800);

        assert!(set(&mut environment, "gravity", "1").unwrap_err().contains("Unknown value"));
        assert!(set(&mut environment, "sun", "bright").is_err());
        assert!(set(&mut environment, "day.horizon", "blue").is_err());
        assert!(set(&mut environment, "noon.horizon", "#000000").is_err());
        assert!(describe(&environment).contains(&"night.zenith = #102030".to_string()));
    }

    #[test]
    fn saved_values_come_back_and_broken_files_are_ignored() {
        let dir = std::env::temp_dir().join(format!("tokikura-environment-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("environment.toml");
        assert_eq!(load(&path), Environment::default());

        let mut environment = Environment::default();
        set(&mut environment, "shadow_softness", "0.25").unwrap();
        set(&mut environment, "day.zenith", "#3366CC").unwrap();
        save(&environment, &path).unwrap();
        assert_eq!(load(&path), environment);

        // Un archivo parcial, o con un color mal escrito, conserva lo demás por defecto.
        std::fs::write(&path, "sun = 1.5\n[night]\nzenith = \"dark\"\nhorizon = \"#010203\"\n").unwrap();
        let partial = load(&path);
        assert_eq!(partial.sun_intensity, 1.5);
        assert_eq!(partial.night.zenith, Environment::default().night.zenith);
        assert_eq!(partial.night.horizon, 0x010203);

        std::fs::write(&path, "sun = [not toml").unwrap();
        assert_eq!(load(&path), Environment::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
