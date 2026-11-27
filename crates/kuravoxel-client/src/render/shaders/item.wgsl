// Ítems sueltos: la forma de un ítem (caras de voxel, ver icons.rs), una cara por
// instancia y 4 vértices generados aquí, con una matriz por ítem que la coloca, la gira
// y la escala.

struct ItemData {
    // Voxel de la forma → posición relativa al chunk de la cámara.
    model: mat4x4<f32>,
};

@group(0) @binding(1) var palette: texture_2d<f32>;
@group(1) @binding(0) var<uniform> item: ItemData;

// Igual que en terrain.wgsl.
const PALETTE_WIDTH: u32 = 2048u;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) normal: vec3<f32>,
    @location(1) @interpolate(flat) color: vec3<f32>,
    @location(2) rel_pos: vec3<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex: u32,
    @location(0) voxel_face: vec4<u32>,
    @location(1) color_index: u32,
) -> VsOut {
    let face = min(voxel_face.w, 5u);
    let tan_u = face_u(face);
    let tan_v = face_v(face);
    let normal = cross(tan_u, tan_v);
    // Triangle strip: (0,0) (1,0) (0,1) (1,1). Las caras positivas, en el lado lejano.
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u));
    let local = vec3<f32>(voxel_face.xyz) + max(normal, vec3<f32>(0.0)) + tan_u * corner.x + tan_v * corner.y;

    let rel = item.model * vec4<f32>(local, 1.0);
    var out: VsOut;
    out.clip = globals.view_proj * rel;
    out.rel_pos = rel.xyz;
    out.normal = normalize((item.model * vec4<f32>(normal, 0.0)).xyz);
    out.color = textureLoad(palette, vec2<u32>(color_index % PALETTE_WIDTH, color_index / PALETTE_WIDTH), 0).rgb;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let color = in.color * surface_light(in.normal, in.normal, in.rel_pos);
    return vec4<f32>(apply_haze(color, in.rel_pos), 1.0);
}
