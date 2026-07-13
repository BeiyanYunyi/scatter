fn project_star(direction: vec3<f32>) -> vec3<f32> {
    let yaw = uniforms.camera.x;
    let pitch = uniforms.camera.y;
    let forward = vec3(sin(yaw) * cos(pitch), sin(pitch), cos(yaw) * cos(pitch));
    let right = vec3(cos(yaw), 0.0, -sin(yaw));
    let up = cross(forward, right);
    let depth = dot(direction, forward);
    if depth <= 0.0 {
        return vec3(0.0, 0.0, 0.0);
    }

    let aspect = uniforms.resolution.x / uniforms.resolution.y;
    let ndc = vec2(
        dot(direction, right) / (depth * uniforms.camera.z * aspect),
        dot(direction, up) / (depth * uniforms.camera.z),
    );
    return vec3(ndc, 1.0);
}
