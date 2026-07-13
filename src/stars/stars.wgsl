const PI: f32 = 3.141592653589793;
const STAR_BRIGHTNESS: f32 = 2.15;
const MIN_STAR_RADIUS_PIXELS: f32 = 0.8;
const MAX_STAR_RADIUS_PIXELS: f32 = 3.0;

struct Uniforms {
    resolution: vec2<f32>,
    viewport_origin: vec2<f32>,
    sun_direction: vec4<f32>,
    atmosphere: vec4<f32>,
    camera: vec4<f32>,
    observer: vec4<f32>,
    precession: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct StarInput {
    @location(0) equatorial: vec2<f32>,
    @location(1) color_magnitude: vec4<f32>,
};

struct StarOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) point: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) brightness: f32,
};

fn horizontal_direction(right_ascension: f32, declination: f32) -> vec3<f32> {
    let zeta = uniforms.precession.x;
    let z = uniforms.precession.y;
    let theta = uniforms.precession.z;
    let shifted_ra = right_ascension + zeta;
    let cos_dec = cos(declination);
    let a = cos_dec * sin(shifted_ra);
    let b = cos(theta) * cos_dec * cos(shifted_ra) - sin(theta) * sin(declination);
    let c = sin(theta) * cos_dec * cos(shifted_ra) + cos(theta) * sin(declination);
    let current_ra = atan2(a, b) + z;
    let current_dec = asin(clamp(c, -1.0, 1.0));

    let hour_angle = uniforms.observer.x - current_ra;
    let latitude = uniforms.observer.y;
    let current_cos_dec = cos(current_dec);
    return vec3(
        -current_cos_dec * sin(hour_angle),
        sin(current_dec) * sin(latitude)
            + current_cos_dec * cos(hour_angle) * cos(latitude),
        sin(current_dec) * cos(latitude)
            - current_cos_dec * cos(hour_angle) * sin(latitude),
    );
}

@vertex
fn vs_star(@builtin(vertex_index) vertex_index: u32, input: StarInput) -> StarOutput {
    let corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    );
    let corner = corners[vertex_index];
    let direction = horizontal_direction(input.equatorial.x, input.equatorial.y);
    let projected = project_star(direction);
    let magnitude = input.color_magnitude.w;
    // Keep dim stars compact while giving first- and second-magnitude stars a visibly larger
    // core. This is intentionally nonlinear: apparent brightness is not represented by a
    // single subpixel sample on a real display.
    let bright_star_radius = 0.22 * max(1.5 - magnitude, 0.0);
    let radius_pixels = clamp(
        1.75 - 0.14 * magnitude + bright_star_radius,
        MIN_STAR_RADIUS_PIXELS,
        MAX_STAR_RADIUS_PIXELS,
    );
    let offset = corner * radius_pixels * 2.0 / uniforms.resolution;

    var output: StarOutput;
    output.point = corner;
    output.color = input.color_magnitude.rgb;
    if direction.y <= 0.0 || projected.z < 0.5 {
        output.position = vec4(2.0, 2.0, 0.0, 1.0);
        output.brightness = 0.0;
        return output;
    }

    let magnitude_fraction = clamp((magnitude + 1.5) / 8.0, 0.0, 1.0);
    let reveal_elevation = mix(-0.025, -0.17, magnitude_fraction);
    let night_visibility = 1.0
        - smoothstep(reveal_elevation, reveal_elevation + 0.04, uniforms.sun_direction.y);
    let airmass = 1.0 / max(direction.y, 0.08);
    let extinction = pow(10.0, -0.4 * 0.18 * (airmass - 1.0));
    let horizon_visibility = smoothstep(0.0, 0.025, direction.y);
    let magnitude_brightness = STAR_BRIGHTNESS * pow(10.0, -0.13 * (magnitude + 1.46));

    output.position = vec4(projected.xy + offset, 0.0, 1.0);
    output.brightness = magnitude_brightness * night_visibility
        * extinction * horizon_visibility;
    return output;
}

@fragment
fn fs_star(input: StarOutput) -> @location(0) vec4<f32> {
    let distance = length(input.point);
    let core = 1.0 - smoothstep(0.1, 0.82, distance);
    let glow = 0.22 * (1.0 - smoothstep(0.25, 1.0, distance));
    let intensity = input.brightness * (core + glow);
    return vec4(input.color * intensity, 0.0);
}
