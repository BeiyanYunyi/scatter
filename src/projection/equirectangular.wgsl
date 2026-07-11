const MAX_ELEVATION: f32 = PI * 0.5;
const MIN_ELEVATION: f32 = -PI / 36.0;

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let uv = (input.position.xy - uniforms.viewport_origin) / uniforms.resolution;
    let azimuth = mix(-PI, PI, uv.x);
    let elevation = mix(MAX_ELEVATION, MIN_ELEVATION, uv.y);
    let cos_elevation = cos(elevation);
    let view_direction = normalize(vec3(
        sin(azimuth) * cos_elevation,
        sin(elevation),
        cos(azimuth) * cos_elevation,
    ));
    return render_sky(view_direction);
}
