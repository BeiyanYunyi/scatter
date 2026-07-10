mod solar;

use bytemuck::{Pod, Zeroable};
use chrono::{DateTime, FixedOffset, Local};
use solar::solar_position;
use std::{error::Error, sync::Arc, time::Duration};
use wgpu::util::DeviceExt;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowAttributes, WindowId},
};

type AppResult<T> = Result<T, Box<dyn Error>>;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    resolution: [f32; 2],
    padding: [f32; 2],
    sun_direction: [f32; 4],
    atmosphere: [f32; 4],
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
}

enum RenderStatus {
    Presented,
    Reconfigure,
    Skip,
    Fatal,
}

impl Renderer {
    async fn new(window: Arc<Window>, location: Location) -> AppResult<Self> {
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
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::include_wgsl!("sky.wgsl"));
        let now = Local::now().fixed_offset();
        let sun = solar_position(&now, location.latitude, location.longitude);
        let initial_uniforms = Self::uniforms(size, sun);
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
        })
    }

    fn uniforms(size: winit::dpi::PhysicalSize<u32>, sun: solar::SolarPosition) -> Uniforms {
        let direction = sun.direction();
        Uniforms {
            resolution: [size.width.max(1) as f32, size.height.max(1) as f32],
            padding: [0.0; 2],
            sun_direction: [direction[0], direction[1], direction[2], 0.0],
            // x: exposure, y: observer altitude in kilometers.
            atmosphere: [1.0, 0.002, 0.0, 0.0],
        }
    }

    fn update(&self) {
        let now = Local::now().fixed_offset();
        let sun = solar_position(&now, self.location.latitude, self.location.longitude);
        let uniforms = Self::uniforms(self.window.inner_size(), sun);
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        self.window.set_title(&format!(
            "Scatter — {}  |  sun {:.1}° high, azimuth {:.1}°",
            now.format("%Y-%m-%d %H:%M:%S %:z"),
            sun.elevation_deg,
            sun.azimuth_deg,
        ));
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
        self.update();
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
}

impl ApplicationHandler for App {
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
        let attributes = WindowAttributes::default()
            .with_title("Scatter")
            .with_inner_size(LogicalSize::new(1280, 720))
            .with_min_inner_size(LogicalSize::new(640, 360));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("failed to create window: {error}");
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(Renderer::new(window, location)) {
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
            WindowEvent::RedrawRequested => match renderer.render() {
                RenderStatus::Presented | RenderStatus::Skip => {}
                RenderStatus::Reconfigure => {
                    renderer.resize(renderer.window.inner_size());
                }
                RenderStatus::Fatal => {
                    eprintln!("the rendering surface was lost or failed validation");
                    event_loop.exit();
                }
            },
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(renderer) = &self.renderer {
            renderer.window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + Duration::from_secs(1),
        ));
    }
}

fn main() -> AppResult<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut App::default())?;
    Ok(())
}
