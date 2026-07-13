const STAR_MAX_ELEVATION: f32 = PI * 0.5;
const STAR_MIN_ELEVATION: f32 = -PI / 36.0;

fn project_star(direction: vec3<f32>) -> vec3<f32> {
    let azimuth = atan2(direction.x, direction.z);
    let elevation = asin(clamp(direction.y, -1.0, 1.0));
    let vertical_uv = (STAR_MAX_ELEVATION - elevation)
        / (STAR_MAX_ELEVATION - STAR_MIN_ELEVATION);
    return vec3(azimuth / PI, 1.0 - 2.0 * vertical_uv, 1.0);
}
