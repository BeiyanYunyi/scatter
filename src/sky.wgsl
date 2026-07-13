const PI: f32 = 3.141592653589793;
const ATMOSPHERE_HEIGHT: f32 = 100.0;
const VIEW_DISTANCE: f32 = 220.0;
const PRIMARY_STEPS: i32 = 20;
const LIGHT_STEPS: i32 = 6;

const BETA_R: vec3<f32> = vec3(0.0058, 0.0135, 0.0331);
const BETA_M_SCATTER: vec3<f32> = vec3(0.0030);
const BETA_M_EXT: vec3<f32> = vec3(0.0044);
const BETA_OZONE_ABS: vec3<f32> = vec3(0.00065, 0.00188, 0.00008);
const SUN_INTENSITY: f32 = 22.0;
const MIE_G: f32 = 0.76;
struct Uniforms {
    resolution: vec2<f32>,
    viewport_origin: vec2<f32>,
    sun_direction: vec4<f32>,
    // x: exposure, y: observer altitude km, z: HDR enabled, w: HDR component ceiling
    atmosphere: vec4<f32>,
    // x: camera yaw, y: camera pitch, z: tan(vertical FOV / 2), w: reserved
    camera: vec4<f32>,
    // x: local sidereal time radians, y: observer latitude radians
    observer: vec4<f32>,
    // J2000-to-date precession angles: zeta, z and theta
    precession: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0),
        vec2(3.0, -1.0),
        vec2(-1.0, 3.0),
    );
    var output: VertexOutput;
    output.position = vec4(positions[index], 0.0, 1.0);
    return output;
}

fn rayleigh_density(height: f32) -> f32 {
    return exp(-max(height, 0.0) / 8.0);
}

fn mie_density(height: f32) -> f32 {
    return exp(-max(height, 0.0) / 1.2);
}

fn ozone_density(height: f32) -> f32 {
    let normalized = (height - 25.0) / 15.0;
    return max(0.0, 1.0 - abs(normalized));
}

fn rayleigh_phase(mu: f32) -> f32 {
    return 3.0 / (16.0 * PI) * (1.0 + mu * mu);
}

fn mie_phase(mu: f32) -> f32 {
    let gg = MIE_G * MIE_G;
    let numerator = 3.0 * (1.0 - gg) * (1.0 + mu * mu);
    let denominator = 8.0 * PI * (2.0 + gg)
        * pow(max(1.0 + gg - 2.0 * MIE_G * mu, 0.0001), 1.5);
    return numerator / denominator;
}

fn light_optical_depth(start_height: f32, sun_y: f32) -> vec3<f32> {
    // The small offset keeps the flat-atmosphere approximation stable around sunset.
    let effective_sun_y = max(sun_y + 0.15, 0.04);
    let max_distance = max((ATMOSPHERE_HEIGHT - start_height) / effective_sun_y, 0.0);
    let step_size = min(max_distance, 600.0) / f32(LIGHT_STEPS);
    var optical_depth = vec3(0.0);

    for (var i = 0; i < LIGHT_STEPS; i += 1) {
        let height = start_height + (f32(i) + 0.5) * step_size * effective_sun_y;
        if (height >= 0.0 && height <= ATMOSPHERE_HEIGHT) {
            optical_depth += vec3(
                rayleigh_density(height),
                mie_density(height),
                ozone_density(height),
            ) * step_size;
        }
    }

    return optical_depth;
}

fn aces_film(color: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((color * (a * color + b)) / (color * (c * color + d) + e), vec3(0.0), vec3(1.0));
}

fn hdr_tone_map(color: vec3<f32>, component_ceiling: f32) -> vec3<f32> {
    // scRGB uses linear sRGB primaries with 1.0 as SDR reference white. Preserve the
    // SDR body of the image, then place scene highlights in the display's > 1.0 range.
    let base = aces_film(color);
    let luminance = dot(color, vec3(0.2126, 0.7152, 0.0722));
    let highlight_range = max(component_ceiling - 1.0, 0.0);
    let highlight = highlight_range
        * (1.0 - exp(-max(luminance - 1.0, 0.0) / max(highlight_range, 0.0001)));
    let highlight_color = color / max(max(color.r, max(color.g, color.b)), 0.0001);
    return min(base + highlight_color * highlight, vec3(component_ceiling));
}

fn sky_radiance(view_direction: vec3<f32>) -> vec3<f32> {
    let sun_direction = normalize(uniforms.sun_direction.xyz);
    let step_size = VIEW_DISTANCE / f32(PRIMARY_STEPS);
    var view_od = vec3(0.0);
    var sum_r = vec3(0.0);
    var sum_m = vec3(0.0);

    for (var i = 0; i < PRIMARY_STEPS; i += 1) {
        let distance = (f32(i) + 0.5) * step_size;
        let height = uniforms.atmosphere.y + distance * view_direction.y;
        if (height < 0.0 || height > ATMOSPHERE_HEIGHT) {
            break;
        }

        let density = vec3(
            rayleigh_density(height),
            mie_density(height),
            ozone_density(height),
        );
        view_od += density * step_size;
        let sun_od = light_optical_depth(height, sun_direction.y);
        let optical_depth = view_od + sun_od;
        let tau = BETA_R * optical_depth.x
            + BETA_M_EXT * optical_depth.y
            + BETA_OZONE_ABS * optical_depth.z;
        let transmittance = exp(-tau);
        sum_r += density.x * transmittance * step_size;
        sum_m += density.y * transmittance * step_size;
    }

    let mu = dot(view_direction, sun_direction);
    var color = SUN_INTENSITY * (
        rayleigh_phase(mu) * BETA_R * sum_r
        + mie_phase(mu) * BETA_M_SCATTER * sum_m
    );
    // The flat-atmosphere approximation cannot model Earth's moving shadow. Fade its
    // scattering continuously while the Sun descends from 3° to -2°.
    let twilight_visibility = smoothstep(-0.0349, 0.05234, sun_direction.y);
    color *= twilight_visibility;

    // A physical half-degree solar disc, softened by a small bloom halo.
    let angular_distance = acos(clamp(mu, -1.0, 1.0));
    let disc = 1.0 - smoothstep(0.0042, 0.0052, angular_distance);
    let halo = exp(-angular_distance * angular_distance * 1800.0);
    let daylight = smoothstep(-0.105, -0.01, sun_direction.y);
    color += vec3(1.0, 0.73, 0.42) * (disc * 16.0 + halo * 0.7) * daylight;

    return color;
}

fn render_sky(view_direction: vec3<f32>) -> vec4<f32> {
    let elevation = asin(clamp(view_direction.y, -1.0, 1.0));
    var color: vec3<f32>;
    if (elevation >= 0.0) {
        color = sky_radiance(view_direction);
        let night = 1.0 - smoothstep(-0.20, -0.03, uniforms.sun_direction.y);
        color += vec3(0.001, 0.002, 0.008) * night;
    } else {
        let horizon_glow = exp(-abs(elevation) * 22.0)
            * smoothstep(-0.18, 0.08, uniforms.sun_direction.y);
        color = vec3(0.002, 0.003, 0.004) + vec3(0.11, 0.045, 0.012) * horizon_glow;
    }

    color *= uniforms.atmosphere.x;
    if (uniforms.atmosphere.z > 0.5) {
        color = hdr_tone_map(color, uniforms.atmosphere.w);
    } else {
        color = aces_film(color);
    }
    return vec4(color, 1.0);
}
