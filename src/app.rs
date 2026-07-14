use crate::{
    AppResult,
    cli::WindowMode,
    config,
    projection::ProjectionKind,
    renderer::{RenderStatus, Renderer},
};
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{WindowAttributes, WindowId},
};

const FRAME_INTERVAL: Duration = Duration::from_secs(1);
const WAKE_LAG_THRESHOLD: Duration = Duration::from_secs(2);
const WAKE_RECOVERY_DELAY: Duration = Duration::from_secs(1);

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

struct App {
    renderers: Vec<ManagedRenderer>,
    window_mode: WindowMode,
    wallpaper_layout: Vec<DisplayGeometry>,
    runtime_config: config::RuntimeConfig,
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
        location: config::LocationConfig,
        projection_kind: ProjectionKind,
        force_sdr: bool,
    ) -> AppResult<ManagedRenderer> {
        let window = Arc::new(event_loop.create_window(attributes)?);
        if self.window_mode.is_wallpaper() {
            #[cfg(target_os = "macos")]
            crate::macos::configure_wallpaper_window(&window)?;
            #[cfg(not(target_os = "macos"))]
            return Err("wallpaper mode is only supported on macOS".into());
        }

        let renderer =
            pollster::block_on(Renderer::new(window, location, projection_kind, force_sdr))?;
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
        let location = app_config.location;
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
            .find(|managed| managed.renderer.window().id() == window_id)
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
                        renderer.resize(renderer.window().inner_size());
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
                renderer.resize(renderer.window().inner_size());
            }
            if managed.render_schedule.can_render(now) {
                renderer.window().request_redraw();
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            (now + FRAME_INTERVAL).min(self.runtime_config.next_check_at()),
        ));
    }
}

pub(crate) fn run(window_mode: WindowMode, runtime_config: config::RuntimeConfig) -> AppResult<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut App::new(window_mode, runtime_config))?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
