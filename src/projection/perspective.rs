use crate::Uniforms;
use winit::event::MouseScrollDelta;

pub const DEFAULT_FOCAL_LENGTH_MM: f32 = 50.0;
const SENSOR_HEIGHT_MM: f32 = 24.0;
const MIN_FOCAL_LENGTH_MM: f32 = 12.0;
const MAX_FOCAL_LENGTH_MM: f32 = 300.0;
const ZOOM_PER_SCROLL_LINE: f32 = 1.1;

pub struct Perspective {
    focal_length_mm: f32,
}

impl Default for Perspective {
    fn default() -> Self {
        Self {
            focal_length_mm: DEFAULT_FOCAL_LENGTH_MM,
        }
    }
}

impl Perspective {
    pub(super) fn configure_uniforms(&self, uniforms: &mut Uniforms) {
        let sun = uniforms.sun_direction;
        let yaw = sun[0].atan2(sun[2]);
        let pitch = camera_pitch(sun[1].clamp(-1.0, 1.0).asin());
        let tan_half_vertical_fov = SENSOR_HEIGHT_MM / (2.0 * self.focal_length_mm);
        uniforms.camera = [yaw, pitch, tan_half_vertical_fov, 0.0];
    }

    pub(super) fn handle_scroll(&mut self, delta: MouseScrollDelta) -> bool {
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, lines) => lines,
            MouseScrollDelta::PixelDelta(position) => position.y as f32 / 100.0,
        };
        if lines == 0.0 {
            return false;
        }
        self.focal_length_mm = (self.focal_length_mm * ZOOM_PER_SCROLL_LINE.powf(lines))
            .clamp(MIN_FOCAL_LENGTH_MM, MAX_FOCAL_LENGTH_MM);
        true
    }

    pub(super) fn focal_length_mm(&self) -> f32 {
        self.focal_length_mm
    }
}

fn camera_pitch(sun_elevation_radians: f32) -> f32 {
    sun_elevation_radians.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_a_normal_human_eye_lens() {
        assert_eq!(Perspective::default().focal_length_mm(), 50.0);
    }

    #[test]
    fn camera_pitch_stops_at_the_horizon_when_sun_is_below_it() {
        assert_eq!(camera_pitch(-0.4), 0.0);
        assert_eq!(camera_pitch(0.4), 0.4);
    }

    #[test]
    fn scrolling_up_zooms_in_and_scrolling_down_zooms_out() {
        let mut camera = Perspective::default();
        camera.handle_scroll(MouseScrollDelta::LineDelta(0.0, 1.0));
        let zoomed_in = camera.focal_length_mm();
        camera.handle_scroll(MouseScrollDelta::LineDelta(0.0, -2.0));

        assert!(zoomed_in > DEFAULT_FOCAL_LENGTH_MM);
        assert!(camera.focal_length_mm() < DEFAULT_FOCAL_LENGTH_MM);
    }
}
