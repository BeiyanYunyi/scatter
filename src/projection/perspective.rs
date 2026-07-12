use crate::Uniforms;
use std::f32::consts::{FRAC_PI_2, TAU};
use winit::event::MouseScrollDelta;

pub const DEFAULT_FOCAL_LENGTH_MM: f32 = 50.0;
const SENSOR_HEIGHT_MM: f32 = 24.0;
const MIN_FOCAL_LENGTH_MM: f32 = 12.0;
const MAX_FOCAL_LENGTH_MM: f32 = 300.0;
const ZOOM_PER_SCROLL_LINE: f32 = 1.1;
const ROTATION_STEP_RADIANS: f32 = 1.0_f32.to_radians();

#[derive(Clone, Copy)]
pub enum CameraControl {
    PitchUp,
    PitchDown,
    YawLeft,
    YawRight,
}

pub struct Perspective {
    focal_length_mm: f32,
    manual_view: Option<[f32; 2]>,
}

impl Default for Perspective {
    fn default() -> Self {
        Self {
            focal_length_mm: DEFAULT_FOCAL_LENGTH_MM,
            manual_view: None,
        }
    }
}

impl Perspective {
    pub(super) fn configure_uniforms(&self, uniforms: &mut Uniforms) {
        let sun = uniforms.sun_direction;
        let tracked_view = [
            sun[0].atan2(sun[2]),
            camera_pitch(sun[1].clamp(-1.0, 1.0).asin()),
        ];
        let [yaw, pitch] = self.manual_view.unwrap_or(tracked_view);
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

    pub(super) fn adjust_view(&mut self, control: CameraControl, sun: [f32; 4]) {
        let view = self.manual_view.get_or_insert_with(|| {
            [
                sun[0].atan2(sun[2]),
                camera_pitch(sun[1].clamp(-1.0, 1.0).asin()),
            ]
        });
        match control {
            CameraControl::PitchUp => view[1] += ROTATION_STEP_RADIANS,
            CameraControl::PitchDown => view[1] -= ROTATION_STEP_RADIANS,
            CameraControl::YawLeft => view[0] -= ROTATION_STEP_RADIANS,
            CameraControl::YawRight => view[0] += ROTATION_STEP_RADIANS,
        }
        view[0] = view[0].rem_euclid(TAU);
        view[1] = view[1].clamp(0.0, FRAC_PI_2);
    }

    pub(super) fn reset_view(&mut self) {
        self.manual_view = None;
    }

    #[cfg(test)]
    fn is_tracking_sun(&self) -> bool {
        self.manual_view.is_none()
    }

    #[cfg(test)]
    fn manual_pitch_degrees(&self) -> Option<f32> {
        self.manual_view.map(|view| view[1].to_degrees())
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

    #[test]
    fn arrow_controls_leave_tracking_mode_and_clamp_pitch() {
        let mut camera = Perspective::default();
        let sun = [0.0, 0.5, 0.866_025_4, 0.0];

        camera.adjust_view(CameraControl::PitchUp, sun);
        for _ in 0..100 {
            camera.adjust_view(CameraControl::PitchUp, sun);
        }

        assert_eq!(camera.manual_pitch_degrees(), Some(90.0));
        assert!(!camera.is_tracking_sun());
    }

    #[test]
    fn reset_returns_camera_to_sun_tracking() {
        let mut camera = Perspective::default();
        let sun = [1.0, 0.0, 0.0, 0.0];

        camera.adjust_view(CameraControl::YawLeft, sun);
        camera.reset_view();

        assert!(camera.is_tracking_sun());
    }
}
