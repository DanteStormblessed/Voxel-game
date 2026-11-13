// Terreno: un quad por instancia (8 bytes), 4 vértices generados aquí.
//
// Formato de la instancia (ver ChunkMesh en mesher.rs). El bit 31 de quad.y distingue
// un cubo (caras de voxel) de un quad de mini-voxeles (modelos, 1/8 de voxel):
//   cubo  quad.x  x:5 | y:5 | z:5 | ancho-1:5 | alto-1:5 | cara:3 | aristas:4
//         quad.y  bloque:16 | ao:8 | luz:4 | libre:3 | 0
//   mini  quad.x  celda xyz:5·3 | desplazamiento xyz:3·3 | ancho-1:3 | alto-1:3
//         quad.y  color:20 | cara:3 | libre:1 | luz:4 | libre:3 | 1

struct Globals {
    view_proj: mat4x4<f32>,
    // Chunk en el que está la cámara. Todo se calcula relativo a su origen.
    cam_chunk: vec4<i32>,
    cam_local: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    sky_ambient: vec4<f32>,
    ground_ambient: vec4<f32>,
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
};

struct ChunkData {
    // xyz: coordenadas del chunk (en chunks de 32 voxeles, no en voxeles).
    // w: tamaño de celda en voxeles (1 = chunk real; 2, 4... = terreno lejano).
    origin: vec4<i32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
// Paleta: 8 entradas por número de bloque (una por cara) y después los colores de los
// modelos. La entrada n está en (n % PALETTE_WIDTH, n / PALETTE_WIDTH). Textura sRGB: el
// color llega en lineal; el alfa es la opacidad de los bloques translúcidos.
@group(0) @binding(1) var palette: texture_2d<f32>;
const PALETTE_WIDTH: u32 = 2048u;
// Lo que se aclara un bloque que emite luz con su máximo (15).
const EMISSIVE_BOOST: f32 = 1.6;
@group(1) @binding(0) var<uniform> chunk: ChunkData;

const CHUNK_SIZE: i32 = 32;

// Tablas por cara: +X, -X, +Y, -Y, +Z, -Z. Siempre U × V = normal.
fn face_u(face: u32) -> vec3<f32> {
    var table = array<vec3<f32>, 6>(
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0),
    );
    return table[face];
}

fn face_v(face: u32) -> vec3<f32> {
    var table = array<vec3<f32>, 6>(
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(1.0, 0.0, 0.0),
    );
    return table[face];
}

// Cuánto baja un punto por la curvatura del mundo, según su distancia horizontal a la cámara.
fn curvature_drop(rel: vec3<f32>) -> f32 {
    let d = rel.xz - globals.cam_local.xz;
    return dot(d, d) * globals.fog_params.y;
}

// Perspectiva aérea: lo lejano se tiñe del color del cielo poco a poco y, al llegar al
// borde del terreno visible, se funde con él del todo. Se mide desde el jugador.
fn haze(rel: vec3<f32>) -> f32 {
    let t = distance(rel.xz, globals.focus.xz) / globals.fog_params.x;
    let aerial = globals.focus.w * (1.0 - exp(-2.5 * t));
    return mix(aerial, 1.0, smoothstep(globals.fog_params.w, 1.0, t));
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // Posición relativa al origen del chunk de la cámara.
    @location(0) rel_pos: vec3<f32>,
    // Posición dentro del quad, en voxeles: (0,0) .. (ancho, alto).
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) size: vec2<f32>,
    // Color lineal y opacidad.
    @location(3) @interpolate(flat) color: vec4<f32>,
    // Oclusión de las 4 esquinas (0..1): (0,0), (1,0), (0,1), (1,1).
    @location(4) @interpolate(flat) ao: vec4<f32>,
    @location(5) @interpolate(flat) tan_u: vec3<f32>,
    @location(6) @interpolate(flat) tan_v: vec3<f32>,
    // 1 si el lado es una arista convexa: (-U, +U, -V, +V).
    @location(7) @interpolate(flat) edges: vec4<f32>,
    // 1 si es terreno lejano.
    @location(8) @interpolate(flat) lod: f32,
    // Luz propia del bloque (0..1): brilla sin sombra.
    @location(9) @interpolate(flat) glow: f32,
};

fn palette_color(index: u32) -> vec4<f32> {
    return textureLoad(palette, vec2<u32>(index % PALETTE_WIDTH, index / PALETTE_WIDTH), 0);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @location(0) quad: vec2<u32>) -> VsOut {
    let a = quad.x;
    let b = quad.y;
    let mini = (b >> 31u) == 1u;

    // Origen y tamaño en voxeles; `depth` es el grosor de la celda en la dirección normal.
    var origin: vec3<f32>;
    var size: vec2<f32>;
    var face: u32;
    var edge_bits = 0u;
    var ao_bits = vec4<u32>(3u);
    var color_index: u32;
    var depth = 1.0;
    if mini {
        let unit = 1.0 / 8.0;
        let cell = vec3<f32>(vec3<u32>(a, a >> 5u, a >> 10u) & vec3<u32>(31u));
        let offset = vec3<f32>(vec3<u32>(a >> 15u, a >> 18u, a >> 21u) & vec3<u32>(7u));
        origin = cell + offset * unit;
        size = (vec2<f32>(vec2<u32>(a >> 24u, a >> 27u) & vec2<u32>(7u)) + vec2<f32>(1.0)) * unit;
        face = min((b >> 20u) & 7u, 5u);
        color_index = b & 0xFFFFFu;
        depth = unit;
    } else {
        origin = vec3<f32>(vec3<u32>(a, a >> 5u, a >> 10u) & vec3<u32>(31u));
        size = vec2<f32>(vec2<u32>(a >> 15u, a >> 20u) & vec2<u32>(31u)) + vec2<f32>(1.0);
        face = min((a >> 25u) & 7u, 5u);
        edge_bits = a >> 28u;
        ao_bits = vec4<u32>(b >> 16u, b >> 18u, b >> 20u, b >> 22u) & vec4<u32>(3u);
        color_index = (b & 0xFFFFu) * 8u + face;
    }

    let tan_u = face_u(face);
    let tan_v = face_v(face);
    let normal = cross(tan_u, tan_v);

    // Triangle strip: (0,0) (1,0) (0,1) (1,1).
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u));

    // Esquina (0,0) del quad, relativa al chunk de la cámara. Las caras positivas están
    // en el lado lejano de su celda.
    let scale = f32(chunk.origin.w);
    let base = vec3<f32>((chunk.origin.xyz - globals.cam_chunk.xyz) * CHUNK_SIZE)
        + (origin + max(normal, vec3<f32>(0.0)) * depth) * scale;

    // El quad crece una fracción de píxel hacia fuera para tapar las uniones en T que
    // deja el mallado greedy (ver QUAD_GROW_PIXELS). Lo translúcido no: el solape se
    // mezclaría dos veces y se vería una línea en cada costura.
    let color = palette_color(color_index);
    let exact = base + (tan_u * corner.x * size.x + tan_v * corner.y * size.y) * scale;
    var grow = distance(exact, globals.cam_local.xyz) * globals.params.z / scale;
    if color.a < 1.0 {
        grow = 0.0;
    }
    let uv = corner * size + (corner * 2.0 - 1.0) * grow;
    var rel = base + (tan_u * uv.x + tan_v * uv.y) * scale;
    rel.y -= curvature_drop(rel);

    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(rel, 1.0);
    out.rel_pos = rel;
    out.uv = uv;
    out.size = size;
    out.color = color;
    out.glow = f32((b >> 24u) & 15u) / 15.0;
    out.ao = vec4<f32>(ao_bits) / 3.0;
    out.tan_u = tan_u;
    out.tan_v = tan_v;
    out.lod = f32(chunk.origin.w > 1);
    out.edges = vec4<f32>(vec4<u32>(edge_bits, edge_bits >> 1u, edge_bits >> 2u, edge_bits >> 3u) & vec4<u32>(1u));
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let flat_normal = cross(in.tan_u, in.tan_v);

    // --- Bisel -----------------------------------------------------------------------
    // Cerca de una arista convexa la normal se inclina hacia fuera hasta 45° justo en el
    // borde. La cara vecina hace lo mismo desde su lado, así que la iluminación es
    // continua a través de la arista y esta se percibe redondeada.
    let radius = globals.params.x;
    let to_low = in.uv;
    let to_high = in.size - in.uv;
    let t_low = clamp(1.0 - to_low / radius, vec2<f32>(0.0), vec2<f32>(1.0)) * in.edges.xz;
    let t_high = clamp(1.0 - to_high / radius, vec2<f32>(0.0), vec2<f32>(1.0)) * in.edges.yw;

    // Cuando el bisel mide menos de un píxel se desvanece en vez de centellear.
    let footprint = fwidth(in.uv);
    let fade = clamp(2.0 - max(footprint.x, footprint.y) / radius, 0.0, 1.0);
    let tilt = (t_high * t_high - t_low * t_low) * fade;
    let normal = normalize(flat_normal + in.tan_u * tilt.x + in.tan_v * tilt.y);

    // --- Oclusión ambiental ----------------------------------------------------------
    let f = clamp(in.uv / in.size, vec2<f32>(0.0), vec2<f32>(1.0));
    let ao_raw = mix(mix(in.ao.x, in.ao.y, f.x), mix(in.ao.z, in.ao.w, f.x), f.y);
    let ao = mix(globals.params.y, 1.0, ao_raw);

    // --- Luz: sol direccional + ambiente hemisférico ---------------------------------
    let sun = max(dot(normal, globals.sun_dir.xyz), 0.0);
    let ambient = mix(globals.ground_ambient.rgb, globals.sky_ambient.rgb, normal.y * 0.5 + 0.5);
    var color = in.color.rgb * (ambient + globals.sun_color.rgb * sun) * ao;
    // Los bloques que emiten luz no tienen sombra: se ven de su color, más claro.
    color = mix(color, in.color.rgb * EMISSIVE_BOOST, in.glow);

    // --- Bruma de la distancia ---------------------------------------------------------
    color = mix(color, globals.fog_color.rgb, haze(in.rel_pos));
    let dist = distance(in.rel_pos.xz, globals.cam_local.xz);

    // --- Relevo entre voxeles reales y terreno lejano ---------------------------------
    // Los voxeles ocupan un cilindro: hasta `clip` en horizontal y entre las alturas de
    // `voxel_band`. El terreno lejano se dibuja fuera de él, con un poco de solape, así
    // que también cubre lo que queda muy por debajo o por encima del jugador.
    let clip = globals.params.w;
    let overlap = globals.fog_params.z;
    let y = in.rel_pos.y;
    let inside = dist < clip - overlap
        && y > globals.voxel_band.x + overlap
        && y < globals.voxel_band.y - overlap;
    if (in.lod > 0.5 && inside) || (in.lod < 0.5 && dist > clip) {
        discard;
    }

    return vec4<f32>(color, in.color.a);
}
