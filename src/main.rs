mod astronomy;
mod config;
#[cfg(target_os = "macos")]
mod macos;
mod projection;
mod solar;
mod stars;

use bytemuck::{Pod, Zeroable};
use chrono::{DateTime, FixedOffset, Local, TimeDelta, Utc};
use solar::solar_position;
use std::{
    error::Error,
    ffi::OsStr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use wgpu::util::DeviceExt;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WindowMode {
    #[default]
    Normal,
    Wallpaper,
}

impl WindowMode {
    fn is_wallpaper(self) -> bool {
        self == Self::Wallpaper
    }
}

#[derive(Debug, PartialEq, Eq)]
struct StartupOptions {
    window_mode: WindowMode,
    config_path: PathBuf,
}

fn parse_startup_options<I, S>(arguments: I) -> Result<Option<StartupOptions>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut mode = WindowMode::Normal;
    let mut config_path = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_ref().to_str() {
            Some("--wallpaper") => mode = WindowMode::Wallpaper,
            Some("--help" | "-h") => return Ok(None),
            Some("--config") => {
                let path = arguments
                    .next()
                    .ok_or_else(|| "--config requires a path argument".to_owned())?;
                config_path = Some(PathBuf::from(path.as_ref()));
            }
            Some(argument) if argument.starts_with("--config=") => {
                config_path = Some(PathBuf::from(&argument["--config=".len()..]));
            }
            Some(argument) => return Err(format!("unknown argument: {argument}")),
            None => return Err("arguments must be valid UTF-8".into()),
        }
    }
    Ok(Some(StartupOptions {
        window_mode: mode,
        config_path: config_path.unwrap_or_else(|| PathBuf::from("config.toml")),
    }))
}

fn print_usage() {
    println!(
        "Usage: scatter [--wallpaper] [--config PATH]\n\n  --wallpaper    Render behind desktop icons on macOS\n  --config PATH  Load and hot-reload this TOML file (default: config.toml)"
    );
}

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

struct Renderer {
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
    _metal_layer: macos::MetalLayer,
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
        force_sdr: bool,
    ) -> AppResult<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        #[cfg(target_os = "macos")]
        let (surface, metal_layer) = macos::create_surface(&instance, &window)?;
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
        macos::configure_output(&metal_layer, output_mode)?;

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

struct App {
    renderers: Vec<ManagedRenderer>,
    window_mode: WindowMode,
    wallpaper_layout: Vec<DisplayGeometry>,
    runtime_config: config::RuntimeConfig,
}

struct ManagedRenderer {
    renderer: Renderer,
    render_schedule: RenderSchedule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DisplayGeometry {
    name: Option<String>,
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
}

impl App {
    fn new(window_mode: WindowMode, runtime_config: config::RuntimeConfig) -> Self {
        Self {
            renderers: Vec::new(),
            window_mode,
            wallpaper_layout: Vec::new(),
            runtime_config,
        }
    }

    fn available_displays(event_loop: &ActiveEventLoop) -> Vec<DisplayGeometry> {
        let mut displays = event_loop
            .available_monitors()
            .map(|monitor| DisplayGeometry {
                name: monitor.name(),
                position: monitor.position(),
                size: monitor.size(),
            })
            .collect::<Vec<_>>();
        Self::sort_displays(&mut displays);
        displays
    }

    fn sort_displays(displays: &mut [DisplayGeometry]) {
        displays.sort_by(|left, right| {
            (
                left.position.x,
                left.position.y,
                left.size.width,
                left.size.height,
                &left.name,
            )
                .cmp(&(
                    right.position.x,
                    right.position.y,
                    right.size.width,
                    right.size.height,
                    &right.name,
                ))
        });
    }

    fn create_window_renderer(
        &self,
        event_loop: &ActiveEventLoop,
        attributes: WindowAttributes,
        location: Location,
        projection_kind: projection::ProjectionKind,
        force_sdr: bool,
    ) -> AppResult<ManagedRenderer> {
        let window = Arc::new(event_loop.create_window(attributes)?);
        if self.window_mode.is_wallpaper() {
            #[cfg(target_os = "macos")]
            macos::configure_wallpaper_window(&window)?;
            #[cfg(not(target_os = "macos"))]
            return Err("wallpaper mode is only supported on macOS".into());
        }

        let renderer = pollster::block_on(Renderer::new(
            window,
            location,
            projection::Projection::new(projection_kind),
            force_sdr,
        ))?;
        Ok(ManagedRenderer {
            renderer,
            render_schedule: RenderSchedule::default(),
        })
    }

    fn build_renderers(
        &self,
        event_loop: &ActiveEventLoop,
        app_config: &config::AppConfig,
    ) -> AppResult<(Vec<ManagedRenderer>, Vec<DisplayGeometry>)> {
        let now = Local::now().fixed_offset();
        let location = Location::from_config(app_config.location, &now);
        let projection_kind = app_config.rendering.projection;
        let force_sdr = app_config.rendering.force_sdr;
        if force_sdr {
            eprintln!("rendering.force_sdr is enabled; forcing SDR output");
        }

        let (attributes, layout) = if self.window_mode.is_wallpaper() {
            let displays = Self::available_displays(event_loop);
            if displays.is_empty() {
                return Err("wallpaper mode requires an attached display".into());
            }
            let attributes = displays
                .iter()
                .enumerate()
                .map(|(index, display)| {
                    let display_name = display
                        .name
                        .as_deref()
                        .map_or_else(|| format!("Display {}", index + 1), str::to_owned);
                    WindowAttributes::default()
                        .with_title(format!("Scatter Wallpaper — {display_name}"))
                        .with_decorations(false)
                        .with_resizable(false)
                        .with_active(false)
                        .with_position(display.position)
                        .with_inner_size(display.size)
                })
                .collect::<Vec<_>>();
            (attributes, displays)
        } else {
            (
                vec![
                    WindowAttributes::default()
                        .with_title("Scatter")
                        .with_inner_size(LogicalSize::new(960, 640))
                        .with_min_inner_size(LogicalSize::new(480, 320)),
                ],
                Vec::new(),
            )
        };

        let mut renderers = Vec::with_capacity(attributes.len());
        for attributes in attributes {
            renderers.push(self.create_window_renderer(
                event_loop,
                attributes,
                location,
                projection_kind,
                force_sdr,
            )?);
        }
        Ok((renderers, layout))
    }

    fn initialize_renderers(&mut self, event_loop: &ActiveEventLoop) -> AppResult<()> {
        let (renderers, layout) = self.build_renderers(event_loop, self.runtime_config.config())?;
        self.renderers = renderers;
        self.wallpaper_layout = layout;
        if self.window_mode.is_wallpaper() {
            eprintln!(
                "wallpaper mode initialized {} display(s)",
                self.renderers.len()
            );
        }
        Ok(())
    }

    fn refresh_config(&mut self, event_loop: &ActiveEventLoop, now: Instant) {
        let next = match self.runtime_config.refresh_if_changed(now) {
            Ok(Some(config)) => config,
            Ok(None) => return,
            Err(error) => {
                eprintln!("ignoring invalid updated config: {error}");
                return;
            }
        };

        match self.build_renderers(event_loop, &next) {
            Ok((renderers, layout)) => {
                self.renderers = renderers;
                self.wallpaper_layout = layout;
                self.runtime_config.apply(next, now);
                eprintln!(
                    "applied config reload from {}",
                    self.runtime_config.path().display()
                );
            }
            Err(error) => eprintln!("ignoring config update that could not be applied: {error}"),
        }
    }

    fn refresh_wallpaper_layout(&mut self, event_loop: &ActiveEventLoop) -> AppResult<()> {
        if !self.window_mode.is_wallpaper() {
            return Ok(());
        }
        let layout = Self::available_displays(event_loop);
        // Display enumeration can be momentarily empty while macOS applies a topology change.
        // Keep the existing windows and retry on the next one-second tick instead of exiting.
        if layout.is_empty() {
            return Ok(());
        }
        if layout != self.wallpaper_layout {
            eprintln!(
                "display layout changed from {} to {} display(s); rebuilding wallpaper windows",
                self.wallpaper_layout.len(),
                layout.len()
            );
            let (renderers, layout) =
                self.build_renderers(event_loop, self.runtime_config.config())?;
            self.renderers = renderers;
            self.wallpaper_layout = layout;
        }
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {
        let wall_time = SystemTime::now();
        let monotonic_time = Instant::now();
        for managed in &mut self.renderers {
            managed
                .render_schedule
                .events_resumed(wall_time, monotonic_time);
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.renderers.is_empty() {
            return;
        }
        if let Err(error) = self.initialize_renderers(event_loop) {
            eprintln!("failed to initialize Scatter: {error}");
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(managed) = self
            .renderers
            .iter_mut()
            .find(|managed| managed.renderer.window.id() == window_id)
        else {
            return;
        };
        let renderer = &mut managed.renderer;
        let render_schedule = &mut managed.render_schedule;

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
            WindowEvent::Occluded(occluded) => render_schedule.set_occluded(occluded),
            WindowEvent::RedrawRequested if render_schedule.can_render(Instant::now()) => {
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
        self.refresh_config(event_loop, now);
        if let Err(error) = self.refresh_wallpaper_layout(event_loop) {
            eprintln!("failed to update the wallpaper display layout: {error}");
            event_loop.exit();
            return;
        }
        for managed in &mut self.renderers {
            let renderer = &mut managed.renderer;
            if managed.render_schedule.take_reconfigure(now) {
                renderer.resize(renderer.window.inner_size());
            }
            if managed.render_schedule.can_render(now) {
                renderer.window.request_redraw();
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            (now + FRAME_INTERVAL).min(self.runtime_config.next_check_at()),
        ));
    }
}

fn main() -> AppResult<()> {
    let Some(options) = parse_startup_options(std::env::args_os().skip(1))
        .map_err(|error| format!("{error}\nRun with --help for usage."))?
    else {
        print_usage();
        return Ok(());
    };
    let runtime_config = config::RuntimeConfig::load(options.config_path)?;
    eprintln!("loaded config from {}", runtime_config.path().display());
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut App::new(options.window_mode, runtime_config))?;
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
mod command_line_tests {
    use super::*;

    #[test]
    fn wallpaper_flag_selects_wallpaper_mode() {
        assert_eq!(
            parse_startup_options(["--wallpaper"]),
            Ok(Some(StartupOptions {
                window_mode: WindowMode::Wallpaper,
                config_path: PathBuf::from("config.toml"),
            }))
        );
    }

    #[test]
    fn no_flag_keeps_normal_window_mode() {
        assert_eq!(
            parse_startup_options(std::iter::empty::<&str>()),
            Ok(Some(StartupOptions {
                window_mode: WindowMode::Normal,
                config_path: PathBuf::from("config.toml"),
            }))
        );
    }

    #[test]
    fn help_stops_before_opening_a_window() {
        assert_eq!(parse_startup_options(["--help"]), Ok(None));
    }

    #[test]
    fn unknown_flag_is_rejected() {
        assert_eq!(
            parse_startup_options(["--unknown"]),
            Err("unknown argument: --unknown".into())
        );
    }

    #[test]
    fn config_path_accepts_separate_and_joined_forms() {
        assert_eq!(
            parse_startup_options(["--config", "custom.toml"])
                .unwrap()
                .unwrap()
                .config_path,
            PathBuf::from("custom.toml")
        );
        assert_eq!(
            parse_startup_options(["--config=other.toml"])
                .unwrap()
                .unwrap()
                .config_path,
            PathBuf::from("other.toml")
        );
    }
}

#[cfg(test)]
mod display_layout_tests {
    use super::*;

    fn display(name: &str, x: i32, y: i32, width: u32, height: u32) -> DisplayGeometry {
        DisplayGeometry {
            name: Some(name.into()),
            position: PhysicalPosition::new(x, y),
            size: PhysicalSize::new(width, height),
        }
    }

    #[test]
    fn display_order_is_stable_across_monitor_enumeration_order() {
        let left = display("Left", -1920, 0, 1920, 1080);
        let primary = display("Primary", 0, 0, 2560, 1440);
        let above = display("Above", 0, -1080, 1920, 1080);
        let mut layout = vec![primary.clone(), left.clone(), above.clone()];

        App::sort_displays(&mut layout);

        assert_eq!(layout, vec![left, above, primary]);
    }

    #[test]
    fn geometry_change_produces_a_different_layout() {
        let original = vec![display("External", 0, 0, 1920, 1080)];
        let resized = vec![display("External", 0, 0, 2560, 1440)];

        assert_ne!(original, resized);
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
