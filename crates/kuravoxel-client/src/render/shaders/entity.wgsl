// Entidades: una caja orientada por instancia, 36 vértices generados aquí.

struct Globals {
    view_proj: mat4x4<f32>,
    cam_chunk: vec4<i32>,
    cam_local: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    sky_ambient: vec4<f32>,
    ground_ambient: vec4<f32>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
    params: vec4<f32>,
    focus: vec4<f32>,
    voxel_band: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

fn face_u(face: u32) -> vec3<f32> {
    var table = array<vec3<f32>, 6>(
        vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0),
    );
    return table[face];
}

fn face_v(face: u32) -> vec3<f32> {
    var table = array<vec3<f32>, 6>(
        vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0),
    );
    return table[face];
}

// Giro sobre Y con el mismo convenio que la cámara: yaw 0 mira a -Z y crece a la derecha.
fn rotate_y(v: vec3<f32>, yaw: f32) -> vec3<f32> {
    let s = sin(yaw);
    let c = cos(yaw);
    return vec3<f32>(v.x * c - v.z * s, v.y, v.x * s + v.z * c);
}

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
    // Misma curvatura del mundo que el terreno (ver terrain.wgsl).
    let d = rel.xz - globals.cam_local.xz;
    rel.y -= dot(d, d) * globals.fog_params.y;

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
    let sun = max(dot(normal, globals.sun_dir.xyz), 0.0);
    let ambient = mix(globals.ground_ambient.rgb, globals.sky_ambient.rgb, normal.y * 0.5 + 0.5);
    var color = in.color * (ambient + globals.sun_color.rgb * sun);

    // Misma bruma que el terreno (ver terrain.wgsl).
    let t = distance(in.rel_pos.xz, globals.focus.xz) / globals.fog_params.x;
    let haze = mix(globals.focus.w * (1.0 - exp(-2.5 * t)), 1.0, smoothstep(globals.fog_params.w, 1.0, t));
    color = mix(color, globals.fog_color.rgb, haze);
    return vec4<f32>(color, 1.0);
}
