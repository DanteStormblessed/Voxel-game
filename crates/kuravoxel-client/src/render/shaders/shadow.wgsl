// Mapas de sombras: la escena vista desde la luz, solo profundidad. Una pasada por
// cascada. De cada chunk solo se dibujan las caras que dan la espalda a la luz (las elige
// la CPU): lo que está iluminado nunca se compara consigo mismo, así que no hace falta
// casi sesgo y las sombras tocan lo que las proyecta.

struct ShadowGlobals {
    // Posición relativa al chunk de la cámara → espacio de recorte de la cascada.
    light_view_proj: mat4x4<f32>,
    cam_chunk: vec4<i32>,
    // xyz: cámara relativa a su chunk; w: 1 / (2 · radio de curvatura)
    cam_local: vec4<f32>,
};

struct ChunkData {
    origin: vec4<i32>,
};

@group(0) @binding(0) var<uniform> shadow: ShadowGlobals;
@group(1) @binding(0) var<uniform> chunk: ChunkData;

const CHUNK_SIZE: i32 = 32;

// La misma curvatura que en la escena: la sombra cae donde se ve lo que la proyecta.
fn curved(rel: vec3<f32>) -> vec4<f32> {
    let d = rel.xz - shadow.cam_local.xz;
    return vec4<f32>(rel.x, rel.y - dot(d, d) * shadow.cam_local.w, rel.z, 1.0);
}

// Terreno: mismo formato de quad que terrain.wgsl.
@vertex
fn vs_terrain(@builtin(vertex_index) vertex: u32, @location(0) quad: vec2<u32>) -> @builtin(position) vec4<f32> {
    let a = quad.x;
    let b = quad.y;
    var origin: vec3<f32>;
    var size: vec2<f32>;
    var face: u32;
    var depth = 1.0;
    if (b >> 31u) == 1u {
        let unit = 1.0 / 8.0;
        let cell = vec3<f32>(vec3<u32>(a, a >> 5u, a >> 10u) & vec3<u32>(31u));
        let offset = vec3<f32>(vec3<u32>(a >> 15u, a >> 18u, a >> 21u) & vec3<u32>(7u));
        origin = cell + offset * unit;
        size = (vec2<f32>(vec2<u32>(a >> 24u, a >> 27u) & vec2<u32>(7u)) + vec2<f32>(1.0)) * unit;
        face = min((b >> 20u) & 7u, 5u);
        depth = unit;
    } else {
        origin = vec3<f32>(vec3<u32>(a, a >> 5u, a >> 10u) & vec3<u32>(31u));
        size = vec2<f32>(vec2<u32>(a >> 15u, a >> 20u) & vec2<u32>(31u)) + vec2<f32>(1.0);
        face = min((a >> 25u) & 7u, 5u);
    }
    let tan_u = face_u(face);
    let tan_v = face_v(face);
    let normal = cross(tan_u, tan_v);
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u));
    let scale = f32(chunk.origin.w);
    let rel = vec3<f32>((chunk.origin.xyz - shadow.cam_chunk.xyz) * CHUNK_SIZE)
        + (origin + max(normal, vec3<f32>(0.0)) * depth + tan_u * corner.x * size.x + tan_v * corner.y * size.y) * scale;
    return shadow.light_view_proj * curved(rel);
}

// Entidades: mismas instancias que entity.wgsl.
@vertex
fn vs_entity(
    @builtin(vertex_index) vertex: u32,
    @location(0) center_yaw: vec4<f32>,
    @location(1) size: vec4<f32>,
    @location(2) color: vec4<f32>,
) -> @builtin(position) vec4<f32> {
    var corners = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u);
    let face = vertex / 6u;
    let corner = corners[vertex % 6u];
    let tan_u = face_u(face);
    let tan_v = face_v(face);
    let normal = cross(tan_u, tan_v);
    let cu = f32(corner & 1u) - 0.5;
    let cv = f32((corner >> 1u) & 1u) - 0.5;
    let local = (normal * 0.5 + tan_u * cu + tan_v * cv) * size.xyz;
    return shadow.light_view_proj * curved(center_yaw.xyz + rotate_y(local, center_yaw.w));
}
