// Entidades: una caja orientada por instancia, 36 vértices generados aquí.

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) color: vec3<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex: u32,
    @location(0) center_yaw: vec4<f32>,
    @location(1) size: vec4<f32>,
    @location(2) color: vec4<f32>,
) -> VsOut {
    var corners = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u);
    let face = vertex / 6u;
    let corner = corners[vertex % 6u];
    let tan_u = face_u(face);
    let tan_v = face_v(face);
    let normal = cross(tan_u, tan_v);
    let cu = f32(corner & 1u) - 0.5;
    let cv = f32((corner >> 1u) & 1u) - 0.5;

    let local = (normal * 0.5 + tan_u * cu + tan_v * cv) * size.xyz;
    var rel = center_yaw.xyz + rotate_y(local, center_yaw.w);
    rel.y -= curvature_drop(rel);

    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(rel, 1.0);
    out.rel_pos = rel;
    out.normal = rotate_y(normal, center_yaw.w);
    out.color = color.rgb;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let normal = normalize(in.normal);
    let color = in.color * surface_light(normal, normal, in.rel_pos);
    return vec4<f32>(apply_haze(color, in.rel_pos), 1.0);
}
