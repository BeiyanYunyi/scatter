mod equirectangular;
mod perspective;

use crate::{FrameViewport, Uniforms};
use serde::Deserialize;
use winit::{dpi::PhysicalSize, event::MouseScrollDelta};

pub use equirectangular::Equirectangular;
pub use perspective::{CameraControl, Perspective};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectionKind {
    #[default]
    Perspective,
    Equirectangular,
}

pub enum Projection {
    Perspective(Perspective),
    Equirectangular(Equirectangular),
}

impl Projection {
    pub fn new(kind: ProjectionKind) -> Self {
        match kind {
            ProjectionKind::Perspective => Self::Perspective(Perspective::default()),
            ProjectionKind::Equirectangular => Self::Equirectangular(Equirectangular),
        }
    }

    pub fn shader_source(&self) -> String {
        let projection = match self {
            Self::Perspective(_) => include_str!("perspective.wgsl"),
            Self::Equirectangular(_) => include_str!("equirectangular.wgsl"),
        };
        format!("{}\n{projection}", include_str!("../sky.wgsl"))
    }

    pub fn star_shader_source(&self) -> String {
        let projection = match self {
            Self::Perspective(_) => include_str!("perspective_stars.wgsl"),
            Self::Equirectangular(_) => include_str!("equirectangular_stars.wgsl"),
        };
        format!("{}\n{projection}", include_str!("../stars/stars.wgsl"))
    }

    pub fn viewport(&self, size: PhysicalSize<u32>) -> FrameViewport {
        match self {
            Self::Perspective(_) => FrameViewport::full(size),
            Self::Equirectangular(projection) => projection.viewport(size),
        }
    }

    pub fn configure_uniforms(&self, uniforms: &mut Uniforms) {
        match self {
            Self::Perspective(projection) => projection.configure_uniforms(uniforms),
            Self::Equirectangular(projection) => projection.configure_uniforms(uniforms),
        }
    }

    pub fn handle_scroll(&mut self, delta: MouseScrollDelta) -> bool {
        match self {
            Self::Perspective(projection) => projection.handle_scroll(delta),
            Self::Equirectangular(_) => false,
        }
    }

    pub fn adjust_view(&mut self, control: CameraControl, sun: [f32; 4]) -> bool {
        match self {
            Self::Perspective(projection) => {
                projection.adjust_view(control, sun);
                true
            }
            Self::Equirectangular(_) => false,
        }
    }

    pub fn reset_view(&mut self) -> bool {
        match self {
            Self::Perspective(projection) => {
                projection.reset_view();
                true
            }
            Self::Equirectangular(_) => false,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Perspective(projection) => {
                format!("perspective {:.0} mm", projection.focal_length_mm())
            }
            Self::Equirectangular(_) => "equirectangular 360°".to_owned(),
        }
    }
}
