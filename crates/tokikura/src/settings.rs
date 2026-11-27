//! Ajustes del jugador: los que se cambian en la pantalla de ajustes (o con `set` en la
//! consola) y se guardan en `settings.toml`.
//!
//! No sabe nada de interfaces ni del motor; solo traduce sus valores a los del motor
//! con [`UserSettings::client`].

use std::ops::RangeInclusive;
use std::path::Path;

use kuravoxel_client::render::ShadowQuality;
use kuravoxel_client::session::{ClientSettings, FrameLimit};
use serde::{Deserialize, Serialize};

pub const VIEW_DISTANCE: RangeInclusive<u8> = 3..=16;
pub const FAR_DISTANCE: RangeInclusive<u8> = 3..=160;
pub const FOV: RangeInclusive<f32> = 50.0..=110.0;
pub const MOUSE_SENSITIVITY: RangeInclusive<f32> = 0.1..=3.0;
pub const UI_SCALE: RangeInclusive<f32> = 0.75..=2.0;
const MAX_NAME: usize = 24;
/// Valores de `max_fps` que ofrece la pantalla de ajustes, en orden. A mano (o con `set
/// fps`) vale cualquier número de `MAX_FPS`.
pub const MAX_FPS_CHOICES: [&str; 10] =
    ["monitor", "30", "60", "75", "90", "120", "144", "165", "240", "unlimited"];
pub const MAX_FPS: RangeInclusive<u32> = 30..=240;
/// Frames por segundo mientras no se juega (ventana sin foco, pausa o menús).
const BACKGROUND_FPS: u32 = 30;

/// `monitor` (los que muestra la pantalla), `unlimited` o un número de `MAX_FPS`.
pub fn frame_limit(value: &str) -> Option<FrameLimit> {
    match value.trim() {
        "monitor" => Some(FrameLimit::Display),
        "unlimited" => Some(FrameLimit::Unlimited),
        number => {
            let fps: u32 = number.parse().ok()?;
            Some(FrameLimit::Fps(fps.clamp(*MAX_FPS.start(), *MAX_FPS.end())))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSettings {
    pub player_name: String,
    /// Radio de voxeles reales, en chunks.
    pub view_distance: u8,
    /// Radio total con terreno lejano, en chunks.
    pub far_distance: u8,
    /// Campo de visión vertical, en grados.
    pub fov: f32,
    /// Multiplicador de la sensibilidad del ratón (1 = la normal).
    pub mouse_sensitivity: f32,
    pub vsync: bool,
    /// Tamaño de la interfaz (1 = normal).
    pub ui_scale: f32,
    /// Calidad de las sombras: `off`, `low`, `medium` o `high`.
    pub shadows: String,
    /// Tope de frames por segundo: `monitor`, `unlimited` o un número (30-240).
    pub max_fps: String,
    /// Desarrollo (no se guarda): frames por segundo sin foco o en pausa; 0 = sin rebaja.
    #[serde(skip)]
    pub background_fps: u32,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            player_name: std::env::var("USER").unwrap_or_else(|_| "Player".into()),
            view_distance: 12,
            far_distance: 160,
            fov: 70.0,
            mouse_sensitivity: 1.0,
            vsync: true,
            ui_scale: 1.0,
            shadows: ShadowQuality::default().name().into(),
            max_fps: "monitor".into(),
            background_fps: BACKGROUND_FPS,
        }
    }
}

fn clamp_f32(value: f32, range: RangeInclusive<f32>, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(*range.start(), *range.end())
    } else {
        fallback
    }
}

impl UserSettings {
    /// Deja todos los valores dentro de sus límites (un archivo editado a mano puede
    /// traer cualquier cosa).
    pub fn sanitized(mut self) -> Self {
        let defaults = Self::default();
        let name: String =
            self.player_name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
        self.player_name = if name.is_empty() { defaults.player_name } else { name };
        self.view_distance = self.view_distance.clamp(*VIEW_DISTANCE.start(), *VIEW_DISTANCE.end());
        // El terreno lejano nunca llega menos lejos que los voxeles reales.
        self.far_distance = self.far_distance.clamp(self.view_distance, *FAR_DISTANCE.end());
        self.fov = clamp_f32(self.fov, FOV, defaults.fov);
        self.mouse_sensitivity =
            clamp_f32(self.mouse_sensitivity, MOUSE_SENSITIVITY, defaults.mouse_sensitivity);
        self.ui_scale = clamp_f32(self.ui_scale, UI_SCALE, defaults.ui_scale);
        if ShadowQuality::parse(&self.shadows).is_none() {
            self.shadows = defaults.shadows;
        }
        self.max_fps = match frame_limit(&self.max_fps) {
            Some(FrameLimit::Fps(fps)) => fps.to_string(),
            Some(_) => self.max_fps.trim().to_string(),
            None => defaults.max_fps,
        };
        self
    }

    pub fn frame_limit(&self) -> FrameLimit {
        frame_limit(&self.max_fps).unwrap_or_default()
    }

    pub fn shadow_quality(&self) -> ShadowQuality {
        ShadowQuality::parse(&self.shadows).unwrap_or_default()
    }

    /// Lee los ajustes; si el archivo no existe o no se entiende, los de por defecto.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                log::warn!("could not read {}: {error}", path.display());
                return Self::default();
            }
        };
        match toml::from_str::<Self>(&text) {
            Ok(settings) => settings.sanitized(),
            Err(error) => {
                log::warn!("ignoring {}: {error}", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(self)?;
        // A un temporal y renombrando: un corte a medias no deja el archivo roto.
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }

    /// La parte de los ajustes que aplica el motor.
    pub fn client(&self) -> ClientSettings {
        ClientSettings {
            vsync: self.vsync,
            fov_degrees: self.fov,
            mouse_sensitivity: self.mouse_sensitivity,
            shadows: self.shadow_quality(),
            max_fps: self.frame_limit(),
            background_fps: self.background_fps,
        }
    }

    /// Los ajustes como texto `clave = valor`, con las mismas claves que acepta `set`.
    pub fn describe(&self) -> Vec<String> {
        vec![
            format!("name = {}", self.player_name),
            format!("view = {}", self.view_distance),
            format!("far = {}", self.far_distance),
            format!("fov = {:.0}", self.fov),
            format!("sensitivity = {:.2}", self.mouse_sensitivity),
            format!("vsync = {}", self.vsync),
            format!("ui_scale = {:.2}", self.ui_scale),
            format!("shadows = {}", self.shadows),
            format!("fps = {}", self.max_fps),
        ]
    }

    /// Cambia un ajuste a partir de texto (consola). El error es el mensaje para quien
    /// lo escribió; el valor final queda dentro de sus límites.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        fn number<T: std::str::FromStr>(value: &str) -> Result<T, String> {
            value.parse().map_err(|_| format!("'{value}' is not a valid number."))
        }
        let mut changed = self.clone();
        match key {
            "name" => changed.player_name = value.to_string(),
            "view" => changed.view_distance = number(value)?,
            "far" => changed.far_distance = number(value)?,
            "fov" => changed.fov = number(value)?,
            "sensitivity" => changed.mouse_sensitivity = number(value)?,
            "ui_scale" => changed.ui_scale = number(value)?,
            "shadows" => {
                if ShadowQuality::parse(value).is_none() {
                    return Err("Use off, low, medium or high.".into());
                }
                changed.shadows = value.to_string();
            }
            "fps" => {
                if frame_limit(value).is_none() {
                    return Err("Use monitor, unlimited or a number from 30 to 240.".into());
                }
                changed.max_fps = value.trim().to_string();
            }
            "vsync" => {
                changed.vsync = match value {
                    "on" | "true" | "1" => true,
                    "off" | "false" | "0" => false,
                    _ => return Err("Use 'on' or 'off'.".into()),
                }
            }
            _ => return Err(format!("Unknown setting '{key}'. Type 'set' for a list.")),
        }
        *self = changed.sanitized();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_values_are_brought_back() {
        let settings = UserSettings {
            player_name: "  \n ".into(),
            view_distance: 200,
            far_distance: 1,
            fov: f32::NAN,
            mouse_sensitivity: 99.0,
            vsync: false,
            ui_scale: 0.0,
            shadows: "ultra".into(),
            max_fps: "fast".into(),
            background_fps: 30,
        }
        .sanitized();
        assert_eq!(settings.max_fps, "monitor");
        assert_eq!(settings.shadows, "medium");
        assert!(!settings.player_name.trim().is_empty());
        assert_eq!(settings.view_distance, 16);
        assert_eq!(settings.far_distance, 16, "far terrain never ends before the voxels");
        assert_eq!(settings.fov, 70.0);
        assert_eq!(settings.mouse_sensitivity, 3.0);
        assert_eq!(settings.ui_scale, 0.75);
    }

    #[test]
    fn saved_settings_come_back_and_broken_files_are_ignored() {
        let dir = std::env::temp_dir().join(format!("tokikura-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        assert_eq!(UserSettings::load(&path), UserSettings::default());

        let mut settings = UserSettings::default();
        settings.set("fov", "95").unwrap();
        settings.set("name", "Kura").unwrap();
        settings.set("vsync", "off").unwrap();
        settings.save(&path).unwrap();
        assert_eq!(UserSettings::load(&path), settings);

        // Un archivo parcial conserva los valores por defecto de lo que falta.
        std::fs::write(&path, "fov = 80.0\n").unwrap();
        let partial = UserSettings::load(&path);
        assert_eq!(partial.fov, 80.0);
        assert_eq!(partial.view_distance, UserSettings::default().view_distance);

        std::fs::write(&path, "fov = [not toml").unwrap();
        assert_eq!(UserSettings::load(&path), UserSettings::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn set_validates_keys_and_values() {
        let mut settings = UserSettings::default();
        assert!(settings.set("gravity", "1").unwrap_err().contains("Unknown setting"));
        assert!(settings.set("fov", "wide").is_err());
        assert!(settings.set("vsync", "maybe").is_err());
        assert!(settings.set("shadows", "soft").is_err());
        settings.set("shadows", "high").unwrap();
        assert_eq!(settings.client().shadows, ShadowQuality::High);
        settings.set("view", "99").unwrap();
        assert_eq!(settings.view_distance, 16);
        assert_eq!(settings.client().fov_degrees, settings.fov);
    }

    #[test]
    fn max_fps_accepts_monitor_unlimited_and_numbers() {
        assert_eq!(UserSettings::default().frame_limit(), FrameLimit::Display);
        let mut settings = UserSettings::default();
        settings.set("fps", "144").unwrap();
        assert_eq!(settings.frame_limit(), FrameLimit::Fps(144));
        settings.set("fps", "1000").unwrap();
        assert_eq!(settings.max_fps, "240", "out of range numbers are brought back");
        settings.set("fps", "unlimited").unwrap();
        assert_eq!(settings.client().max_fps, FrameLimit::Unlimited);
        assert!(settings.set("fps", "fast").is_err());
        for choice in MAX_FPS_CHOICES {
            assert!(frame_limit(choice).is_some(), "{choice}");
        }
    }
}
