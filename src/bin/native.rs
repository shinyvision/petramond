//! Native desktop binary: winit window + wgpu surface, runs `App::tick`.

use std::sync::Arc;

use llamacraft::app::App;
use llamacraft::camera::Camera;
use llamacraft::mathh::Vec3;
use llamacraft::render::{new_renderer_from_target, Renderer};
use llamacraft::world::RENDER_DIST;

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

struct Game {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    app: Option<App>,
    seed: u32,
    render_dist: i32,
}

impl Game {
    fn new(seed: u32, render_dist: i32) -> Self {
        Self { window: None, renderer: None, app: None, seed, render_dist }
    }
}

impl ApplicationHandler for Game {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let attrs = Window::default_attributes()
            .with_title("Llamacraft")
            .with_inner_size(PhysicalSize::new(1280, 720));
        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        let size = window.inner_size();
        let renderer = pollster::block_on(async {
            new_renderer_from_target(window.clone(), size.width, size.height).await
        });
        let cam = Camera::new(Vec3::new(8.0, 90.0, 8.0), size.width as f32 / size.height as f32);
        let app = App::new(cam, self.seed, self.render_dist);
        // Try to grab cursor.
        let _ = window.set_cursor_grab(CursorGrabMode::Confined);
        window.set_cursor_visible(false);
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.app = Some(app);
    }

    fn window_event(
        &mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent,
    ) {
        let Some(app) = self.app.as_mut() else { return; };
        let Some(renderer) = self.renderer.as_mut() else { return; };
        let Some(window) = self.window.as_ref() else { return; };
        match event {
            WindowEvent::CloseRequested => { event_loop.exit(); }
            WindowEvent::Resized(s) => {
                renderer.resize(s.width, s.height);
                app.cam.aspect = s.width as f32 / s.height as f32;
            }
            WindowEvent::ScaleFactorChanged { scale_factor:_, inner_size_writer: _ } => {
                let s = window.inner_size();
                renderer.resize(s.width, s.height);
                app.cam.aspect = s.width as f32 / s.height as f32;
            }
            WindowEvent::KeyboardInput { event: KeyEvent { physical_key, state, .. }, .. } => {
                if let PhysicalKey::Code(code) = physical_key {
                    let down = state == ElementState::Pressed;
                    let code_str = keycode_str(code);
                    if !code_str.is_empty() {
                        app.set_key(code_str, down);
                    }
                    if code == KeyCode::Escape {
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                app.tick(renderer);
                window.request_redraw();
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self, _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId, event: DeviceEvent,
    ) {
        let Some(app) = self.app.as_mut() else { return; };
        match event {
            DeviceEvent::MouseMotion { delta: (dx, dy) } => {
                app.mouse.dx += dx as f32;
                app.mouse.dy += dy as f32;
                app.mouse.grabbing = true;
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = self.window.as_ref() { w.request_redraw(); }
    }
}

fn keycode_str(c: KeyCode) -> &'static str {
    match c {
        KeyCode::KeyW => "KeyW",
        KeyCode::KeyA => "KeyA",
        KeyCode::KeyS => "KeyS",
        KeyCode::KeyD => "KeyD",
        KeyCode::Space => "Space",
        KeyCode::ShiftLeft => "ShiftLeft",
        KeyCode::ShiftRight => "ShiftRight",
        KeyCode::ControlLeft => "ControlLeft",
        KeyCode::ControlRight => "ControlRight",
        _ => "",
    }
}

fn main() {
    env_logger::init();
    let seed: u32 = std::env::var("LLAMACRAFT_SEED")
        .ok().and_then(|s| s.parse().ok())
        .unwrap_or(0x1234_5678);
    let rd: i32 = std::env::var("LLAMACRAFT_RD")
        .ok().and_then(|s| s.parse().ok())
        .unwrap_or(RENDER_DIST);

    let mut game = Game::new(seed, rd);
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut game).unwrap();
}