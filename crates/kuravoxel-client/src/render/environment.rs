//! Ambiente: de dónde viene la luz y de qué color es el cielo a cada hora del día.
//!
//! [`Environment`] son los valores que se ajustan a ojo (inclinación del sol, colores del
//! día, del atardecer y de la noche, sombras); [`Lighting`] es lo que sale de aplicarlos a
//! una hora concreta. Es todo cálculo en CPU: no toca la GPU.

use glam::Vec3;
use kuravoxel_common::block::hex_to_linear;

/// Calidad de las sombras: tamaño de cada mapa y muestras del filtro.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShadowQuality {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl ShadowQuality {
    pub const ALL: [ShadowQuality; 4] =
        [ShadowQuality::Off, ShadowQuality::Low, ShadowQuality::Medium, ShadowQuality::High];

    /// Lado de cada mapa de sombras, en texels.
    pub fn resolution(self) -> u32 {
        match self {
            ShadowQuality::Off => 16,
            ShadowQuality::Low => 1024,
            ShadowQuality::Medium => 2048,
            ShadowQuality::High => 4096,
        }
    }

    /// Muestras del filtro a cada lado del centro (1 = 3×3, 2 = 5×5).
    pub fn filter_half(self) -> u32 {
        match self {
            ShadowQuality::Off | ShadowQuality::Low => 1,
            ShadowQuality::Medium | ShadowQuality::High => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ShadowQuality::Off => "off",
            ShadowQuality::Low => "low",
            ShadowQuality::Medium => "medium",
            ShadowQuality::High => "high",
        }
    }

    pub fn parse(name: &str) -> Option<ShadowQuality> {
        Self::ALL.into_iter().find(|quality| quality.name() == name)
    }
}

/// Colores de un momento del día, como 0xRRGGBB en sRGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyColors {
    /// Cielo en lo alto.
    pub zenith: u32,
    /// Cielo en el horizonte; el terreno lejano se funde con él.
    pub horizon: u32,
    /// Luz directa (del sol, o de la luna por la noche).
    pub light: u32,
    /// Luz ambiente que llega del cielo.
    pub ambient: u32,
}

/// Lo que se puede ajustar del ciclo de día y noche y de las sombras.
#[derive(Clone, Debug, PartialEq)]
pub struct Environment {
    /// Grados que el recorrido del sol se inclina hacia el norte: con 0 pasa justo por
    /// encima a mediodía y las sombras desaparecen bajo las cosas.
    pub sun_tilt: f32,
    pub sun_intensity: f32,
    pub moon_intensity: f32,
    pub ambient_intensity: f32,
    /// Cuánta luz directa quitan las sombras (0 = no se ven, 1 = del todo).
    pub shadow_strength: f32,
    /// Radio de la penumbra, en metros.
    pub shadow_softness: f32,
    /// Hasta dónde llegan las sombras desde la cámara, en metros.
    pub shadow_distance: f32,
    pub day: SkyColors,
    /// Amanecer y atardecer.
    pub dusk: SkyColors,
    /// Noche con luna llena.
    pub night: SkyColors,
    /// Noche sin luna (luna nueva): las noches se mezclan entre `night` y esta según la
    /// parte iluminada de la luna. Puede ser casi negra; lo que emite luz sigue viéndose.
    pub moonless: SkyColors,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            sun_tilt: 32.0,
            sun_intensity: 0.85,
            moon_intensity: 0.45,
            ambient_intensity: 1.0,
            shadow_strength: 0.85,
            shadow_softness: 0.10,
            shadow_distance: 200.0,
            day: SkyColors { zenith: 0x5C9FE6, horizon: 0xA6D0F0, light: 0xFFF3DC, ambient: 0xA8B4C8 },
            dusk: SkyColors { zenith: 0x47528C, horizon: 0xD9A0A0, light: 0xFF9A4C, ambient: 0x8E86A6 },
            night: SkyColors { zenith: 0x0A1030, horizon: 0x22305C, light: 0x9DB4F0, ambient: 0x46558C },
            moonless: SkyColors { zenith: 0x020308, horizon: 0x070A14, light: 0x000000, ambient: 0x0C0F1A },
        }
    }
}

impl Environment {
    /// Deja todo dentro de límites razonables (viene de un archivo o de la consola).
    pub fn sanitized(mut self) -> Self {
        let defaults = Self::default();
        let fix = |value: f32, min: f32, max: f32, fallback: f32| {
            if value.is_finite() { value.clamp(min, max) } else { fallback }
        };
        self.sun_tilt = fix(self.sun_tilt, 0.0, 70.0, defaults.sun_tilt);
        self.sun_intensity = fix(self.sun_intensity, 0.0, 3.0, defaults.sun_intensity);
        self.moon_intensity = fix(self.moon_intensity, 0.0, 3.0, defaults.moon_intensity);
        self.ambient_intensity = fix(self.ambient_intensity, 0.0, 3.0, defaults.ambient_intensity);
        self.shadow_strength = fix(self.shadow_strength, 0.0, 1.0, defaults.shadow_strength);
        self.shadow_softness = fix(self.shadow_softness, 0.0, 0.5, defaults.shadow_softness);
        self.shadow_distance = fix(self.shadow_distance, 32.0, 512.0, defaults.shadow_distance);
        self
    }
}

/// Luz y cielo de un instante.
#[derive(Clone, Copy, Debug)]
pub struct Lighting {
    /// Hacia el sol (puede estar bajo el horizonte). La luna está justo enfrente.
    pub sun: Vec3,
    /// Eje sobre el que giran el sol y la luna: siempre perpendicular a ambos.
    pub axis: Vec3,
    /// Hacia la luz que ilumina: el sol de día, la luna de noche.
    pub light_dir: Vec3,
    /// Color lineal de esa luz, con su intensidad.
    pub light_color: [f32; 3],
    /// 0 con la luz en el horizonte, 1 cuando ya está alta: lo que se ven las sombras.
    pub light_fade: f32,
    pub sky_ambient: [f32; 3],
    pub ground_ambient: [f32; 3],
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    /// Resplandor del horizonte hacia el sol: color e intensidad.
    pub glow: [f32; 4],
    pub sun_disc: [f32; 4],
    pub moon_disc: [f32; 4],
    /// Fase de la luna: parte iluminada (0 = nueva, 1 = llena) y el coseno y el seno del
    /// ángulo de fase, con los que el cielo dibuja la parte en sombra del disco.
    pub moon_phase: [f32; 4],
}

/// Proporción de la luz ambiente que rebota del suelo, por canal (algo cálida).
const GROUND_BOUNCE: [f32; 3] = [0.53, 0.43, 0.31];

fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn linear(hex: u32) -> [f32; 3] {
    let [r, g, b, _] = hex_to_linear(hex);
    [r, g, b]
}

fn blend(colors: [[f32; 3]; 3], weights: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|c| (0..3).map(|i| colors[i][c] * weights[i]).sum())
}

fn scaled(color: [f32; 3], factor: f32) -> [f32; 3] {
    color.map(|c| c * factor)
}

/// Hacia dónde está el sol a una hora (0-24) y el eje sobre el que gira. Sale por el este
/// (+X) a las 6, pasa por lo más alto a las 12, inclinado hacia el norte (-Z), y se pone
/// por el oeste a las 18.
pub fn sun_direction(hours: f32, tilt_degrees: f32) -> (Vec3, Vec3) {
    let angle = (hours - 6.0) / 24.0 * std::f32::consts::TAU;
    let (sin_tilt, cos_tilt) = tilt_degrees.to_radians().sin_cos();
    let sun = Vec3::new(angle.cos(), angle.sin() * cos_tilt, -angle.sin() * sin_tilt);
    (sun, Vec3::new(0.0, sin_tilt, cos_tilt))
}

/// Mezcla dos juegos de colores: `t` = 0 da `a`, 1 da `b` (en sRGB, como se escriben).
fn mix_colors(a: &SkyColors, b: &SkyColors, t: f32) -> SkyColors {
    let mix = |a: u32, b: u32| {
        let channel = |shift: u32| {
            let (x, y) = (((a >> shift) & 0xFF) as f32, ((b >> shift) & 0xFF) as f32);
            ((x + (y - x) * t).round().clamp(0.0, 255.0) as u32) << shift
        };
        channel(16) | channel(8) | channel(0)
    };
    SkyColors {
        zenith: mix(a.zenith, b.zenith),
        horizon: mix(a.horizon, b.horizon),
        light: mix(a.light, b.light),
        ambient: mix(a.ambient, b.ambient),
    }
}

impl Lighting {
    /// Luz y cielo a una hora (0-24) con la luna de edad `moon_age` (0 = nueva, 0,5 =
    /// llena; ver `WorldTime::moon_age`).
    pub fn at(environment: &Environment, hours: f32, moon_age: f32) -> Lighting {
        let phase_angle = moon_age.rem_euclid(1.0) * std::f32::consts::TAU;
        let moon_light = (1.0 - phase_angle.cos()) * 0.5;
        // La noche de esta luna: entre la de luna llena y la de sin luna.
        let night_colors = mix_colors(&environment.moonless, &environment.night, moon_light);
        let (sun, axis) = sun_direction(hours, environment.sun_tilt);
        let height = sun.y;

        // Cuánto hay de día, de atardecer y de noche según la altura del sol.
        let day = smoothstep(-0.05, 0.30, height);
        let night = 1.0 - smoothstep(-0.25, 0.05, height);
        let dusk = (1.0 - day - night).max(0.0);
        let total = day + dusk + night;
        let weights = [day / total, dusk / total, night / total];
        let pick = |get: fn(&SkyColors) -> u32| {
            blend(
                [linear(get(&environment.day)), linear(get(&environment.dusk)), linear(get(&night_colors))],
                weights,
            )
        };

        let ambient = scaled(pick(|colors| colors.ambient), environment.ambient_intensity);
        // Una sola luz directa: el sol mientras está sobre el horizonte y la luna después.
        // Las dos se apagan al cruzarlo, así el relevo no da un salto.
        let (light_dir, light_color, light_fade) = if height >= 0.0 {
            let fade = smoothstep(0.0, 0.14, height);
            let warm = day / (day + dusk).max(1e-4);
            let color = blend(
                [linear(environment.day.light), linear(environment.dusk.light), [0.0; 3]],
                [warm, 1.0 - warm, 0.0],
            );
            (sun, scaled(color, environment.sun_intensity * fade), fade)
        } else {
            // La luna alumbra según su fase; sin luna no hay luz directa ni sombras.
            let fade = smoothstep(0.0, 0.14, -height) * moon_light;
            (-sun, scaled(linear(night_colors.light), environment.moon_intensity * fade), fade)
        };

        let [glow_r, glow_g, glow_b] = linear(environment.dusk.light);
        let disc = blend([[1.0, 0.97, 0.86], [1.0, 0.62, 0.30], [1.0, 0.62, 0.30]], weights);
        Lighting {
            sun,
            axis,
            light_dir,
            light_color,
            light_fade,
            sky_ambient: ambient,
            ground_ambient: std::array::from_fn(|c| ambient[c] * GROUND_BOUNCE[c]),
            zenith: pick(|colors| colors.zenith),
            horizon: pick(|colors| colors.horizon),
            glow: [glow_r, glow_g, glow_b, weights[1] * 0.85],
            sun_disc: [disc[0], disc[1], disc[2], 1.0],
            moon_disc: [0.88, 0.92, 1.0, 1.0 - weights[0] * 0.75],
            moon_phase: [moon_light, phase_angle.cos(), phase_angle.sin(), 0.0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Edad de la luna llena.
    const FULL: f32 = 0.5;

    #[test]
    fn the_sun_rises_in_the_east_and_leans_north() {
        let (dawn, axis) = sun_direction(6.0, 30.0);
        assert!(dawn.x > 0.99 && dawn.y.abs() < 1e-4);
        let (noon, _) = sun_direction(12.0, 30.0);
        assert!(noon.y > 0.8 && noon.z < -0.4, "at noon it is high and to the north: {noon}");
        let (dusk, _) = sun_direction(18.0, 30.0);
        assert!(dusk.x < -0.99);
        for hours in [0.0, 3.0, 9.5, 12.0, 17.0, 22.0] {
            let (sun, _) = sun_direction(hours, 30.0);
            assert!((sun.length() - 1.0).abs() < 1e-4);
            assert!(sun.dot(axis).abs() < 1e-4, "the axis stays perpendicular to the sun");
        }
    }

    #[test]
    fn the_moon_takes_over_at_night_and_light_fades_at_the_horizon() {
        let environment = Environment::default();
        let noon = Lighting::at(&environment, 12.0, FULL);
        assert_eq!(noon.light_dir, noon.sun);
        assert!(noon.light_fade > 0.99);

        let midnight = Lighting::at(&environment, 0.0, FULL);
        assert!(midnight.light_dir.y > 0.5, "the moon is up: {}", midnight.light_dir);
        assert!(midnight.light_color[0] < noon.light_color[0] * 0.5);
        assert!(midnight.zenith[2] < noon.zenith[2]);

        let sunrise = Lighting::at(&environment, 6.0, FULL);
        assert!(sunrise.light_fade < 0.01 && sunrise.light_color[0] < 0.01);
        assert!(sunrise.glow[3] > 0.5, "the horizon glows at dawn");
        assert!(noon.glow[3] < 0.05);
    }

    #[test]
    fn moonless_nights_are_dark_and_cast_no_shadows() {
        let environment = Environment::default();
        let full = Lighting::at(&environment, 0.0, FULL);
        let new = Lighting::at(&environment, 0.0, 0.0);
        assert!(new.light_color.iter().all(|c| *c < 1e-4), "no moonlight: {:?}", new.light_color);
        assert!(new.light_fade < 1e-4, "so no shadows to draw");
        assert!(new.sky_ambient[2] < full.sky_ambient[2] * 0.25);
        assert!(new.zenith[2] < full.zenith[2] * 0.25 && new.horizon[2] < full.horizon[2]);
        let quarter = Lighting::at(&environment, 0.0, 0.25);
        assert!((quarter.moon_phase[0] - 0.5).abs() < 1e-4);
        assert!(quarter.light_color[2] > new.light_color[2] && quarter.light_color[2] < full.light_color[2]);
        // De día la luna no cambia nada.
        let noon = (Lighting::at(&environment, 12.0, FULL), Lighting::at(&environment, 12.0, 0.0));
        assert_eq!(noon.0.zenith, noon.1.zenith);
        assert_eq!(noon.0.light_color, noon.1.light_color);
    }

    #[test]
    fn broken_values_are_brought_back() {
        let environment = Environment { sun_tilt: f32::NAN, shadow_strength: 4.0, ..Environment::default() }.sanitized();
        assert_eq!(environment.sun_tilt, Environment::default().sun_tilt);
        assert_eq!(environment.shadow_strength, 1.0);
    }
}
