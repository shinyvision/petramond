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
use crate::controls::{control_from_code, Control, PointerButton};
use crate::mathh::Vec3;
use crate::render::Renderer;
use crate::world::RENDER_DIST;

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlCanvasElement, KeyboardEvent, MouseEvent, WheelEvent};

/// Holds the initialised renderer + app once async construction completes.
/// Before that it is `None` and tick() is a no-op.
pub struct WebHost {
    pub canvas: HtmlCanvasElement,
    pub state: Option<RuntimeState>,
}

pub struct RuntimeState {
    pub renderer: Renderer,
    pub app: App,
}

impl WebHost {
    /// Synchronous shell; actual wgpu/app init happens in `boot`.
    pub fn new(canvas: HtmlCanvasElement) -> Rc<RefCell<WebHost>> {
        console_error_panic_hook::set_once();
        console_log::init_with_level(log::Level::Info).ok();
        Rc::new(RefCell::new(WebHost {
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
    let host = WebHost::new(canvas.clone());

    // ---- Async boot: create renderer + app on the JS event loop. ----
    let g_boot = host.clone();
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
        g_boot.borrow_mut().state = Some(RuntimeState { renderer, app });
        web_sys::console::log_1(&"llamacraft: renderer ready".into());
    });

    // ---- rAF loop: tick only once state is Some. ----
    schedule_next(host.clone());

    // ---- Input wiring. ----
    wire_input(host.clone())?;

    Ok(JsValue::UNDEFINED)
}

fn schedule_next(host: Rc<RefCell<WebHost>>) {
    let g = host.clone();
    let cb = Closure::<dyn FnMut()>::new(move || {
        g.borrow_mut().tick();
        schedule_next(g.clone());
    });
    let window = web_sys::window().unwrap();
    let _ = window.request_animation_frame(cb.as_ref().unchecked_ref());
    cb.forget();
}

fn wire_input(host: Rc<RefCell<WebHost>>) -> Result<(), JsValue> {
    let window = web_sys::window().unwrap();
    let document = window.document().unwrap();

    // Keys on document.
    let g2 = host.clone();
    let doc_key = document.clone();
    let on_key = Closure::<dyn FnMut(KeyboardEvent)>::new(move |ev: KeyboardEvent| {
        let code = ev.code();
        let down = ev.type_() == "keydown";
        let Some(control) = control_from_code(&code) else {
            return;
        };
        if code == "Space" || (code == "KeyY" && ev.ctrl_key()) {
            ev.prevent_default();
        }
        let is_inventory_toggle = matches!(control, Control::ToggleInventory) && down;
        // Escape on keydown closes an open inventory (App mirrors the E-close
        // side effects). The browser auto-exits pointer lock on Escape, so we
        // don't request it back here — the user re-clicks the canvas to re-lock.
        // Forward the key and capture the resulting inventory state in a tight
        // borrow scope, so the pointer-lock calls below run after the borrow
        // (and the canvas re-borrow) is released.
        let cursor_policy = {
            let mut g = g2.borrow_mut();
            match g.state.as_mut() {
                Some(s) => {
                    s.app.handle_control(control, down);
                    Some(s.app.cursor_policy())
                }
                None => None,
            }
        };
        // The E keydown is a user gesture, so we can (re)acquire or release
        // pointer lock to match the inventory state it just toggled.
        if is_inventory_toggle {
            if let Some(cursor) = cursor_policy {
                if cursor.visible {
                    doc_key.exit_pointer_lock();
                } else {
                    let canvas = g2.borrow().canvas.clone();
                    let _ = canvas.request_pointer_lock();
                }
            }
        }
    });
    document.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref())?;
    document.add_event_listener_with_callback("keyup", on_key.as_ref().unchecked_ref())?;
    on_key.forget();

    // Pointer lock on canvas click.
    let g3 = host.clone();
    let canvas_for_listener = host.borrow().canvas.clone();
    let on_click = Closure::<dyn FnMut(MouseEvent)>::new(move |_ev: MouseEvent| {
        let should_grab = {
            let mut g = g3.borrow_mut();
            match g.state.as_mut() {
                Some(s) if s.app.cursor_policy().grabbed => {
                    s.app.set_pointer_grabbed(true);
                    true
                }
                _ => false,
            }
        };
        if should_grab {
            let canvas = g3.borrow().canvas.clone();
            let _ = canvas.request_pointer_lock();
        }
    });
    canvas_for_listener
        .add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())?;
    on_click.forget();

    // Break/place: mousedown on canvas. Only act once the pointer is locked to
    // our canvas — the first click (which acquires the lock via `on_click`)
    // therefore doesn't accidentally edit a block. left=0 break, right=2 place.
    let g_md = host.clone();
    let doc_md = document.clone();
    let canvas_md = host.borrow().canvas.clone();
    let on_mousedown = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let locked = doc_md.pointer_lock_element().map(|e| e.id());
        let canvas_id = g_md.borrow().canvas.id();
        if locked.as_deref() != Some(canvas_id.as_str()) {
            return;
        }
        let mut g = g_md.borrow_mut();
        if let Some(s) = g.state.as_mut() {
            match ev.button() {
                // Left is hold-to-break: set both the edge flag (parity) and the
                // level `left_held` that drives timed mining.
                0 => {
                    s.app.set_pointer_button(PointerButton::Primary, true);
                }
                2 => s.app.set_pointer_button(PointerButton::Secondary, true),
                _ => {}
            }
        }
    });
    canvas_md
        .add_event_listener_with_callback("mousedown", on_mousedown.as_ref().unchecked_ref())?;
    on_mousedown.forget();

    // Mouseup: clear the held-left state so mining stops on release. Listened on
    // the document (not just the canvas) so a release outside the canvas — e.g.
    // after the pointer leaves while dragging — still ends the hold.
    let g_mu = host.clone();
    let on_mouseup = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        if ev.button() != 0 {
            return;
        }
        let mut g = g_mu.borrow_mut();
        if let Some(s) = g.state.as_mut() {
            s.app.set_pointer_button(PointerButton::Primary, false);
        }
    });
    document.add_event_listener_with_callback("mouseup", on_mouseup.as_ref().unchecked_ref())?;
    on_mouseup.forget();

    // Suppress the browser context menu so right-click can place blocks.
    let on_contextmenu = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        ev.prevent_default();
    });
    canvas_md
        .add_event_listener_with_callback("contextmenu", on_contextmenu.as_ref().unchecked_ref())?;
    on_contextmenu.forget();

    // Mousemove. When pointer-locked to our canvas (playing) we accumulate raw
    // movement deltas for mouse-look. When NOT locked (inventory open) we instead
    // track the absolute cursor position for the UI hit-test / drag cursor: the
    // canvas-relative offset, scaled into the renderer's surface coordinate space
    // (config.width/height) so it matches the UI layout. The surface is sized from
    // the canvas client size, so the ratio also folds in any devicePixelRatio
    // difference between CSS pixels and the render target.
    let g4 = host.clone();
    let doc_for_move = document.clone();
    let on_move = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let canvas_id = g4.borrow().canvas.id();
        let locked = doc_for_move.pointer_lock_element().map(|e| e.id());
        let is_locked = locked.as_deref() == Some(canvas_id.as_str());
        let mut g = g4.borrow_mut();
        let WebHost { canvas, state, .. } = &mut *g;
        let Some(s) = state.as_mut() else {
            return;
        };
        if is_locked {
            s.app
                .add_pointer_motion(ev.movement_x() as f32, ev.movement_y() as f32);
        } else {
            // Map the canvas-relative CSS-pixel offset into surface pixels.
            let (sw, sh) = s.renderer.screen_size();
            let cw = canvas.client_width().max(1) as f32;
            let ch = canvas.client_height().max(1) as f32;
            s.app.set_cursor_position(
                ev.offset_x() as f32 * (sw as f32 / cw),
                ev.offset_y() as f32 * (sh as f32 / ch),
            );
        }
    });
    document.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())?;
    on_move.forget();

    // Mouse wheel -> hotbar scroll. `WheelEvent.deltaY` is positive on wheel-down
    // already, matching the App's shared convention (positive == wheel-down).
    // `preventDefault` stops the page from scrolling.
    let g_wheel = host.clone();
    let canvas_wheel = host.borrow().canvas.clone();
    let on_wheel = Closure::<dyn FnMut(WheelEvent)>::new(move |ev: WheelEvent| {
        ev.prevent_default();
        let mut g = g_wheel.borrow_mut();
        if let Some(s) = g.state.as_mut() {
            s.app.add_scroll_delta(wheel_notches(&ev));
        }
    });
    canvas_wheel.add_event_listener_with_callback("wheel", on_wheel.as_ref().unchecked_ref())?;
    on_wheel.forget();

    // Resize: apply to renderer + camera aspect when state is ready.
    let g5 = host.clone();
    let on_resize = Closure::<dyn FnMut()>::new(move || {
        let canvas = g5.borrow().canvas.clone();
        let w = canvas.client_width().max(1) as u32;
        let h = canvas.client_height().max(1) as u32;
        let mut g = g5.borrow_mut();
        if let Some(s) = g.state.as_mut() {
            s.renderer.resize(w, h);
            s.app.resize(w, h);
        }
    });
    window.add_event_listener_with_callback("resize", on_resize.as_ref().unchecked_ref())?;
    on_resize.forget();

    Ok(())
}

/// A `wheel` event as a count of notches (`1.0` == one detent), in the App's
/// "positive == wheel-down" convention. Pixel-mode deltas — what hi-res /
/// free-spin mice and trackpads report, often a few px per event — are scaled by
/// [`super::PIXELS_PER_NOTCH`] so they accumulate toward a slot instead of
/// jumping one per event. Line- and page-mode deltas already approximate notches.
fn wheel_notches(ev: &WheelEvent) -> f32 {
    let dy = ev.delta_y() as f32;
    match ev.delta_mode() {
        WheelEvent::DOM_DELTA_PIXEL => dy / super::PIXELS_PER_NOTCH,
        _ => dy,
    }
}
