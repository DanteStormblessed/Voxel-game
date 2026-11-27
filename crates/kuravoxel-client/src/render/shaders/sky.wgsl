// Cielo: un triángulo que cubre la pantalla, dibujado al fondo (donde no hay nada). Un
// degradado del horizonte al cénit, con el sol y, enfrente, la luna.

// Coseno del radio aparente de cada disco (~2,6° y ~2°: mayores que los reales, como en
// un dibujo) y ancho de su borde.
const SUN_COS: f32 = 0.99897;
const MOON_COS: f32 = 0.99939;
const DISC_EDGE: f32 = 0.00012;
// Lo que se ve la parte en sombra de la luna (luz que le llega reflejada de la Tierra).
const EARTHSHINE: f32 = 0.015;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> VsOut {
    // (-1, -1), (3, -1), (-1, 3)
    let p = vec2<f32>(f32((vertex << 1u) & 2u), f32(vertex & 2u)) * 2.0 - vec2<f32>(1.0);
    var out: VsOut;
    // Z invertida: 0 es el fondo.
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dir = normalize(
        globals.cam_forward.xyz + globals.cam_right.xyz * in.ndc.x + globals.cam_up.xyz * in.ndc.y
    );
    let up = clamp(dir.y, 0.0, 1.0);
    var color = mix(horizon_color(dir), globals.sky_zenith.rgb, 1.0 - pow(1.0 - up, globals.sky_zenith.w));

    // Por debajo del horizonte no hay astros (ahí va el terreno, o su bruma).
    let above = smoothstep(-0.03, 0.01, dir.y);
    let toward_sun = dot(dir, globals.sun_pos.xyz);

    let sun = globals.sun_disc.w * above;
    let halo = pow(max(toward_sun, 0.0), 220.0) * 0.30 + pow(max(toward_sun, 0.0), 14.0) * 0.07;
    color += globals.sun_disc.rgb * halo * sun;
    color = mix(color, globals.sun_disc.rgb, smoothstep(SUN_COS - DISC_EDGE, SUN_COS + DISC_EDGE, toward_sun) * sun);

    let moon = globals.moon_disc.w * above;
    let moon_halo = pow(max(-toward_sun, 0.0), 300.0) * 0.12 * globals.moon_phase.x;
    color += globals.moon_disc.rgb * moon_halo * moon;
    let disc = smoothstep(MOON_COS - DISC_EDGE, MOON_COS + DISC_EDGE, -toward_sun) * moon;
    if disc > 0.0 {
        // Fase: el disco es una esfera vista de frente, iluminada desde un lado que gira
        // con la edad de la luna (de espaldas en luna nueva, de frente en luna llena).
        let moon_dir = -globals.sun_pos.xyz;
        var side = cross(moon_dir, vec3<f32>(0.0, 1.0, 0.0));
        if length(side) < 0.001 {
            side = vec3<f32>(1.0, 0.0, 0.0);
        }
        side = normalize(side);
        let rise = cross(side, moon_dir);
        let radius = sqrt(1.0 - MOON_COS * MOON_COS);
        let p = vec2<f32>(dot(dir, side), dot(dir, rise)) / radius;
        let normal = vec3<f32>(p, sqrt(max(1.0 - dot(p, p), 0.0)));
        let toward_light = vec3<f32>(globals.moon_phase.z, 0.0, -globals.moon_phase.y);
        let lit = smoothstep(-0.06, 0.06, dot(normal, toward_light));
        color = mix(color, globals.moon_disc.rgb * mix(EARTHSHINE, 1.0, lit), disc);
    }

    return vec4<f32>(color, 1.0);
}
