//! Ajustes del jugador: los que se cambian en la pantalla de ajustes (o con `set` en la
//! consola) y se guardan en `settings.toml`.
//!
//! No sabe nada de interfaces ni del motor; solo traduce sus valores a los del motor
//! con [`UserSettings::client`].

use std::ops::RangeInclusive;
use std::path::Path;

use kuravoxel_client::session::ClientSettings;
use serde::{Deserialize, Serialize};

pub const VIEW_DISTANCE: RangeInclusive<u8> = 3..=16;
pub const FAR_DISTANCE: RangeInclusive<u8> = 3..=160;
pub const FOV: RangeInclusive<f32> = 50.0..=110.0;
pub const MOUSE_SENSITIVITY: RangeInclusive<f32> = 0.1..=3.0;
pub const UI_SCALE: RangeInclusive<f32> = 0.75..=2.0;
const MAX_NAME: usize = 24;

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
        self
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
        }
        .sanitized();
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
        settings.set("view", "99").unwrap();
        assert_eq!(settings.view_distance, 16);
        assert_eq!(settings.client().fov_degrees, settings.fov);
    }
}
