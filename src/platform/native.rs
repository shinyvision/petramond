//! Native desktop platform host: winit window + wgpu surface.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::App;
use crate::camera::Camera;
use crate::controls::{control_from_key_code, Control, Modifiers, PointerButton};
use crate::mathh::Vec3;
use crate::render::{new_renderer_from_target, Renderer};
use crate::world::RENDER_DIST;

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{
    DeviceEvent, ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, Window, WindowId};

const TARGET_FPS: u64 = 60;
const FRAME: Duration = Duration::from_nanos(1_000_000_000 / TARGET_FPS);
/// Idle update cadence. The sim is decoupled from drawing: when nothing is animating
/// and no input is pending, the host still wakes this often to run `App::update` (so
/// the world keeps ticking at 20 TPS) but skips the draw. Input wakes it sooner.
const IDLE_TICK: Duration = Duration::from_millis(50);
/// Redraw at least this often even when fully idle, so slow continuous changes (sky /
/// fog drift) and any untracked state never stay stale — a cheap on-demand-draw backstop.
const KEEPALIVE: Duration = Duration::from_millis(250);

pub fn run() {
    env_logger::init();
    let seed: u32 = std::env::var("LLAMACRAFT_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x1234_5678);
    let rd: i32 = std::env::var("LLAMACRAFT_RD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(RENDER_DIST);
    let world_name = std::env::var("LLAMACRAFT_WORLD").unwrap_or_else(|_| "world".to_string());

    let mut host = NativeHost::new(world_name, seed, rd);
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut host).unwrap();

    // Final save on quit: queue the writes, then dropping `host` joins the save
    // thread so everything is flushed before the process exits.
    if let Some(app) = host.app.as_mut() {
        app.save_on_exit();
    }
}

struct NativeHost {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    app: Option<App>,
    world_name: String,
    seed: u32,
    render_dist: i32,
    /// When the next `App::update` (sim + input) is due — `now + FRAME` while active,
    /// `now + IDLE_TICK` while idle. Drives the decoupled sim/render cadences.
    next_update: Instant,
    /// When the last frame was requested, for the frame-rate cap and keep-alive.
    last_draw: Instant,
    /// Per-second updates/redraws counters logged to stderr when `LLAMACRAFT_PERF` is
    /// set — shows the decoupled rates (e.g. idle ≈ sim updates with few redraws).
    perf_log: bool,
    perf_since: Instant,
    perf_updates: u32,
    perf_redraws: u32,
}

impl NativeHost {
    fn new(world_name: String, seed: u32, render_dist: i32) -> Self {
        Self {
            window: None,
            renderer: None,
            app: None,
            world_name,
            seed,
            render_dist,
            next_update: Instant::now(),
            last_draw: Instant::now(),
            perf_log: std::env::var_os("LLAMACRAFT_PERF").is_some(),
            perf_since: Instant::now(),
            perf_updates: 0,
            perf_redraws: 0,
        }
    }
}

impl ApplicationHandler for NativeHost {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Llamacraft")
            .with_inner_size(PhysicalSize::new(1280, 720));
        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        let size = window.inner_size();
        let renderer = pollster::block_on(async {
            new_renderer_from_target(window.clone(), size.width, size.height).await
        });
        let cam = Camera::new(
            Vec3::new(8.0, 90.0, 8.0),
            size.width as f32 / size.height.max(1) as f32,
        );
        let app = App::new(cam, &self.world_name, self.seed, self.render_dist);

        let _ = window.set_cursor_grab(CursorGrabMode::Confined);
        window.set_cursor_visible(false);
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.app = Some(app);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(app) = self.app.as_mut() else {
            return;
        };
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                renderer.resize(size.width, size.height);
                app.resize(size.width, size.height);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = window.inner_size();
                renderer.resize(size.width, size.height);
                app.resize(size.width, size.height);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        ..
                    },
                ..
            } => {
                let down = state == ElementState::Pressed;
                if let Some(control) = control_from_key_code(code) {
                    let consumed = app.handle_control(control, down);
                    if matches!(control, Control::CloseScreen) && down && !consumed {
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                // Physical Ctrl/Shift state, tracked apart from the rebindable
                // Sprint/Sneak controls so UI modifiers don't follow a rebind.
                let state = mods.state();
                app.set_modifiers(Modifiers {
                    ctrl: state.control_key(),
                    shift: state.shift_key(),
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                app.add_scroll_delta(-wheel_notches(delta));
            }
            WindowEvent::CursorMoved { position, .. } => {
                app.set_cursor_position(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { state, button, .. } => match (state, button) {
                (ElementState::Pressed, MouseButton::Left) => {
                    app.set_pointer_button(PointerButton::Primary, true);
                }
                (ElementState::Released, MouseButton::Left) => {
                    app.set_pointer_button(PointerButton::Primary, false);
                }
                (ElementState::Pressed, MouseButton::Right) => {
                    app.set_pointer_button(PointerButton::Secondary, true);
                }
                (ElementState::Released, MouseButton::Right) => {
                    app.set_pointer_button(PointerButton::Secondary, false);
                }
                _ => {}
            },
            WindowEvent::RedrawRequested => {
                let cursor = app.cursor_policy();
                let grab = if cursor.grabbed {
                    CursorGrabMode::Confined
                } else {
                    CursorGrabMode::None
                };
                let _ = window.set_cursor_grab(grab);
                window.set_cursor_visible(cursor.visible);
                // The host requests this only when `App::update` (or the keep-alive)
                // asked for it; the simulation itself advances in `about_to_wait`.
                app.render(renderer);
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        let Some(app) = self.app.as_mut() else {
            return;
        };
        match event {
            DeviceEvent::MouseMotion { delta: (dx, dy) } => {
                app.add_pointer_motion(dx as f32, dy as f32);
            }
            DeviceEvent::MouseWheel { delta } => {
                app.add_scroll_delta(-wheel_notches(delta));
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(app), Some(renderer)) = (self.app.as_mut(), self.renderer.as_ref()) else {
            return;
        };
        let now = Instant::now();
        // Advance the simulation + input on its own clock, decoupled from drawing, and
        // request a draw only when the frame would actually differ. Pending input pulls
        // an update forward — but never sooner than the frame cap, so interaction stays
        // responsive without uncapping the frame rate.
        let frame_ready = now.saturating_duration_since(self.last_draw) >= FRAME;
        let due = now >= self.next_update || (app.wants_redraw() && frame_ready);
        if due {
            let need_render = app.update(renderer);
            self.perf_updates += 1;
            let keepalive = now.saturating_duration_since(self.last_draw) >= KEEPALIVE;
            if need_render || keepalive {
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
                self.last_draw = now;
                self.next_update = now + FRAME;
                self.perf_redraws += 1;
            } else {
                // Idle: keep ticking the sim, but don't draw.
                self.next_update = now + IDLE_TICK;
            }
        }
        if self.perf_log && now.saturating_duration_since(self.perf_since).as_secs() >= 1 {
            eprintln!(
                "perf: {} updates/s, {} redraws/s",
                self.perf_updates, self.perf_redraws
            );
            self.perf_updates = 0;
            self.perf_redraws = 0;
            self.perf_since = now;
        }
        // Wake at the next scheduled update; if input is pending but frame-capped, wake
        // at the cap instead so it's served within a frame.
        let wake = if app.wants_redraw() {
            self.next_update.min(self.last_draw + FRAME)
        } else {
            self.next_update
        };
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
    }

    /// Tear down the GPU + window state here, while winit still holds the live
    /// Wayland connection. winit calls `exiting` as the loop winds down (after
    /// `event_loop.exit()`), before the `EventLoop` — and the Wayland connection
    /// it owns — is dropped.
    ///
    /// This ordering is load-bearing: `wgpu::Instance` enables every backend, so
    /// it always spins up a GLES/EGL (Mesa) instance even when we render through
    /// Vulkan. The whole wgpu context (`wgpu_core::Global`) is kept alive by the
    /// GPU objects the `Renderer` holds, so it only drops when the `Renderer`
    /// does. That drop runs `eglTerminate`, which talks to the Wayland display.
    /// If the `Renderer` instead dropped with `host` at the end of `run` — after
    /// the `EventLoop` (a later-declared local) had already closed the Wayland
    /// connection — `eglTerminate` calls into freed `libwayland-client` proxies
    /// and segfaults on exit. Dropping it here keeps the connection valid for the
    /// teardown. Drop the renderer first so its surface releases its `Arc<Window>`
    /// before we drop ours.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.renderer = None;
        self.window = None;
    }
}

/// A scroll event as a count of wheel notches (`1.0` == one detent). winit
/// already divides Windows' raw `WHEEL_DELTA` (120) into `LineDelta`, so a
/// classic detent is `1.0` and a hi-res / free-spin wheel reports the fractions
/// that sum to it. Pixel-precise devices report `PixelDelta`, normalized through
/// [`super::PIXELS_PER_NOTCH`] so both paths feed the accumulator the same unit.
fn wheel_notches(delta: MouseScrollDelta) -> f32 {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => y,
        MouseScrollDelta::PixelDelta(p) => p.y as f32 / super::PIXELS_PER_NOTCH,
    }
}
