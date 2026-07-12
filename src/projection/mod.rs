mod equirectangular;
mod perspective;

use crate::{FrameViewport, Uniforms};
use std::error::Error;
use winit::{dpi::PhysicalSize, event::MouseScrollDelta};

pub use equirectangular::Equirectangular;
pub use perspective::{CameraControl, Perspective};

pub const ENVIRONMENT_VARIABLE: &str = "SKY_PROJECTION";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionKind {
    Perspective,
    Equirectangular,
}

impl ProjectionKind {
    fn parse(value: &str) -> Result<Self, Box<dyn Error>> {
        match value.to_ascii_lowercase().as_str() {
            "perspective" => Ok(Self::Perspective),
            "equirectangular" => Ok(Self::Equirectangular),
            _ => {
                Err(format!("{ENVIRONMENT_VARIABLE} must be perspective or equirectangular").into())
            }
        }
    }

    pub fn from_environment() -> Result<Self, Box<dyn Error>> {
        let Some(raw) = std::env::var_os(ENVIRONMENT_VARIABLE) else {
            return Ok(Self::Perspective);
        };
        Self::parse(
            raw.to_str()
                .ok_or_else(|| format!("{ENVIRONMENT_VARIABLE} is not valid UTF-8"))?,
        )
    }
}

pub enum Projection {
    Perspective(Perspective),
    Equirectangular(Equirectangular),
}

impl Projection {
    pub fn from_environment() -> Result<Self, Box<dyn Error>> {
        Ok(match ProjectionKind::from_environment()? {
            ProjectionKind::Perspective => Self::Perspective(Perspective::default()),
            ProjectionKind::Equirectangular => Self::Equirectangular(Equirectangular),
        })
    }

    pub fn shader_source(&self) -> String {
        let projection = match self {
            Self::Perspective(_) => include_str!("perspective.wgsl"),
            Self::Equirectangular(_) => include_str!("equirectangular.wgsl"),
        };
        format!("{}\n{projection}", include_str!("../sky.wgsl"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_names_are_case_insensitive() {
        assert_eq!(
            ProjectionKind::parse("Perspective").unwrap(),
            ProjectionKind::Perspective
        );
        assert_eq!(
            ProjectionKind::parse("EQUIRECTANGULAR").unwrap(),
            ProjectionKind::Equirectangular
        );
    }

    #[test]
    fn unknown_projection_is_rejected() {
        assert!(ProjectionKind::parse("fisheye").is_err());
    }
}
