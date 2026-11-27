// Lo que comparten todos los shaders de la escena: los uniforms del frame, la luz (sol o
// luna + ambiente), las sombras en cascada y la bruma. Va delante de cada shader (ver
// `scene_shader` en render/mod.rs).

// Uniforms por frame. Debe coincidir con `Globals` en render/mod.rs.
struct Globals {
    view_proj: mat4x4<f32>,
    // Chunk en el que está la cámara. Todo se calcula relativo a su origen.
    cam_chunk: vec4<i32>,
    cam_local: vec4<f32>,
    // xyz: hacia la luz principal (el sol o, de noche, la luna); w: cuánto oscurecen las
    // sombras (0 = sin sombras)
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    sky_ambient: vec4<f32>,
    ground_ambient: vec4<f32>,
    // Color del cielo en el horizonte: con él se funde el terreno lejano.
    fog_color: vec4<f32>,
    // x: distancia a la que la bruma tapa del todo el terreno, y: 1 / (2 · radio de
    // curvatura), z: solape voxeles / terreno lejano, w: fracción de x en la que empieza
    // el fundido final con el cielo
    fog_params: vec4<f32>,
    // x: radio del bisel en voxeles, y: brillo mínimo con oclusión total,
    // z: crecimiento del quad por unidad de distancia,
    // w: distancia a la que los voxeles reales ceden el paso al terreno lejano
    params: vec4<f32>,
    // xyz: punto en el que se centra la vista (el jugador); w: intensidad de la bruma
    focus: vec4<f32>,
    // x, y: alturas (relativas al chunk de la cámara) entre las que hay voxeles reales
    voxel_band: vec4<f32>,
    // rgb: color del cielo en el cénit; w: exponente del degradado hacia el horizonte
    sky_zenith: vec4<f32>,
    // rgb: resplandor del horizonte en la dirección del sol; w: intensidad
    sky_glow: vec4<f32>,
    // xyz: hacia el sol (la luna está justo enfrente)
    sun_pos: vec4<f32>,
    // rgb: color del disco; w: cuánto se ve
    sun_disc: vec4<f32>,
    moon_disc: vec4<f32>,
    // x: parte iluminada de la luna (0 = nueva, 1 = llena); y, z: coseno y seno del
    // ángulo de fase.
    moon_phase: vec4<f32>,
    // Ejes de la cámara; derecha y arriba ya escalados por el campo de visión, así que
    // `forward + x · right + y · up` es el rayo del píxel (x, y) en [-1, 1].
    cam_right: vec4<f32>,
    cam_up: vec4<f32>,
    cam_forward: vec4<f32>,
    // Posición relativa al chunk de la cámara → espacio de cada cascada de sombras.
    shadow_mats: array<mat4x4<f32>, 3>,
    // x: 1 / lado del mapa en texels, y: muestras del filtro a cada lado del centro,
    // z: radio de la penumbra en metros, w: sesgo de profundidad
    shadow_params: vec4<f32>,
    // Metros por texel de cada cascada.
    shadow_texel: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(2) var shadow_map: texture_depth_2d_array;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;

const SHADOW_CASCADES: i32 = 3;
// Franja del borde de una cascada (en coordenadas de su mapa) en la que se mezcla con la
// siguiente, para que no se note el cambio de resolución.
const CASCADE_BLEND: f32 = 0.06;

// Cuánto baja un punto por la curvatura del mundo, según su distancia horizontal a la cámara.
fn curvature_drop(rel: vec3<f32>) -> f32 {
    let d = rel.xz - globals.cam_local.xz;
    return dot(d, d) * globals.fog_params.y;
}

// Color del horizonte mirando hacia `dir`: el del cielo y, al amanecer y al atardecer, un
// resplandor en la dirección del sol.
fn horizon_color(dir: vec3<f32>) -> vec3<f32> {
    let flat_dir = dir.xz / max(length(dir.xz), 0.0001);
    let sun = globals.sun_pos.xz / max(length(globals.sun_pos.xz), 0.0001);
    let toward = max(dot(flat_dir, sun), 0.0);
    return mix(globals.fog_color.rgb, globals.sky_glow.rgb, globals.sky_glow.w * toward * toward * toward);
}

// Perspectiva aérea: lo lejano se tiñe del color del horizonte poco a poco y, al llegar
// al borde del terreno visible, se funde con él del todo. Se mide desde el jugador.
fn apply_haze(color: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    let t = distance(rel.xz, globals.focus.xz) / globals.fog_params.x;
    let aerial = globals.focus.w * (1.0 - exp(-2.5 * t));
    let amount = mix(aerial, 1.0, smoothstep(globals.fog_params.w, 1.0, t));
    return mix(color, horizon_color(rel - globals.cam_local.xyz), amount);
}

// Luz que llega a `rel` en una cascada. x: fracción iluminada (0 = en sombra); y: distancia
// al borde del mapa (negativa si el punto queda fuera de la cascada).
fn cascade_light(cascade: i32, rel: vec3<f32>, normal: vec3<f32>) -> vec2<f32> {
    let texel = globals.shadow_texel[cascade];
    let inv_size = globals.shadow_params.x;
    let half = i32(globals.shadow_params.y);
    // El punto se separa un poco de su cara: así una superficie no se da sombra a sí misma.
    let pos = rel + normal * (texel * 2.0 + 0.01);
    let light = globals.shadow_mats[cascade] * vec4<f32>(pos, 1.0);
    let uv = light.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);

    // Penumbra de ancho constante en metros, dentro de lo que el filtro puede dar sin
    // que se vean sus muestras.
    let radius = clamp(globals.shadow_params.z / texel, 0.5, 1.5 * f32(half));
    let spacing = radius / f32(half) * inv_size;
    let margin = (radius + 1.5) * inv_size;
    let edge = min(min(uv.x, uv.y), min(1.0 - uv.x, 1.0 - uv.y)) - margin;
    if edge < 0.0 || light.z <= 0.0 || light.z >= 1.0 {
        return vec2<f32>(1.0, -1.0);
    }

    let reference = light.z - globals.shadow_params.w;
    // Primero las cuatro esquinas del filtro: si coinciden, el punto está del todo al sol
    // o del todo en sombra y el resto de muestras no cambiaría nada. Solo la penumbra
    // paga el filtro entero. Leer el mapa es lo más caro del frame (caché de texturas).
    let reach = f32(half) * spacing;
    let corners = textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(-reach, -reach), cascade, reference)
        + textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(reach, -reach), cascade, reference)
        + textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(-reach, reach), cascade, reference)
        + textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(reach, reach), cascade, reference);
    if corners == 0.0 || corners == 4.0 {
        return vec2<f32>(corners * 0.25, edge);
    }
    var sum = 0.0;
    for (var y = -half; y <= half; y++) {
        for (var x = -half; x <= half; x++) {
            let offset = vec2<f32>(f32(x), f32(y)) * spacing;
            sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + offset, cascade, reference);
        }
    }
    let side = f32(2 * half + 1);
    return vec2<f32>(sum / (side * side), edge);
}

// Fracción de la luz principal que llega a `rel` (1 = sin sombra). `normal` es la de la
// cara, sin bisel.
fn sun_visibility(rel: vec3<f32>, normal: vec3<f32>) -> f32 {
    let strength = globals.sun_dir.w;
    if strength <= 0.0 {
        return 1.0;
    }
    var lit = 1.0;
    for (var i = 0; i < SHADOW_CASCADES; i++) {
        let here = cascade_light(i, rel, normal);
        if here.y < 0.0 {
            continue;
        }
        lit = here.x;
        if here.y < CASCADE_BLEND {
            // Cerca del borde: se mezcla con la cascada siguiente o, en la última, con
            // "sin sombra", que es lo que hay más allá.
            var beyond = 1.0;
            if i + 1 < SHADOW_CASCADES {
                let next = cascade_light(i + 1, rel, normal);
                if next.y >= 0.0 {
                    beyond = next.x;
                }
            }
            lit = mix(beyond, lit, here.y / CASCADE_BLEND);
        }
        break;
    }
    return mix(1.0, lit, strength);
}

// Luz que recibe una superficie: ambiente hemisférico + luz principal con su sombra.
// `normal` puede llevar bisel; `flat_normal` es la de la cara.
fn surface_light(normal: vec3<f32>, flat_normal: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    var sun = max(dot(normal, globals.sun_dir.xyz), 0.0);
    if sun > 0.0 {
        sun *= sun_visibility(rel, flat_normal);
    }
    let ambient = mix(globals.ground_ambient.rgb, globals.sky_ambient.rgb, normal.y * 0.5 + 0.5);
    return ambient + globals.sun_color.rgb * sun;
}
