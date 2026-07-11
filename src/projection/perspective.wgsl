@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let uv = (input.position.xy - uniforms.viewport_origin) / uniforms.resolution;
    let screen = vec2(
        (uv.x * 2.0 - 1.0) * uniforms.resolution.x / uniforms.resolution.y,
        1.0 - uv.y * 2.0,
    );

    let yaw = uniforms.camera.x;
    let pitch = uniforms.camera.y;
    let forward = vec3(sin(yaw) * cos(pitch), sin(pitch), cos(yaw) * cos(pitch));
    let right = vec3(cos(yaw), 0.0, -sin(yaw));
    let up = cross(forward, right);
    let view_direction = normalize(
        forward
            + right * screen.x * uniforms.camera.z
            + up * screen.y * uniforms.camera.z,
    );
    return render_sky(view_direction);
}
