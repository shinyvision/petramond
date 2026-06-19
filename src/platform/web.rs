//! Web (wasm) entry: `run(canvas)` called from JS.
//!
//! Sets up wgpu surface on an HTMLCanvasElement, winit via web events is not
//! used — we wire keyboard/mouse/pointer directly via web-sys. A
//! `requestAnimationFrame` loop drives app ticks.
//!
//! On wasm, futures returned by wgpu are JS-promise-backed: they cannot be
//! driven by a blocking executor like `pollster` (no threads, condvars trap).
//! We therefore construct the renderer inside `wasm_bindgen_futures::spawn_local`
//! and only begin ticking once it has resolved.

use crate::app::App;
use crate::camera::Camera;
use crate::mathh::Vec3;
use crate::render::Renderer;
use crate::world::RENDER_DIST;

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlCanvasElement, KeyboardEvent, MouseEvent};

/// Holds the initialised renderer + app once async construction completes.
/// Before that it is `None` and tick() is a no-op.
pub struct WebGame {
    pub canvas: HtmlCanvasElement,
    pub state: Option<GameState>,
}

pub struct GameState {
    pub renderer: Renderer,
    pub app: App,
}

impl WebGame {
    /// Synchronous shell; actual wgpu/app init happens in `boot`.
    pub fn new(canvas: HtmlCanvasElement) -> Rc<RefCell<WebGame>> {
        console_error_panic_hook::set_once();
        console_log::init_with_level(log::Level::Info).ok();
        Rc::new(RefCell::new(WebGame {
            canvas,
            state: None,
        }))
    }

    pub fn tick(&mut self) {
        let Some(s) = self.state.as_mut() else {
            return;
        };
        s.app.tick(&mut s.renderer);
    }
}

/// Top-level JS entry. Schedules async init, wires input, starts rAF loop.
#[wasm_bindgen]
pub fn run(canvas: HtmlCanvasElement) -> Result<JsValue, JsValue> {
    let game = WebGame::new(canvas.clone());

    // ---- Async boot: create renderer + app on the JS event loop. ----
    let g_boot = game.clone();
    let canvas_boot = canvas.clone();
    spawn_local(async move {
        let w = canvas_boot.client_width().max(1) as u32;
        let h = canvas_boot.client_height().max(1) as u32;
        let renderer = {
            let instance = wgpu::Instance::new(&crate::render::instance_descriptor());
            let target = wgpu::SurfaceTarget::Canvas(canvas_boot.clone());
            let surface = match instance.create_surface(target) {
                Ok(s) => s,
                Err(e) => {
                    web_sys::console::error_1(&format!("create_surface failed: {:?}", e).into());
                    return;
                }
            };
            crate::render::new_renderer_with_instance(instance, surface, w, h).await
        };
        let cam = Camera::new(Vec3::new(8.0, 90.0, 8.0), w as f32 / h as f32);
        let app = App::new(cam, 0x1234_5678u32, RENDER_DIST);
        g_boot.borrow_mut().state = Some(GameState { renderer, app });
        web_sys::console::log_1(&"llamacraft: renderer ready".into());
    });

    // ---- rAF loop: tick only once state is Some. ----
    schedule_next(game.clone());

    // ---- Input wiring. ----
    wire_input(game.clone())?;

    Ok(JsValue::UNDEFINED)
}

fn schedule_next(game: Rc<RefCell<WebGame>>) {
    let g = game.clone();
    let cb = Closure::<dyn FnMut()>::new(move || {
        g.borrow_mut().tick();
        schedule_next(g.clone());
    });
    let window = web_sys::window().unwrap();
    let _ = window.request_animation_frame(cb.as_ref().unchecked_ref());
    cb.forget();
}

fn wire_input(game: Rc<RefCell<WebGame>>) -> Result<(), JsValue> {
    let window = web_sys::window().unwrap();
    let document = window.document().unwrap();

    // Keys on document.
    let g2 = game.clone();
    let on_key = Closure::<dyn FnMut(KeyboardEvent)>::new(move |ev: KeyboardEvent| {
        let code = ev.code();
        let down = ev.type_() == "keydown";
        if !matches!(
            code.as_str(),
            "KeyW"
                | "KeyA"
                | "KeyS"
                | "KeyD"
                | "KeyY"
                | "Space"
                | "ShiftLeft"
                | "ShiftRight"
                | "ControlLeft"
                | "ControlRight"
        ) {
            return;
        }
        if code == "Space" || (code == "KeyY" && ev.ctrl_key()) {
            ev.prevent_default();
        }
        let mut g = g2.borrow_mut();
        if let Some(s) = g.state.as_mut() {
            s.app.set_key(&code, down);
        }
    });
    document.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref())?;
    document.add_event_listener_with_callback("keyup", on_key.as_ref().unchecked_ref())?;
    on_key.forget();

    // Pointer lock on canvas click.
    let g3 = game.clone();
    let canvas_for_listener = game.borrow().canvas.clone();
    let on_click = Closure::<dyn FnMut(MouseEvent)>::new(move |_ev: MouseEvent| {
        // Re-borrow canvas from game (kept alive there) for the lock request.
        let canvas = g3.borrow().canvas.clone();
        let _ = canvas.request_pointer_lock();
        let mut g = g3.borrow_mut();
        if let Some(s) = g.state.as_mut() {
            s.app.mouse.grabbing = true;
        }
    });
    canvas_for_listener
        .add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())?;
    on_click.forget();

    // Break/place: mousedown on canvas. Only act once the pointer is locked to
    // our canvas — the first click (which acquires the lock via `on_click`)
    // therefore doesn't accidentally edit a block. left=0 break, right=2 place.
    let g_md = game.clone();
    let doc_md = document.clone();
    let canvas_md = game.borrow().canvas.clone();
    let on_mousedown = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let locked = doc_md.pointer_lock_element().map(|e| e.id());
        let canvas_id = g_md.borrow().canvas.id();
        if locked.as_deref() != Some(canvas_id.as_str()) {
            return;
        }
        let mut g = g_md.borrow_mut();
        if let Some(s) = g.state.as_mut() {
            match ev.button() {
                0 => s.app.mouse.left_click = true,
                2 => s.app.mouse.right_click = true,
                _ => {}
            }
        }
    });
    canvas_md
        .add_event_listener_with_callback("mousedown", on_mousedown.as_ref().unchecked_ref())?;
    on_mousedown.forget();

    // Suppress the browser context menu so right-click can place blocks.
    let on_contextmenu = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        ev.prevent_default();
    });
    canvas_md
        .add_event_listener_with_callback("contextmenu", on_contextmenu.as_ref().unchecked_ref())?;
    on_contextmenu.forget();

    // Mousemove deltas (only while pointer locked to our canvas).
    let g4 = game.clone();
    let doc_for_move = document.clone();
    let on_move = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let locked = doc_for_move.pointer_lock_element().map(|e| e.id());
        let canvas_id = g4.borrow().canvas.id();
        if locked.as_deref() != Some(canvas_id.as_str()) {
            return;
        }
        let mut g = g4.borrow_mut();
        if let Some(s) = g.state.as_mut() {
            s.app.mouse.dx += ev.movement_x() as f32;
            s.app.mouse.dy += ev.movement_y() as f32;
        }
    });
    document.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())?;
    on_move.forget();

    // Resize: apply to renderer + camera aspect when state is ready.
    let g5 = game.clone();
    let on_resize = Closure::<dyn FnMut()>::new(move || {
        let canvas = g5.borrow().canvas.clone();
        let w = canvas.client_width().max(1) as u32;
        let h = canvas.client_height().max(1) as u32;
        let mut g = g5.borrow_mut();
        if let Some(s) = g.state.as_mut() {
            s.renderer.resize(w, h);
            s.app.cam.aspect = w as f32 / h as f32;
        }
    });
    window.add_event_listener_with_callback("resize", on_resize.as_ref().unchecked_ref())?;
    on_resize.forget();

    Ok(())
}
