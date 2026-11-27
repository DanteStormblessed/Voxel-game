// Marcas sobre el mundo (selección): cajas translúcidas que se ven a través de los bloques.

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) color: vec3<f32>,
    // x: opacidad; y: 1 si la caja lleva rejilla de bloques.
    @location(3) @interpolate(flat) params: vec2<f32>,
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
    out.params = vec2<f32>(color.a, size.w);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var alpha = in.params.x;
    if (in.params.y > 0.5) {
        // Rejilla: una línea fina en cada límite entre bloques. Las coordenadas son
        // relativas a un chunk, así que su parte fraccionaria es la del mundo.
        let n = abs(normalize(in.normal));
        let p = in.rel_pos;
        let w = max(fwidth(p), vec3<f32>(0.00001));
        let d = abs(fract(p + vec3<f32>(0.5)) - vec3<f32>(0.5)) / w;
        var edge = vec3<f32>(1.0) - clamp(d - vec3<f32>(0.25), vec3<f32>(0.0), vec3<f32>(1.0));
        // La coordenada perpendicular a la cara no dibuja línea.
        edge = edge * (vec3<f32>(1.0) - step(vec3<f32>(0.5), n));
        let grid = max(edge.x, max(edge.y, edge.z));
        alpha = mix(alpha, min(alpha * 3.0 + 0.12, 0.7), grid);
    }
    return vec4<f32>(in.color, alpha);
}
