// Tablas por cara: +X, -X, +Y, -Y, +Z, -Z. Siempre U × V = normal. Deben coincidir con
// `FACES` en mesher.rs.
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

// Giro sobre Y con el mismo convenio que la cámara: yaw 0 mira a -Z y crece a la derecha.
fn rotate_y(v: vec3<f32>, yaw: f32) -> vec3<f32> {
    let s = sin(yaw);
    let c = cos(yaw);
    return vec3<f32>(v.x * c - v.z * s, v.y, v.x * s + v.z * c);
}
