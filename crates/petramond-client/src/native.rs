//! Native desktop platform host: winit window + wgpu surface.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::{App, CursorIcon as AppCursorIcon, CursorPolicy, ExitKind};
use crate::keymap::{key_code, mouse_button, text_key_from_named};
use petramond_input::controls::Modifiers;
use petramond_render::camera::Camera;
use petramond_render::{new_renderer_from_target, RenderFailure, Renderer};

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{DeviceEvent, ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, CursorIcon as WinitCursorIcon, Window, WindowId};

fn frame_period(fps: u32) -> Duration {
    Duration::from_nanos(1_000_000_000 / fps.clamp(10, 240) as u64)
}

fn client_content_stages() -> [&'static dyn petramond_world::content::Stage; 3] {
    [
        &petramond_audio::music_registry::CATALOG,
        &crate::animation::locomotion::TABLE,
        petramond_render::atlas::stage(),
    ]
}

pub fn run() {
    petramond::platform::init_logging();
    // Staged content changes land before any pack is discovered; the lock
    // this process then holds (shared) keeps every other Petramond process
    // from applying changes under it for as long as it runs.
    let (content_report, content_lock) = petramond::content::apply_pending();
    // Every content catalog loads here, once, before anything touches one: a
    // bad pack is a load report on stderr, not a panic on whichever thread
    // first read a catalog.
    if let Err(e) = petramond::content::install_from_env(&client_content_stages()) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    // Pack discovery + any cold wasm compiles happen behind the shell menu,
    // so the first world open finds every mod module ready.
    petramond::modding::prewarm_modules();
    let seed: u32 = std::env::var("PETRAMOND_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x1234_5678);
    // Env var > client.json > built-in default, so one-off runs can override
    // the persistent setting.
    petramond::save::client::ensure_file();
    let client = petramond::save::client::load();
    let rd: i32 = std::env::var("PETRAMOND_RD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(client.render_dist)
        .clamp(4, 64);
    let fps: u32 = std::env::var("PETRAMOND_FPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(client.fps_cap);
    let mut host = NativeHost::new(seed, rd, fps, client.menu_fps_cap.min(fps));
    host.content_report = Some(content_report);
    host.content_lock = content_lock;
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut host).unwrap();

    // Final save on quit: queue the writes, then dropping `host` joins the save
    // thread so everything is flushed before the process exits.
    let relaunch = host.app.as_mut().and_then(|app| {
        app.save_on_exit();
        app.take_relaunch()
    });
    let content_lock = host.content_lock.take();
    drop(host);
    // What mods queued to their files (the instances' closing syncs
    // included) reaches the disk before the I/O threads die with the
    // process.
    petramond::modding::client::files::flush();
    if let Some(route) = relaunch {
        drop(content_lock);
        if let Err(e) = petramond_util::process::spawn_self(&route) {
            log::error!("could not start Petramond again: {e}");
        }
    }
}

struct NativeHost {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    app: Option<App>,
    seed: u32,
    render_dist: i32,
    /// Gameplay frame period (from the `fps_cap` client setting / `PETRAMOND_FPS`).
    frame: Duration,
    /// Frame period while a modal screen is up (cursor released): near-static
    /// screens don't need the full gameplay rate. Still renders every frame.
    menu_frame: Duration,
    /// When the next frame (`App::update` + redraw) is due — the frame cap.
    next_update: Instant,
    /// Per-second update/render counters logged to stderr when `PETRAMOND_PERF` is set.
    perf_log: bool,
    perf_since: Instant,
    perf_updates: u32,
    perf_renders: u32,
    perf_update_total: Duration,
    perf_update_max: Duration,
    perf_render_total: Duration,
    perf_render_max: Duration,
    /// Last cursor grab/visibility state applied to the window. Cursor policy changes
    /// only when screens open/close; reapplying it every redraw sends compositor work
    /// through the hot mouse-look path.
    cursor_policy: Option<CursorPolicy>,
    modifiers: Modifiers,
    /// The display refresh last handed to the app, in millihertz.
    refresh_mhz: Option<u32>,
    /// The startup content apply's report, handed to the app once it exists.
    content_report: Option<petramond::content::ApplyReport>,
    content_lock: Option<petramond::content::ContentLock>,
}

impl NativeHost {
    fn new(seed: u32, render_dist: i32, fps_cap: u32, menu_fps_cap: u32) -> Self {
        Self {
            window: None,
            renderer: None,
            app: None,
            seed,
            render_dist,
            frame: frame_period(fps_cap),
            menu_frame: frame_period(menu_fps_cap),
            next_update: Instant::now(),
            perf_log: std::env::var_os("PETRAMOND_PERF").is_some(),
            perf_since: Instant::now(),
            perf_updates: 0,
            perf_renders: 0,
            perf_update_total: Duration::ZERO,
            perf_update_max: Duration::ZERO,
            perf_render_total: Duration::ZERO,
            perf_render_max: Duration::ZERO,
            cursor_policy: None,
            modifiers: Modifiers::default(),
            refresh_mhz: None,
            content_report: None,
            content_lock: None,
        }
    }

    fn apply_content(&mut self, event_loop: &ActiveEventLoop) {
        let dirs = petramond::content::Dirs::installed();
        if self.content_lock.is_none() {
            self.content_lock = petramond::content::ContentLock::shared(&dirs).ok();
        }
        let Some(lock) = self.content_lock.as_mut() else {
            if let Some(app) = self.app.as_mut() {
                app.content_applied(petramond::content::ApplyReport {
                    failed: vec![(
                        "content lock".into(),
                        "could not open the content lock".into(),
                    )],
                    ..Default::default()
                });
            }
            return;
        };
        let report = petramond::content::install::apply_pending_live(&dirs, lock);
        if !report.applied.is_empty() {
            if let Err(error) = petramond::content::install_from_env(&client_content_stages()) {
                log::error!("installed content failed to load: {error}");
                eprintln!("Petramond cannot load the installed content: {error}");
                event_loop.exit();
                return;
            }
            petramond::modding::clear_module_cache();
            petramond::gui::doc_theme::reload();
            petramond::gui::documents::reload();
            let Some(window) = self.window.clone() else {
                return;
            };
            self.renderer = None;
            let size = window.inner_size();
            match pollster::block_on(new_renderer_from_target(window, size.width, size.height)) {
                Ok(mut renderer) => {
                    if let Some(app) = self.app.as_mut() {
                        app.renderer_recreated(&mut renderer);
                    }
                    self.renderer = Some(renderer);
                }
                Err(error) => {
                    fatal_render_error(event_loop, &error);
                    return;
                }
            }
            petramond::modding::prewarm_modules();
        }
        if let Some(app) = self.app.as_mut() {
            app.content_applied(report);
        }
        self.next_update = Instant::now();
    }

    /// React to a failure the renderer reported. A lost device gets a new
    /// renderer — the app re-applies its graphics settings and re-queues the
    /// world's terrain for upload; running out of GPU memory, or failing to
    /// build the replacement, ends the game with the error.
    fn recover_renderer(&mut self, event_loop: &ActiveEventLoop, failure: RenderFailure) {
        let RenderFailure::DeviceLost(reason) = &failure else {
            fatal_render_error(event_loop, &failure);
            return;
        };
        log::warn!("GPU device lost ({reason}); rebuilding the renderer");
        let (Some(window), Some(app)) = (self.window.clone(), self.app.as_mut()) else {
            return;
        };
        // The dead renderer's surface must release the window before a new
        // surface is created on it.
        self.renderer = None;
        let size = window.inner_size();
        match pollster::block_on(new_renderer_from_target(window, size.width, size.height)) {
            Ok(mut renderer) => {
                app.renderer_recreated(&mut renderer);
                self.renderer = Some(renderer);
                self.next_update = Instant::now();
            }
            Err(e) => fatal_render_error(event_loop, &e),
        }
    }
}

fn modifiers_after_key_event(
    mut modifiers: Modifiers,
    code: KeyCode,
    down: bool,
) -> Option<Modifiers> {
    match code {
        KeyCode::ControlLeft | KeyCode::ControlRight => modifiers.ctrl = down,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => modifiers.shift = down,
        KeyCode::AltLeft | KeyCode::AltRight => modifiers.alt = down,
        KeyCode::SuperLeft | KeyCode::SuperRight => modifiers.meta = down,
        _ => return None,
    }
    Some(modifiers)
}

fn apply_cursor_policy(window: &Window, applied: &mut Option<CursorPolicy>, cursor: CursorPolicy) {
    if *applied == Some(cursor) {
        return;
    }
    if applied.is_none_or(|p| p.grabbed != cursor.grabbed) {
        let size = window.inner_size();
        let center = PhysicalPosition::new(size.width as f64 * 0.5, size.height as f64 * 0.5);
        if cursor.grabbed {
            // macOS re-associates cursor motion when warping, so centre before
            // locking there. Wayland rejects this until locked and uses the
            // retry below as its locked-pointer position hint.
            let centered_before_grab = window.set_cursor_position(center).is_ok();
            // Locked keeps the hidden OS cursor from drifting while raw relative
            // motion drives the camera. X11 cannot lock, so confine there.
            let _ = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            if !centered_before_grab {
                let _ = window.set_cursor_position(center);
            }
        } else {
            // Set the unlock position while Wayland's locked-pointer protocol
            // still permits it, so the visible menu cursor opens in the centre.
            if applied.is_some_and(|previous| previous.grabbed) {
                let _ = window.set_cursor_position(center);
            }
            let _ = window.set_cursor_grab(CursorGrabMode::None);
        }
    }
    if applied.is_none_or(|p| p.visible != cursor.visible) {
        window.set_cursor_visible(cursor.visible);
    }
    if applied.is_none_or(|p| p.icon != cursor.icon) {
        window.set_cursor(match cursor.icon {
            AppCursorIcon::Default => WinitCursorIcon::Default,
        });
    }
    *applied = Some(cursor);
}

impl ApplicationHandler for NativeHost {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Petramond")
            .with_inner_size(PhysicalSize::new(1280, 720));
        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        let size = window.inner_size();
        let mut renderer = match pollster::block_on(new_renderer_from_target(
            window.clone(),
            size.width,
            size.height,
        )) {
            Ok(renderer) => renderer,
            Err(e) => {
                fatal_render_error(event_loop, &e);
                return;
            }
        };
        let cam = Camera::new(
            petramond_math::world_pos::WorldPos::new(8.0, 90.0, 8.0),
            size.width as f32 / size.height.max(1) as f32,
        );
        App::publish_device_frame_limits(&renderer);
        crate::folder_picker::install();
        let mut app = App::new(cam, self.render_dist);
        if let Some(report) = self.content_report.take() {
            app.set_content_report(report);
        }
        // Graphics settings reach the renderer through the same call every
        // later options change uses; there is no creation-time path.
        app.apply_graphics(&mut renderer);
        if let Ok(world_name) = std::env::var("PETRAMOND_WORLD") {
            if !world_name.is_empty() {
                app.start_game(&world_name, self.seed);
            }
        }

        self.window = Some(window);
        self.renderer = Some(renderer);
        self.app = Some(app);
        apply_cursor_policy(
            self.window.as_ref().unwrap(),
            &mut self.cursor_policy,
            self.app.as_ref().unwrap().cursor_policy(),
        );
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
                app.request_exit(ExitKind::Quit);
                if app.take_quit_requested() {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                renderer.resize(size.width, size.height);
                app.resize(size.width, size.height);
                self.next_update = Instant::now();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = window.inner_size();
                renderer.resize(size.width, size.height);
                app.resize(size.width, size.height);
                self.next_update = Instant::now();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let KeyEvent {
                    physical_key,
                    state,
                    logical_key,
                    text,
                    ..
                } = event;
                let down = state == ElementState::Pressed;
                if let PhysicalKey::Code(winit_code) = physical_key {
                    let Some(code) = key_code(winit_code) else {
                        return;
                    };
                    if let Some(modifiers) =
                        modifiers_after_key_event(self.modifiers, winit_code, down)
                    {
                        self.modifiers = modifiers;
                        app.set_modifiers(modifiers);
                    }
                    // Remap capture (Options → Controls) preempts everything:
                    // the pressed key BECOMES the binding (ESC cancels), so it
                    // must never double as text/navigation/control input.
                    if app.remap_capture_key(code, down) {
                        if app.take_quit_requested() {
                            event_loop.exit();
                        }
                        return;
                    }
                }
                if down {
                    let mut handled_shortcut = false;
                    if let PhysicalKey::Code(winit_code) = physical_key {
                        let Some(code) = key_code(winit_code) else {
                            return;
                        };
                        handled_shortcut = app.handle_text_shortcut_code(code);
                    }
                    if !handled_shortcut {
                        if let Key::Named(named) = &logical_key {
                            if let Some(key) = text_key_from_named(named) {
                                app.handle_text_key(key);
                            }
                        }
                        if !app.take_quit_requested() {
                            if let Some(text) = text.as_ref() {
                                if !app.handle_text_input(text.as_str()) {
                                    // Text entry is opportunistic; non-text screens ignore it.
                                }
                            }
                        }
                    }
                }
                if let PhysicalKey::Code(winit_code) = physical_key {
                    let Some(code) = key_code(winit_code) else {
                        return;
                    };
                    // The app resolves raw keys through the player's binding
                    // table (engine + mod actions, fixed fallback keys
                    // included). `false` = an unconsumed CloseScreen press —
                    // quit, as always.
                    if !app.handle_raw_key(code, down) {
                        app.request_exit(ExitKind::Quit);
                    }
                }
                if app.take_quit_requested() {
                    event_loop.exit();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                // Physical Ctrl/Shift state, tracked apart from the rebindable
                // Sprint/Sneak controls so UI modifiers don't follow a rebind.
                let state = mods.state();
                self.modifiers = Modifiers {
                    ctrl: state.control_key(),
                    shift: state.shift_key(),
                    alt: state.alt_key(),
                    meta: state.super_key(),
                };
                app.set_modifiers(self.modifiers);
            }
            WindowEvent::Focused(false) => {
                self.modifiers = Modifiers::default();
                app.set_modifiers(self.modifiers);
                app.release_client_mod_keys();
                app.release_pointer_buttons();
                app.release_input_bindings();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                app.add_scroll_delta(-wheel_notches(delta));
            }
            WindowEvent::CursorMoved { position, .. } => {
                app.set_cursor_position(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // EVERY button forwards raw — side buttons are bindable; the
                // app routes left/right to the UI on menu screens itself.
                app.handle_raw_mouse(mouse_button(button), state == ElementState::Pressed);
            }
            WindowEvent::RedrawRequested => {
                apply_cursor_policy(window, &mut self.cursor_policy, app.cursor_policy());
                let refresh = window
                    .current_monitor()
                    .and_then(|monitor| monitor.refresh_rate_millihertz());
                if refresh != self.refresh_mhz {
                    if let Some(mhz) = refresh {
                        app.set_display_refresh_hz(f64::from(mhz) / 1000.0);
                    }
                    self.refresh_mhz = refresh;
                }
                // The host requests this once per `App::update`; the simulation itself
                // advances in `about_to_wait`.
                let render_start = Instant::now();
                if !app.render(renderer) {
                    self.next_update = Instant::now();
                }
                let failure = renderer.failure();
                if self.perf_log {
                    let dt = render_start.elapsed();
                    self.perf_render_total += dt;
                    self.perf_render_max = self.perf_render_max.max(dt);
                    self.perf_renders += 1;
                }
                if let Some(failure) = failure {
                    self.recover_renderer(event_loop, failure);
                }
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
                if self.cursor_policy.is_some_and(|cursor| cursor.grabbed) {
                    app.add_pointer_motion(dx as f32, dy as f32);
                }
            }
            DeviceEvent::MouseWheel { delta } => {
                app.add_scroll_delta(-wheel_notches(delta));
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self
            .app
            .as_mut()
            .is_some_and(App::take_content_apply_requested)
        {
            self.apply_content(event_loop);
            return;
        }
        let (Some(app), Some(renderer)) = (self.app.as_mut(), self.renderer.as_mut()) else {
            return;
        };
        let now = Instant::now();
        // Fixed frame-capped loop: every wake runs one `App::update` and draws it.
        // The SERVER thread owns the fixed-step accumulator holding the sim at
        // 20 TPS (`src/server/handle.rs`); updates here only exchange messages
        // with it. A released cursor means a modal screen is up — SHELL screens
        // take the cheaper menu cap, but a game menu (container/machine panel)
        // keeps the gameplay cadence: its bound state is answering the server,
        // and halving the frame rate taxes every round trip twice (input
        // sampling and drain-to-present).
        if now >= self.next_update {
            let update_start = Instant::now();
            app.update(renderer);
            if app.take_quit_requested() {
                event_loop.exit();
                return;
            }
            if self.perf_log {
                let dt = update_start.elapsed();
                self.perf_update_total += dt;
                self.perf_update_max = self.perf_update_max.max(dt);
            }
            self.perf_updates += 1;
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
            // A frame the stepped clock moves does not wait for the wall: its
            // capture may be due at once. A held frame keeps the normal cap.
            let frame = if app.frame_uncapped() {
                Duration::ZERO
            } else if app.cursor_policy().grabbed
                || app.game_menu_open()
                || app.client_canvas_screen()
            {
                self.frame
            } else {
                self.menu_frame
            };
            self.next_update = now + frame;
        }
        if self.perf_log && now.saturating_duration_since(self.perf_since).as_secs() >= 1 {
            let avg_update = avg_ms(self.perf_update_total, self.perf_updates);
            let avg_render = avg_ms(self.perf_render_total, self.perf_renders);
            eprintln!(
                "perf: {} updates/s, {} renders/s, update avg/max {:.2}/{:.2} ms, render avg/max {:.2}/{:.2} ms",
                self.perf_updates,
                self.perf_renders,
                avg_update,
                ms(self.perf_update_max),
                avg_render,
                ms(self.perf_render_max),
            );
            self.perf_updates = 0;
            self.perf_renders = 0;
            self.perf_update_total = Duration::ZERO;
            self.perf_update_max = Duration::ZERO;
            self.perf_render_total = Duration::ZERO;
            self.perf_render_max = Duration::ZERO;
            self.perf_since = now;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_update));
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

/// Report a renderer failure the game cannot continue past, and quit.
fn fatal_render_error(event_loop: &ActiveEventLoop, error: &dyn std::error::Error) {
    log::error!("renderer: {error}");
    eprintln!("Petramond cannot continue: {error}");
    event_loop.exit();
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

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn avg_ms(total: Duration, count: u32) -> f64 {
    if count == 0 {
        0.0
    } else {
        ms(total) / count as f64
    }
}
