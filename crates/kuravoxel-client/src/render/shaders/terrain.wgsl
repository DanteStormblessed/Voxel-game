// Terreno: un quad por instancia (8 bytes), 4 vértices generados aquí.
//
// Formato de la instancia (ver mesher.rs):
//   quad.x  x:5 | y:5 | z:5 | ancho-1:5 | alto-1:5 | cara:3 | aristas:4
//   quad.y  bloque:8 | ao:8

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
    // x: inicio de la niebla, y: 1 / (fin - inicio), z: solape voxeles / terreno lejano
    fog_params: vec4<f32>,
    // x: radio del bisel en voxeles, y: brillo mínimo con oclusión total,
    // z: crecimiento del quad por unidad de distancia,
    // w: distancia a la que los voxeles reales ceden el paso al terreno lejano
    params: vec4<f32>,
};

struct Palette {
    colors: array<vec4<f32>, 256>,
};

struct ChunkData {
    // xyz: coordenadas del chunk (en chunks de 32 voxeles, no en voxeles).
    // w: tamaño de celda en voxeles (1 = chunk real; 2, 4... = terreno lejano).
    origin: vec4<i32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var<uniform> palette: Palette;
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

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // Posición relativa al origen del chunk de la cámara.
    @location(0) rel_pos: vec3<f32>,
    // Posición dentro del quad, en voxeles: (0,0) .. (ancho, alto).
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) size: vec2<f32>,
    @location(3) @interpolate(flat) color: vec3<f32>,
    // Oclusión de las 4 esquinas (0..1): (0,0), (1,0), (0,1), (1,1).
    @location(4) @interpolate(flat) ao: vec4<f32>,
    @location(5) @interpolate(flat) tan_u: vec3<f32>,
    @location(6) @interpolate(flat) tan_v: vec3<f32>,
    // 1 si el lado es una arista convexa: (-U, +U, -V, +V).
    @location(7) @interpolate(flat) edges: vec4<f32>,
    // 1 si es terreno lejano.
    @location(8) @interpolate(flat) lod: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @location(0) quad: vec2<u32>) -> VsOut {
    let a = quad.x;
    let b = quad.y;

    let origin = vec3<f32>(vec3<u32>(a, a >> 5u, a >> 10u) & vec3<u32>(31u));
    let size = vec2<f32>(vec2<u32>(a >> 15u, a >> 20u) & vec2<u32>(31u)) + vec2<f32>(1.0);
    let face = min((a >> 25u) & 7u, 5u);
    let edge_bits = a >> 28u;

    let tan_u = face_u(face);
    let tan_v = face_v(face);
    let normal = cross(tan_u, tan_v);

    // Triangle strip: (0,0) (1,0) (0,1) (1,1).
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u));

    // Esquina (0,0) del quad, relativa al chunk de la cámara. Las caras positivas están
    // en el lado lejano del voxel.
    let scale = f32(chunk.origin.w);
    let base = vec3<f32>((chunk.origin.xyz - globals.cam_chunk.xyz) * CHUNK_SIZE)
        + (origin + max(normal, vec3<f32>(0.0))) * scale;

    // El quad crece una fracción de píxel hacia fuera para tapar las uniones en T que
    // deja el mallado greedy (ver QUAD_GROW_PIXELS).
    let exact = base + (tan_u * corner.x * size.x + tan_v * corner.y * size.y) * scale;
    let grow = distance(exact, globals.cam_local.xyz) * globals.params.z / scale;
    let uv = corner * size + (corner * 2.0 - 1.0) * grow;
    let rel = base + (tan_u * uv.x + tan_v * uv.y) * scale;

    let ao_bits = vec4<u32>(b >> 8u, b >> 10u, b >> 12u, b >> 14u) & vec4<u32>(3u);

    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(rel, 1.0);
    out.rel_pos = rel;
    out.uv = uv;
    out.size = size;
    out.color = palette.colors[b & 255u].rgb;
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
    var color = in.color * (ambient + globals.sun_color.rgb * sun) * ao;

    // --- Niebla por distancia horizontal (oculta el borde del área cargada) -----------
    let dist = distance(in.rel_pos.xz, globals.cam_local.xz);
    let fog = clamp((dist - globals.fog_params.x) * globals.fog_params.y, 0.0, 1.0);
    color = mix(color, globals.fog_color.rgb, fog * fog);

    // --- Relevo entre voxeles reales y terreno lejano ---------------------------------
    // Los voxeles se dibujan hasta `clip`; el terreno lejano, desde un poco antes.
    let clip = globals.params.w;
    if (in.lod > 0.5 && dist < clip - globals.fog_params.z) || (in.lod < 0.5 && dist > clip) {
        discard;
    }

    return vec4<f32>(color, 1.0);
}
