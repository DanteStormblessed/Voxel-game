//! Ruido de gradiente 2D propio: sin dependencias, sin tablas, determinista por semilla.
//! Las coordenadas entran en f64 para que el terreno no se degrade lejos del origen.

#[inline(always)]
pub fn hash2(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x85EB_CA6B)
        ^ (y as u32).wrapping_mul(0xC2B2_AE35)
        ^ seed.wrapping_mul(0x27D4_EB2F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h
}

#[inline(always)]
fn grad(hash: u32, x: f32, y: f32) -> f32 {
    match hash & 7 {
        0 => x + y,
        1 => x - y,
        2 => y - x,
        3 => -x - y,
        4 => x,
        5 => -x,
        6 => y,
        _ => -y,
    }
}

#[inline(always)]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline(always)]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Ruido Perlin 2D, aproximadamente en [-1, 1].
pub fn perlin2(x: f64, y: f64, seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (xi, yi) = (xf as i64 as i32, yf as i64 as i32);
    let (fx, fy) = ((x - xf) as f32, (y - yf) as f32);
    let (xi1, yi1) = (xi.wrapping_add(1), yi.wrapping_add(1));

    let a = grad(hash2(xi, yi, seed), fx, fy);
    let b = grad(hash2(xi1, yi, seed), fx - 1.0, fy);
    let c = grad(hash2(xi, yi1, seed), fx, fy - 1.0);
    let d = grad(hash2(xi1, yi1, seed), fx - 1.0, fy - 1.0);

    let (u, v) = (fade(fx), fade(fy));
    lerp(lerp(a, b, u), lerp(c, d, u), v)
}

/// Suma fractal de octavas, normalizada a aproximadamente [-1, 1].
pub fn fbm2(x: f64, y: f64, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amplitude, mut norm) = (0.0f32, 1.0f32, 0.0f32);
    let mut frequency = 1.0f64;
    for octave in 0..octaves {
        let octave_seed = seed.wrapping_add(octave.wrapping_mul(0x9E37_79B9));
        sum += perlin2(x * frequency, y * frequency, octave_seed) * amplitude;
        norm += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    sum / norm
}

#[inline(always)]
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
