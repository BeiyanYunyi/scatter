use super::FrameViewport;
use winit::dpi::PhysicalSize;

const HORIZONTAL_FOV_DEGREES: f32 = 360.0;
const VERTICAL_FOV_DEGREES: f32 = 95.0;
const SKY_ASPECT_RATIO: f32 = HORIZONTAL_FOV_DEGREES / VERTICAL_FOV_DEGREES;

pub struct Equirectangular;

impl Equirectangular {
    pub(super) fn viewport(&self, size: PhysicalSize<u32>) -> FrameViewport {
        letterbox_viewport(size)
    }

    pub(super) fn camera_uniform(&self) -> [f32; 4] {
        [0.0; 4]
    }
}

fn letterbox_viewport(size: PhysicalSize<u32>) -> FrameViewport {
    let surface_width = size.width.max(1) as f32;
    let surface_height = size.height.max(1) as f32;
    let surface_aspect = surface_width / surface_height;

    if surface_aspect > SKY_ASPECT_RATIO {
        let width = surface_height * SKY_ASPECT_RATIO;
        FrameViewport {
            x: (surface_width - width) * 0.5,
            y: 0.0,
            width,
            height: surface_height,
        }
    } else {
        let height = surface_width / SKY_ASPECT_RATIO;
        FrameViewport {
            x: 0.0,
            y: (surface_height - height) * 0.5,
            width: surface_width,
            height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    }

    #[test]
    fn window_wider_than_angular_view_gets_centered_side_bars() {
        let viewport = letterbox_viewport(PhysicalSize::new(4000, 900));

        assert_close(viewport.width, 3410.5264);
        assert_close(viewport.height, 900.0);
        assert_close(viewport.x, 294.7368);
        assert_close(viewport.y, 0.0);
    }

    #[test]
    fn window_taller_than_angular_view_gets_centered_horizontal_bars() {
        let viewport = letterbox_viewport(PhysicalSize::new(950, 950));

        assert_close(viewport.width, 950.0);
        assert_close(viewport.height, 250.6944);
        assert_close(viewport.x, 0.0);
        assert_close(viewport.y, 349.6528);
    }

    #[test]
    fn three_hundred_sixty_by_ninety_five_view_uses_the_whole_surface() {
        let viewport = letterbox_viewport(PhysicalSize::new(1440, 380));

        assert_close(viewport.width, 1440.0);
        assert_close(viewport.height, 380.0);
        assert_close(viewport.x, 0.0);
        assert_close(viewport.y, 0.0);
    }
}
