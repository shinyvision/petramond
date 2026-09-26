//! Options state and raw-input resolution: the rebindable-controls engine's
//! app seam (raw key/mouse/scroll → [`Control`] edges through the player's
//! [`BindingSet`]), remap capture for the Options → Controls screen, and the
//! apply/persist paths for every Options value (volumes, particles, view
//! distance). Screen controllers live in `shell_docs/options*.rs`; this file
//! owns the behavior they call into.

use super::{App, AppScreen};
use petramond_input::controls::{
    fixed_control_from_key_code, is_modifier_key, ActionOut, BindMods, Binding, BoundInput,
    Control, ScrollDir,
};
use petramond_input::keycode::{KeyCode, MouseButton};
use petramond_world::gui_state::PointerButton;

impl App {
    /// Resolve a raw keyboard event through the binding table (fixed fallback
    /// keys when nothing matched). Returns `false` only for an unconsumed
    /// CloseScreen press — the host's quit signal, exactly like the old
    /// hardcoded translation.
    pub fn handle_raw_key(&mut self, code: KeyCode, down: bool) -> bool {
        // Text entry owns key presses while focused. Releases still flow
        // through the binding engine so an action held before focus cannot
        // stick; Escape remains the explicit close-screen control.
        if down && self.ui.text_input_focused() && code != KeyCode::Escape {
            return true;
        }
        let mut out = Vec::new();
        self.controls.binding_engine.on_input(
            &self.controls.action_table,
            &self.options.settings.bindings,
            BoundInput::Key(code),
            down,
            self.controls.modifiers,
            &mut out,
        );
        if !out.is_empty() {
            self.dispatch_actions(out);
            return true;
        }
        let Some(control) = fixed_control_from_key_code(code) else {
            return true;
        };
        let consumed = self.handle_control(control, down);
        consumed || !(matches!(control, Control::CloseScreen) && down)
    }

    /// Resolve a raw mouse-button event: capture for remap, bindings in
    /// gameplay, the classic Primary/Secondary UI routing everywhere else.
    /// Releases ALWAYS run through the binding engine so a held bound action
    /// can never stick across a screen change.
    pub fn handle_raw_mouse(&mut self, button: MouseButton, down: bool) {
        if self.options.remap().is_some() && self.remap_capture_mouse(button, down) {
            return;
        }
        let gameplay = self.screen.gameplay_enabled() && self.session.is_some();
        if gameplay || !down {
            let mut out = Vec::new();
            self.controls.binding_engine.on_input(
                &self.controls.action_table,
                &self.options.settings.bindings,
                BoundInput::Mouse(button),
                down,
                self.controls.modifiers,
                &mut out,
            );
            self.dispatch_actions(out);
            if gameplay {
                return;
            }
        }
        // Menus/chat/canvas: the physical left/right buttons drive the UI,
        // whatever the bindings say.
        let pointer_button = match button {
            MouseButton::Left => Some(PointerButton::Primary),
            MouseButton::Right => Some(PointerButton::Secondary),
            _ => None,
        };
        if let Some(pb) = pointer_button {
            self.set_pointer_button(pb, down);
        }
    }

    /// Fire the bindings bound to whole scroll notches (positive = down).
    /// Each notch is a full press+release pulse: hotbar next/prev by default,
    /// any other action a player scroll-binds behaves like a tap per notch.
    pub(super) fn pulse_scroll_bindings(&mut self, notches: i32) {
        let dir = if notches > 0 {
            ScrollDir::Down
        } else {
            ScrollDir::Up
        };
        for _ in 0..notches.unsigned_abs() {
            let mut out = Vec::new();
            self.controls.binding_engine.on_input(
                &self.controls.action_table,
                &self.options.settings.bindings,
                BoundInput::Scroll(dir),
                true,
                self.controls.modifiers,
                &mut out,
            );
            self.controls.binding_engine.on_input(
                &self.controls.action_table,
                &self.options.settings.bindings,
                BoundInput::Scroll(dir),
                false,
                self.controls.modifiers,
                &mut out,
            );
            self.dispatch_actions(out);
        }
    }

    /// Release every held bound action (window focus loss, session teardown).
    pub fn release_input_bindings(&mut self) {
        let mut out = Vec::new();
        self.controls.binding_engine.release_all(&mut out);
        self.dispatch_actions(out);
    }

    pub(super) fn dispatch_actions(&mut self, out: Vec<(ActionOut, bool)>) {
        for (action, down) in out {
            match action {
                ActionOut::Control(control) => {
                    self.handle_control(control, down);
                }
                ActionOut::ClientMod(id) => self.dispatch_mod_action(&id, down),
            }
        }
    }

    /// Dispatch a mod-registered bound action to its owning client mod, with
    /// the same gating the legacy physical-key path had: presses only reach
    /// mods in gameplay/client-GUI contexts and never over a focused text
    /// input; releases always land so the mod's edge filter can't latch.
    fn dispatch_mod_action(&mut self, id: &str, pressed: bool) {
        if !super::client_mod_ui::client_key_dispatch_permitted(
            pressed,
            self.screen,
            self.ui.text_input_focused(),
        ) {
            return;
        }
        if let Some(session) = self.session.as_mut() {
            session.game.client_mod_action(id, pressed);
        }
        self.apply_client_mod_commands();
    }

    /// Rebuild the remappable-action table for the current session: engine
    /// actions plus whatever the loaded client mods registered. Held bindings
    /// release first — an action must not stay down across the swap.
    pub(super) fn rebuild_action_table(&mut self) {
        self.release_input_bindings();
        let mut table = petramond_input::controls::ActionTable::engine();
        if let Some(session) = self.session.as_ref() {
            for (id, label, category, default) in session.game.client_bindable_actions() {
                table.push_registered_action(id, label, category, default);
            }
        }
        self.controls.action_table = table;
    }

    // --- Remap capture (Options → Controls) ---

    /// The armed remap, IF the controls screen is still the one open. Any
    /// other screen (a connection loss can swap screens underneath) disarms it
    /// so capture never eats input elsewhere.
    fn active_remap(&mut self) -> Option<String> {
        if self.screen != AppScreen::OptionsControls {
            self.options.cancel_remap();
            return None;
        }
        self.options.remap().map(str::to_owned)
    }

    /// Capture a raw KEY for the armed remap. Consumes everything while
    /// remapping: ESC cancels (the one unbindable key); a modifier tap binds
    /// that modifier on its release, a modifier HOLD starts a chord; any other
    /// key (plus held modifiers) becomes the binding.
    pub fn remap_capture_key(&mut self, code: KeyCode, down: bool) -> bool {
        let Some(action) = self.active_remap() else {
            return false;
        };
        if code == KeyCode::Escape {
            if down {
                self.options.cancel_remap();
            }
            return true;
        }
        if is_modifier_key(code) {
            if down {
                self.options.arm_mod(code);
            } else if self.options.armed_mod() == Some(code) {
                // Tap-released with nothing else captured: bind the bare
                // modifier (any OTHER still-held modifiers chord it).
                self.options.finish_remap(
                    &action,
                    Binding {
                        mods: BindMods::from_modifiers(self.controls.modifiers),
                        input: BoundInput::Key(code),
                    },
                );
            }
            return true;
        }
        if down {
            self.options.finish_remap(
                &action,
                Binding {
                    mods: BindMods::from_modifiers(self.controls.modifiers),
                    input: BoundInput::Key(code),
                },
            );
        }
        true
    }

    /// Capture a raw MOUSE BUTTON for the armed remap — unless the press is
    /// over another widget of the controls document (a different action's
    /// button, Back): those route to the UI, which switches or cancels the
    /// remap instead of binding a click. Returns whether the event was
    /// consumed as capture.
    fn remap_capture_mouse(&mut self, button: MouseButton, down: bool) -> bool {
        let Some(action) = self.active_remap() else {
            return false;
        };
        if !down {
            // The release after a capture (or over a widget) is nobody's
            // press; swallow it so the UI never sees an unmatched up.
            return true;
        }
        if self.cursor_over_interactive_widget() {
            // Route to the UI: clicking another action's button cancels this
            // remap and arms that one; Back cancels and leaves.
            return false;
        }
        self.options.finish_remap(
            &action,
            Binding {
                mods: BindMods::from_modifiers(self.controls.modifiers),
                input: BoundInput::Mouse(button),
            },
        );
        true
    }

    /// Capture a SCROLL direction for the armed remap (any wheel movement
    /// while remapping binds its direction).
    pub(super) fn remap_capture_scroll(&mut self, delta: f32) {
        let Some(action) = self.active_remap() else {
            return;
        };
        if delta == 0.0 {
            return;
        }
        let dir = if delta > 0.0 {
            ScrollDir::Down
        } else {
            ScrollDir::Up
        };
        self.options.finish_remap(
            &action,
            Binding {
                mods: BindMods::from_modifiers(self.controls.modifiers),
                input: BoundInput::Scroll(dir),
            },
        );
    }

    /// Whether the cursor is over one of the controls document's widgets
    /// (bind buttons / Back) — presses there are UI clicks, not capture.
    fn cursor_over_interactive_widget(&self) -> bool {
        let (x, y) = self.controls.pointer.cursor();
        self.ui.out().named.iter().any(|(key, rect)| {
            (key.id == "back" || key.id == "bind")
                && x >= rect.x as f32
                && x < (rect.x + rect.w) as f32
                && y >= rect.y as f32
                && y < (rect.y + rect.h) as f32
        })
    }

    // --- Apply: the side effects an option has outside `OptionsState` ---

    /// Push the current volume settings into the audio engine (live).
    pub(super) fn apply_volumes(&mut self) {
        let settings = &self.options.settings;
        self.sound.set_volumes(
            settings.master_volume,
            settings.sound_volume,
            settings.music_volume,
        );
    }

    /// Apply the particles mode to both presentation halves: the game-side
    /// fleck system now, the renderer's emitter density on the next render.
    pub(super) fn apply_particles(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.game.set_particles_mode(self.options.settings.particles);
        }
        self.options.mark_renderer_dirty();
    }

    /// Apply and persist a committed view distance: replica + server
    /// streaming through the game session, fog/cull on the next render, and
    /// the render distance every future session start reads.
    pub(super) fn apply_view_distance(&mut self, chunks: i32) {
        let chunks = self.options.set_view_distance(chunks);
        self.render_dist = chunks;
        if let Some(session) = self.session.as_mut() {
            session.game.set_view_distance(chunks);
        }
        self.options.persist();
    }

    /// Hand the renderer every graphics setting it owns, when one changed.
    /// The host calls this once the window's renderer exists and `App::render`
    /// calls it every frame with the renderer in hand — the same call, so a
    /// setting cannot be applied at creation and then forgotten. The renderer
    /// answers with the anti-aliasing mode it can actually run; a fallback is
    /// written back and persisted so the options readout shows what runs.
    pub(crate) fn apply_graphics(&mut self, renderer: &mut petramond_render::Renderer) {
        if !self.options.take_renderer_dirty()
            && self.options.settings.anti_aliasing == renderer.anti_aliasing()
        {
            return;
        }
        let applied = renderer.apply_graphics(&self.options.settings.graphics());
        if applied != self.options.settings.anti_aliasing {
            self.options.settings.anti_aliasing = applied;
            self.options.persist();
        }
    }

    /// The host replaced a renderer whose GPU device was lost. The new one
    /// knows no settings and holds no terrain: hand it the graphics settings
    /// and queue every meshed column of the world for upload again.
    pub(crate) fn renderer_recreated(&mut self, renderer: &mut petramond_render::Renderer) {
        self.renderer_options_dirty = true;
        self.apply_graphics(renderer);
        if let Some(game) = self.game.as_mut() {
            game.terrain_render_handoff().request_full_reupload();
        }
    }
}
