//! Native desktop platform host: winit window + wgpu surface.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::App;
use crate::camera::Camera;
use crate::controls::{control_from_key_code, Control, PointerButton};
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

    let mut host = NativeHost::new(seed, rd);
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut host).unwrap();
}

struct NativeHost {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    app: Option<App>,
    seed: u32,
    render_dist: i32,
    next_frame: Instant,
}

impl NativeHost {
    fn new(seed: u32, render_dist: i32) -> Self {
        Self {
            window: None,
            renderer: None,
            app: None,
            seed,
            render_dist,
            next_frame: Instant::now(),
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
        let app = App::new(cam, self.seed, self.render_dist);

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
                app.tick(renderer);
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
        let now = Instant::now();
        if now >= self.next_frame {
            self.next_frame = now + FRAME;
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
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
