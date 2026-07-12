mod projection;
mod solar;

use bytemuck::{Pod, Zeroable};
use chrono::{DateTime, FixedOffset, Local, TimeDelta, Utc};
use solar::solar_position;
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use wgpu::util::DeviceExt;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowAttributes, WindowId},
};

type AppResult<T> = Result<T, Box<dyn Error>>;
const HORIZONTAL_FOV_DEGREES: f32 = 360.0;
const VERTICAL_FOV_DEGREES: f32 = 95.0;
const SKY_ASPECT_RATIO: f32 = HORIZONTAL_FOV_DEGREES / VERTICAL_FOV_DEGREES;
const HDR_SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const HDR_MAX_COMPONENT: f32 = 4.0;
const FRAME_INTERVAL: Duration = Duration::from_secs(1);
const WAKE_LAG_THRESHOLD: Duration = Duration::from_secs(2);
const WAKE_RECOVERY_DELAY: Duration = Duration::from_secs(1);
const TIME_CONTROL_DEBOUNCE: Duration = Duration::from_millis(50);

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

#[derive(Default)]
struct RenderSchedule {
    occluded: bool,
    last_active: Option<SystemTime>,
    recover_at: Option<Instant>,
    reconfigure: bool,
}

impl RenderSchedule {
    fn events_resumed(&mut self, wall_time: SystemTime, monotonic_time: Instant) {
        let inactive_for = self
            .last_active
            .and_then(|last_active| wall_time.duration_since(last_active).ok());
        self.last_active = Some(wall_time);

        if inactive_for.is_some_and(|duration| duration >= WAKE_LAG_THRESHOLD) {
            self.recover_at = Some(monotonic_time + WAKE_RECOVERY_DELAY);
            self.reconfigure = true;
        }
    }

    fn set_occluded(&mut self, occluded: bool) {
        if self.occluded && !occluded {
            self.reconfigure = true;
        }
        self.occluded = occluded;
    }

    fn can_render(&self, now: Instant) -> bool {
        !self.occluded && self.recover_at.is_none_or(|recover_at| now >= recover_at)
    }

    fn take_reconfigure(&mut self, now: Instant) -> bool {
        if !self.can_render(now) || !self.reconfigure {
            return false;
        }
        self.recover_at = None;
        self.reconfigure = false;
        true
    }
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

fn select_output_mode(
    supported_formats: &[wgpu::TextureFormat],
    default_format: wgpu::TextureFormat,
) -> OutputMode {
    if supported_formats.contains(&HDR_SURFACE_FORMAT) {
        OutputMode {
            format: HDR_SURFACE_FORMAT,
            hdr: true,
        }
    } else {
        OutputMode {
            format: default_format,
            hdr: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FrameViewport {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl FrameViewport {
    fn full(size: winit::dpi::PhysicalSize<u32>) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: size.width.max(1) as f32,
            height: size.height.max(1) as f32,
        }
    }
}

fn letterbox_viewport(size: winit::dpi::PhysicalSize<u32>) -> FrameViewport {
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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    resolution: [f32; 2],
    viewport_origin: [f32; 2],
    sun_direction: [f32; 4],
    atmosphere: [f32; 4],
    camera: [f32; 4],
}

#[derive(Clone, Copy)]
struct Location {
    latitude: f64,
    longitude: f64,
}

impl Location {
    fn from_environment(now: &DateTime<FixedOffset>) -> AppResult<Self> {
        let default_longitude = now.offset().local_minus_utc() as f64 / 240.0;
        let latitude = parse_coordinate("SKY_LATITUDE", 35.0, -90.0, 90.0)?;
        let longitude = parse_coordinate("SKY_LONGITUDE", default_longitude, -180.0, 180.0)?;
        Ok(Self {
            latitude,
            longitude,
        })
    }
}

fn parse_coordinate(name: &str, default: f64, minimum: f64, maximum: f64) -> AppResult<f64> {
    let Some(raw) = std::env::var_os(name) else {
        return Ok(default);
    };
    let value: f64 = raw
        .to_str()
        .ok_or_else(|| format!("{name} is not valid UTF-8"))?
        .parse()
        .map_err(|_| format!("{name} must be a number"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{name} must be between {minimum} and {maximum}").into());
    }
    Ok(value)
}

struct Renderer {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    location: Location,
    output_mode: OutputMode,
    projection: projection::Projection,
    time_control: TimeControl,
}

enum RenderStatus {
    Presented,
    Reconfigure,
    Skip,
    Fatal,
}

impl Renderer {
    async fn new(
        window: Arc<Window>,
        location: Location,
        projection: projection::Projection,
    ) -> AppResult<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
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
        let output_mode = select_output_mode(&capabilities.formats, config.format);
        config.format = output_mode.format;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        let shader_source = projection.shader_source();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        let now = longitude_local_time(Utc::now(), location.longitude);
        let sun = solar_position(&now, location.latitude, location.longitude);
        let initial_uniforms =
            Self::uniforms(projection.viewport(size), sun, output_mode, &projection);
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky uniforms"),
            contents: bytemuck::bytes_of(&initial_uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
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

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            location,
            output_mode,
            projection,
            time_control: TimeControl::default(),
        })
    }

    fn uniforms(
        viewport: FrameViewport,
        sun: solar::SolarPosition,
        output_mode: OutputMode,
        projection: &projection::Projection,
    ) -> Uniforms {
        let direction = sun.direction();
        let mut uniforms = Uniforms {
            resolution: [viewport.width, viewport.height],
            viewport_origin: [viewport.x, viewport.y],
            sun_direction: [direction[0], direction[1], direction[2], 0.0],
            // x: exposure, y: observer altitude km, z: HDR enabled, w: HDR component ceiling.
            atmosphere: [
                1.0,
                0.002,
                f32::from(output_mode.is_hdr()),
                HDR_MAX_COMPONENT,
            ],
            camera: [0.0; 4],
        };
        projection.configure_uniforms(&mut uniforms);
        uniforms
    }

    fn current_time(&self) -> DateTime<FixedOffset> {
        longitude_local_time(Utc::now(), self.location.longitude)
            + TimeDelta::minutes(self.time_control.offset_minutes())
    }

    fn update(&self, viewport: FrameViewport) {
        let now = self.current_time();
        let sun = solar_position(&now, self.location.latitude, self.location.longitude);
        let uniforms = Self::uniforms(viewport, sun, self.output_mode, &self.projection);
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

    fn handle_scroll(&mut self, delta: winit::event::MouseScrollDelta) {
        if self.projection.handle_scroll(delta) {
            self.window.request_redraw();
        }
    }

    fn handle_key(&mut self, key: KeyCode, now: Instant) {
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

    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    fn render(&mut self) -> RenderStatus {
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

#[derive(Default)]
struct App {
    renderer: Option<Renderer>,
    render_schedule: RenderSchedule,
}

impl ApplicationHandler for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {
        self.render_schedule
            .events_resumed(SystemTime::now(), Instant::now());
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let now = Local::now().fixed_offset();
        let location = match Location::from_environment(&now) {
            Ok(location) => location,
            Err(error) => {
                eprintln!("configuration error: {error}");
                event_loop.exit();
                return;
            }
        };
        let projection = match projection::Projection::from_environment() {
            Ok(projection) => projection,
            Err(error) => {
                eprintln!("configuration error: {error}");
                event_loop.exit();
                return;
            }
        };
        let attributes = WindowAttributes::default()
            .with_title("Scatter")
            .with_inner_size(LogicalSize::new(960, 640))
            .with_min_inner_size(LogicalSize::new(480, 320));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("failed to create window: {error}");
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(Renderer::new(window, location, projection)) {
            Ok(renderer) => self.renderer = Some(renderer),
            Err(error) => {
                eprintln!("failed to initialize wgpu: {error}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if window_id != renderer.window.id() {
            return;
        }

        match event {
            WindowEvent::CloseRequested
            | WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::Escape),
                        ..
                    },
                ..
            } => event_loop.exit(),
            WindowEvent::Resized(size) => renderer.resize(size),
            WindowEvent::MouseWheel { delta, .. } => renderer.handle_scroll(delta),
            WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        physical_key: PhysicalKey::Code(key),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => renderer.handle_key(key, Instant::now()),
            WindowEvent::Occluded(occluded) => self.render_schedule.set_occluded(occluded),
            WindowEvent::RedrawRequested if self.render_schedule.can_render(Instant::now()) => {
                match renderer.render() {
                    RenderStatus::Presented | RenderStatus::Skip => {}
                    RenderStatus::Reconfigure => {
                        renderer.resize(renderer.window.inner_size());
                    }
                    RenderStatus::Fatal => {
                        eprintln!("the rendering surface was lost or failed validation");
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if let Some(renderer) = self.renderer.as_mut() {
            if self.render_schedule.take_reconfigure(now) {
                renderer.resize(renderer.window.inner_size());
            }
            if self.render_schedule.can_render(now) {
                renderer.window.request_redraw();
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(now + FRAME_INTERVAL));
    }
}

fn main() -> AppResult<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut App::default())?;
    Ok(())
}

#[cfg(test)]
mod viewport_tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    }

    #[test]
    fn window_wider_than_angular_view_gets_centered_side_bars() {
        let viewport = letterbox_viewport(winit::dpi::PhysicalSize::new(4000, 900));

        assert_close(viewport.width, 3410.5264);
        assert_close(viewport.height, 900.0);
        assert_close(viewport.x, 294.7368);
        assert_close(viewport.y, 0.0);
    }

    #[test]
    fn window_taller_than_angular_view_gets_centered_horizontal_bars() {
        let viewport = letterbox_viewport(winit::dpi::PhysicalSize::new(950, 950));

        assert_close(viewport.width, 950.0);
        assert_close(viewport.height, 250.6944);
        assert_close(viewport.x, 0.0);
        assert_close(viewport.y, 349.6528);
    }

    #[test]
    fn three_hundred_sixty_by_ninety_five_view_uses_the_whole_surface() {
        let viewport = letterbox_viewport(winit::dpi::PhysicalSize::new(1440, 380));

        assert_close(viewport.width, 1440.0);
        assert_close(viewport.height, 380.0);
        assert_close(viewport.x, 0.0);
        assert_close(viewport.y, 0.0);
    }
}

#[cfg(test)]
mod atmosphere_shader_tests {
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
}

#[cfg(test)]
mod output_mode_tests {
    use super::*;

    #[test]
    fn prefers_float_surface_for_hdr_output() {
        let formats = [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba16Float,
        ];

        let output = select_output_mode(&formats, wgpu::TextureFormat::Bgra8UnormSrgb);

        assert_eq!(output.format, wgpu::TextureFormat::Rgba16Float);
        assert!(output.is_hdr());
    }

    #[test]
    fn keeps_default_surface_format_when_hdr_is_unavailable() {
        let formats = [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ];

        let output = select_output_mode(&formats, wgpu::TextureFormat::Bgra8UnormSrgb);

        assert_eq!(output.format, wgpu::TextureFormat::Bgra8UnormSrgb);
        assert!(!output.is_hdr());
    }
}

#[cfg(test)]
mod render_schedule_tests {
    use super::*;
    use std::time::{Instant, SystemTime};

    #[test]
    fn overdue_timer_enters_recovery_before_rendering_again() {
        let before_sleep = SystemTime::now();
        let wake_wall_time = before_sleep + Duration::from_secs(30);
        let wake_time = Instant::now();
        let mut schedule = RenderSchedule::default();

        schedule.events_resumed(before_sleep, wake_time - Duration::from_secs(30));
        schedule.events_resumed(wake_wall_time, wake_time);

        assert!(!schedule.can_render(wake_time));
        assert!(!schedule.can_render(wake_time + WAKE_RECOVERY_DELAY - Duration::from_millis(1)));
        assert!(schedule.take_reconfigure(wake_time + WAKE_RECOVERY_DELAY));
        assert!(schedule.can_render(wake_time + WAKE_RECOVERY_DELAY));
        assert!(!schedule.take_reconfigure(wake_time + WAKE_RECOVERY_DELAY));
    }

    #[test]
    fn normally_elapsed_timer_does_not_enter_recovery() {
        let first_wall_time = SystemTime::now();
        let next_wall_time = first_wall_time + Duration::from_millis(10);
        let wake_time = Instant::now();
        let mut schedule = RenderSchedule::default();

        schedule.events_resumed(first_wall_time, wake_time - Duration::from_millis(10));
        schedule.events_resumed(next_wall_time, wake_time);

        assert!(schedule.can_render(wake_time));
        assert!(!schedule.take_reconfigure(wake_time));
    }

    #[test]
    fn occluded_window_does_not_render() {
        let now = Instant::now();
        let mut schedule = RenderSchedule::default();

        schedule.set_occluded(true);
        assert!(!schedule.can_render(now));

        schedule.set_occluded(false);
        assert!(schedule.can_render(now));
        assert!(schedule.take_reconfigure(now));
    }
}

#[cfg(test)]
mod time_control_tests {
    use super::*;
    use chrono::{TimeZone, Utc};

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
