use crate::{
    AppResult, astronomy, config,
    projection::{self, FrameViewport},
    solar::{self, solar_position},
    stars,
};
use bytemuck::{Pod, Zeroable};
use chrono::{DateTime, FixedOffset, Local, TimeDelta, Utc};
use std::{sync::Arc, time::Instant};
use wgpu::util::DeviceExt;
use winit::{keyboard::KeyCode, window::Window};

const HDR_SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const HDR_MAX_COMPONENT: f32 = 4.0;
const TIME_CONTROL_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(50);

#[derive(Default)]
struct TimeControl {
    offset_minutes: i64,
    last_adjustment: Option<Instant>,
}

impl TimeControl {
    fn adjust_minutes(&mut self, minutes: i64, now: Instant) -> bool {
        if self
            .last_adjustment
            .is_some_and(|last| now.duration_since(last) < TIME_CONTROL_DEBOUNCE)
        {
            return false;
        }
        self.offset_minutes += minutes;
        self.last_adjustment = Some(now);
        true
    }

    fn reset(&mut self) {
        self.offset_minutes = 0;
        self.last_adjustment = None;
    }

    fn offset_minutes(&self) -> i64 {
        self.offset_minutes
    }
}

fn longitude_local_time(utc: DateTime<Utc>, longitude: f64) -> DateTime<FixedOffset> {
    let offset_seconds = (longitude * 240.0).round() as i32;
    utc.with_timezone(&FixedOffset::east_opt(offset_seconds).expect("longitude is in range"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OutputMode {
    format: wgpu::TextureFormat,
    hdr: bool,
}

impl OutputMode {
    fn is_hdr(self) -> bool {
        self.hdr
    }

    fn label(self) -> &'static str {
        if self.is_hdr() { "HDR" } else { "SDR" }
    }
}

fn select_output_mode_with_preference(
    supported_formats: &[wgpu::TextureFormat],
    default_format: wgpu::TextureFormat,
    force_sdr: bool,
) -> Option<OutputMode> {
    if !force_sdr && supported_formats.contains(&HDR_SURFACE_FORMAT) {
        return Some(OutputMode {
            format: HDR_SURFACE_FORMAT,
            hdr: true,
        });
    }

    let format = (default_format != HDR_SURFACE_FORMAT)
        .then_some(default_format)
        .or_else(|| {
            [
                wgpu::TextureFormat::Bgra8UnormSrgb,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                wgpu::TextureFormat::Bgra8Unorm,
                wgpu::TextureFormat::Rgba8Unorm,
            ]
            .into_iter()
            .find(|format| supported_formats.contains(format))
        })
        .or_else(|| {
            supported_formats
                .iter()
                .copied()
                .find(|format| *format != HDR_SURFACE_FORMAT)
        })?;

    Some(OutputMode { format, hdr: false })
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    resolution: [f32; 2],
    viewport_origin: [f32; 2],
    sun_direction: [f32; 4],
    atmosphere: [f32; 4],
    camera: [f32; 4],
    observer: [f32; 4],
    precession: [f32; 4],
}

#[derive(Clone, Copy)]
struct Location {
    latitude: f64,
    longitude: f64,
}

impl Location {
    fn from_config(config: config::LocationConfig, now: &DateTime<FixedOffset>) -> Self {
        let default_longitude = now.offset().local_minus_utc() as f64 / 240.0;
        Self {
            latitude: config.latitude,
            longitude: config.longitude.unwrap_or(default_longitude),
        }
    }
}

pub(crate) struct Renderer {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    star_renderer: stars::StarRenderer,
    location: Location,
    output_mode: OutputMode,
    projection: projection::Projection,
    time_control: TimeControl,
    #[cfg(target_os = "macos")]
    _metal_layer: crate::macos::MetalLayer,
}

pub(crate) enum RenderStatus {
    Presented,
    Reconfigure,
    Skip,
    Fatal,
}

impl Renderer {
    pub(crate) async fn new(
        window: Arc<Window>,
        location_config: config::LocationConfig,
        projection_kind: projection::ProjectionKind,
        force_sdr: bool,
    ) -> AppResult<Self> {
        let location = Location::from_config(location_config, &Local::now().fixed_offset());
        let projection = projection::Projection::new(projection_kind);
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        #[cfg(target_os = "macos")]
        let (surface, metal_layer) = crate::macos::create_surface(&instance, &window)?;
        #[cfg(not(target_os = "macos"))]
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("scatter device"),
                ..Default::default()
            })
            .await?;
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("the selected GPU cannot present to this window")?;
        let capabilities = surface.get_capabilities(&adapter);
        let output_mode =
            select_output_mode_with_preference(&capabilities.formats, config.format, force_sdr)
                .ok_or("the selected GPU surface does not support an SDR format")?;
        config.format = output_mode.format;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        #[cfg(target_os = "macos")]
        crate::macos::configure_output(&metal_layer, output_mode.is_hdr())?;

        let shader_source = projection.shader_source();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        let now = longitude_local_time(Utc::now(), location.longitude);
        let sun = solar_position(&now, location.latitude, location.longitude);
        let initial_uniforms = Self::uniforms(
            projection.viewport(size),
            sun,
            output_mode,
            &projection,
            &now,
            location,
        );
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky uniforms"),
            contents: bytemuck::bytes_of(&initial_uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("atmosphere pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let star_renderer =
            stars::StarRenderer::new(&device, config.format, &bind_group_layout, &projection)?;

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            star_renderer,
            location,
            output_mode,
            projection,
            time_control: TimeControl::default(),
            #[cfg(target_os = "macos")]
            _metal_layer: metal_layer,
        })
    }

    fn uniforms(
        viewport: FrameViewport,
        sun: solar::SolarPosition,
        output_mode: OutputMode,
        projection: &projection::Projection,
        time: &DateTime<FixedOffset>,
        location: Location,
    ) -> Uniforms {
        let direction = sun.direction();
        let sun_direction = [direction[0], direction[1], direction[2], 0.0];
        Uniforms {
            resolution: [viewport.width, viewport.height],
            viewport_origin: [viewport.x, viewport.y],
            sun_direction,
            // x: exposure, y: observer altitude km, z: HDR enabled, w: HDR component ceiling.
            atmosphere: [
                1.0,
                0.002,
                f32::from(output_mode.is_hdr()),
                HDR_MAX_COMPONENT,
            ],
            camera: projection.camera_uniform(sun_direction),
            observer: [
                astronomy::local_sidereal_time(time, location.longitude),
                (location.latitude as f32).to_radians(),
                0.0,
                0.0,
            ],
            precession: {
                let [zeta, z, theta] = astronomy::precession_angles(time);
                [zeta, z, theta, 0.0]
            },
        }
    }

    fn current_time(&self) -> DateTime<FixedOffset> {
        longitude_local_time(Utc::now(), self.location.longitude)
            + TimeDelta::minutes(self.time_control.offset_minutes())
    }

    fn update(&self, viewport: FrameViewport) {
        let now = self.current_time();
        let sun = solar_position(&now, self.location.latitude, self.location.longitude);
        let uniforms = Self::uniforms(
            viewport,
            sun,
            self.output_mode,
            &self.projection,
            &now,
            self.location,
        );
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        self.window.set_title(&format!(
            "Scatter [{} | {}] — {} LMT  |  sun {:.1}° high, azimuth {:.1}°",
            self.output_mode.label(),
            self.projection.label(),
            now.format("%Y-%m-%d %H:%M:%S"),
            sun.elevation_deg,
            sun.azimuth_deg,
        ));
    }

    pub(crate) fn window(&self) -> &Window {
        &self.window
    }

    pub(crate) fn handle_scroll(&mut self, delta: winit::event::MouseScrollDelta) {
        if self.projection.handle_scroll(delta) {
            self.window.request_redraw();
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyCode, now: Instant) {
        let changed = match key {
            KeyCode::ArrowUp | KeyCode::ArrowDown | KeyCode::ArrowLeft | KeyCode::ArrowRight => {
                let sun = solar_position(
                    &self.current_time(),
                    self.location.latitude,
                    self.location.longitude,
                );
                let control = match key {
                    KeyCode::ArrowUp => projection::CameraControl::PitchUp,
                    KeyCode::ArrowDown => projection::CameraControl::PitchDown,
                    KeyCode::ArrowLeft => projection::CameraControl::YawLeft,
                    KeyCode::ArrowRight => projection::CameraControl::YawRight,
                    _ => unreachable!(),
                };
                let direction = sun.direction();
                self.projection
                    .adjust_view(control, [direction[0], direction[1], direction[2], 0.0])
            }
            KeyCode::KeyR => self.projection.reset_view(),
            KeyCode::Comma => self.time_control.adjust_minutes(-1, now),
            KeyCode::Period => self.time_control.adjust_minutes(1, now),
            KeyCode::KeyT => {
                self.time_control.reset();
                true
            }
            _ => false,
        };
        if changed {
            self.window.request_redraw();
        }
    }

    pub(crate) fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub(crate) fn render(&mut self) -> RenderStatus {
        let viewport = self.projection.viewport(self.window.inner_size());
        self.update(viewport);
        let (frame, reconfigure_after_present) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => (frame, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return RenderStatus::Skip;
            }
            wgpu::CurrentSurfaceTexture::Outdated => return RenderStatus::Reconfigure,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Validation => {
                return RenderStatus::Fatal;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sky command encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sky render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(
                viewport.x,
                viewport.y,
                viewport.width,
                viewport.height,
                0.0,
                1.0,
            );
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.draw(0..3, 0..1);
            self.star_renderer.draw(&mut pass, &self.uniform_bind_group);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        if reconfigure_after_present {
            RenderStatus::Reconfigure
        } else {
            RenderStatus::Presented
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::time::Duration;

    #[test]
    fn twilight_samples_use_the_stabilized_sun_elevation() {
        let shader = include_str!("sky.wgsl");

        assert!(
            shader.contains(
                "let height = start_height + (f32(i) + 0.5) * step_size * effective_sun_y;"
            ),
            "twilight light samples must not descend below the horizon in discrete steps"
        );
    }

    #[test]
    fn twilight_scattering_fades_without_a_hard_cutoff() {
        let shader = include_str!("sky.wgsl");

        assert!(
            shader.contains(
                "let twilight_visibility = smoothstep(-0.0349, 0.05234, sun_direction.y);"
            ) && shader.contains("color *= twilight_visibility;"),
            "atmospheric scattering must fade continuously from 3 to -2 degrees"
        );
    }

    #[test]
    fn prefers_float_surface_for_hdr_output() {
        let formats = [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba16Float,
        ];
        let output = select_output_mode_with_preference(
            &formats,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            false,
        )
        .unwrap();

        assert_eq!(output.format, wgpu::TextureFormat::Rgba16Float);
        assert!(output.is_hdr());
    }

    #[test]
    fn keeps_default_surface_format_when_hdr_is_unavailable() {
        let formats = [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ];
        let output = select_output_mode_with_preference(
            &formats,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            false,
        )
        .unwrap();

        assert_eq!(output.format, wgpu::TextureFormat::Bgra8UnormSrgb);
        assert!(!output.is_hdr());
    }

    #[test]
    fn forced_sdr_uses_an_eight_bit_surface_when_hdr_is_available() {
        let formats = [
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ];
        let output =
            select_output_mode_with_preference(&formats, wgpu::TextureFormat::Rgba16Float, true)
                .unwrap();

        assert_eq!(output.format, wgpu::TextureFormat::Bgra8UnormSrgb);
        assert!(!output.is_hdr());
    }

    #[test]
    fn forced_sdr_rejects_an_hdr_only_surface() {
        assert!(
            select_output_mode_with_preference(
                &[wgpu::TextureFormat::Rgba16Float],
                wgpu::TextureFormat::Rgba16Float,
                true,
            )
            .is_none()
        );
    }

    #[test]
    fn target_longitude_time_uses_four_minutes_per_degree() {
        let utc = Utc.with_ymd_and_hms(2026, 7, 12, 12, 0, 0).unwrap();
        let local = longitude_local_time(utc, 121.5);

        assert_eq!(
            local.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-07-12 20:06:00"
        );
    }

    #[test]
    fn time_adjustments_are_debounced_and_resettable() {
        let start = Instant::now();
        let mut control = TimeControl::default();

        assert!(control.adjust_minutes(1, start));
        assert!(!control.adjust_minutes(1, start + Duration::from_millis(49)));
        assert!(control.adjust_minutes(-1, start + Duration::from_millis(50)));
        assert_eq!(control.offset_minutes(), 0);

        control.adjust_minutes(1, start + Duration::from_millis(100));
        control.reset();
        assert_eq!(control.offset_minutes(), 0);
    }
}
