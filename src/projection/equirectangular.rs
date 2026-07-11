use crate::{FrameViewport, Uniforms};
use winit::dpi::PhysicalSize;

pub struct Equirectangular;

impl Equirectangular {
    pub(super) fn viewport(&self, size: PhysicalSize<u32>) -> FrameViewport {
        crate::letterbox_viewport(size)
    }

    pub(super) fn configure_uniforms(&self, _uniforms: &mut Uniforms) {}
}
