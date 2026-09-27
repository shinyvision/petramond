use super::{App, AppScreen};
use petramond_input::controls::{
    fixed_control_from_key_code, is_modifier_key, ActionOut, BindMods, Binding, BoundInput,
    Control, ScrollDir,
};
use petramond_input::keycode::{KeyCode, MouseButton};
use petramond_world::gui_state::PointerButton;

impl App {
    pub fn handle_raw_key(&mut self, code: KeyCode, down: bool) -> bool {
        if down && self.ui.text_input_focused() && code != KeyCode::Escape {
            return true;
        }
        let mut out = Vec::new();
        self.resolve_input(BoundInput::Key(code), down, &mut out);
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

    pub fn handle_raw_mouse(&mut self, button: MouseButton, down: bool) {
        if self.options.remap().is_some() && self.remap_capture_mouse(button, down) {
            return;
        }
        let gameplay = self.screen.gameplay_enabled() && self.session.is_some();
        if gameplay || !down {
            let mut out = Vec::new();
            self.resolve_input(BoundInput::Mouse(button), down, &mut out);
            self.dispatch_actions(out);
            if gameplay {
                return;
            }
        }
        let pointer_button = match button {
            MouseButton::Left => Some(PointerButton::Primary),
            MouseButton::Right => Some(PointerButton::Secondary),
            _ => None,
        };
        if let Some(pb) = pointer_button {
            self.set_pointer_button(pb, down);
        }
    }

    pub(super) fn pulse_scroll_bindings(&mut self, notches: i32) {
        let dir = if notches > 0 {
            ScrollDir::Down
        } else {
            ScrollDir::Up
        };
        for _ in 0..notches.unsigned_abs() {
            let mut out = Vec::new();
            self.resolve_input(BoundInput::Scroll(dir), true, &mut out);
            self.resolve_input(BoundInput::Scroll(dir), false, &mut out);
            self.dispatch_actions(out);
        }
    }

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

    fn key_screen(&self) -> (bool, Option<String>) {
        let screen = match self.screen {
            AppScreen::ClientModGui(kind) => petramond_world::gui_state::kind_key(kind),
            _ => None,
        };
        let screen = screen
            .map(str::to_owned)
            .or_else(|| self.client_canvas_key().map(str::to_owned));
        (self.screen.gameplay_enabled(), screen)
    }

    fn resolve_input(&mut self, input: BoundInput, down: bool, out: &mut Vec<(ActionOut, bool)>) {
        let (gameplay, screen) = self.key_screen();
        let at = petramond::modding::client::keys::KeyContext {
            gameplay,
            screen: screen.as_deref(),
        };
        let mut engine = std::mem::take(&mut self.controls.binding_engine);
        let mods = self.client_mods_now();
        let live = |row: &petramond_input::controls::ActionRow| {
            !row.is_mod() || mods.is_some_and(|m| m.action_fires(&row.id, at))
        };
        engine.on_input(
            &self.controls.action_table,
            &self.options.settings.bindings,
            input,
            down,
            self.controls.modifiers,
            &live,
            out,
        );
        self.controls.binding_engine = engine;
    }

    fn dispatch_mod_action(&mut self, id: &str, pressed: bool) {
        if pressed && self.ui.text_input_focused() {
            return;
        }
        let (gameplay, screen) = self.key_screen();
        let at = petramond::modding::client::keys::KeyContext {
            gameplay,
            screen: screen.as_deref(),
        };
        if let Some(session) = self.session.as_mut() {
            session.game.client_mod_action(id, pressed, at);
        } else if let Some(runtime) = self.shell_mods_mut() {
            runtime.action(None, id, pressed, at);
        }
        self.apply_client_mod_commands();
    }

    pub(super) fn rebuild_action_table(&mut self) {
        self.release_input_bindings();
        let mut table = petramond_input::controls::ActionTable::engine();
        if let Some(runtime) = self.client_mods_now() {
            for action in runtime.key_actions() {
                table.push_registered_action(
                    action.full_id.clone(),
                    action.label.clone(),
                    action.category.clone(),
                    action.default,
                );
            }
        }
        self.controls.action_table = table;
        self.publish_key_labels();
    }

    pub(super) fn publish_key_labels(&mut self) {
        let table = &self.controls.action_table;
        let labels = table
            .rows()
            .iter()
            .filter(|row| row.id.contains(':'))
            .map(|row| {
                let binding = table.effective(&self.options.settings.bindings, row);
                (row.id.clone(), binding.label())
            })
            .collect();
        if let Some(runtime) = self.client_mods_now() {
            runtime.presented().lock().key_labels = labels;
        }
    }

    fn active_remap(&mut self) -> Option<String> {
        if self.screen != AppScreen::OptionsControls {
            self.options.cancel_remap();
            return None;
        }
        self.options.remap().map(str::to_owned)
    }

    /// Swallows all input while a remap is armed. ESC cancels and can't itself be bound. A modifier
    /// tap binds that modifier on release, a hold starts a chord, and any other key becomes the
    /// binding along with any held modifiers.
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
                self.finish_remap(
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
            self.finish_remap(
                &action,
                Binding {
                    mods: BindMods::from_modifiers(self.controls.modifiers),
                    input: BoundInput::Key(code),
                },
            );
        }
        true
    }

    fn remap_capture_mouse(&mut self, button: MouseButton, down: bool) -> bool {
        let Some(action) = self.active_remap() else {
            return false;
        };
        if !down {
            return true;
        }
        if self.cursor_over_interactive_widget() {
            return false;
        }
        self.finish_remap(
            &action,
            Binding {
                mods: BindMods::from_modifiers(self.controls.modifiers),
                input: BoundInput::Mouse(button),
            },
        );
        true
    }

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
        self.finish_remap(
            &action,
            Binding {
                mods: BindMods::from_modifiers(self.controls.modifiers),
                input: BoundInput::Scroll(dir),
            },
        );
    }

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

    fn finish_remap(&mut self, action_id: &str, binding: Binding) {
        self.options.finish_remap(action_id, binding);
        self.publish_key_labels();
    }

    pub(super) fn apply_volumes(&mut self) {
        let settings = &self.options.settings;
        self.sound.set_volumes(
            settings.master_volume,
            settings.sound_volume,
            settings.music_volume,
        );
    }

    pub(super) fn apply_particles(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session
                .game
                .set_particles_mode(self.options.settings.particles);
        }
        self.options.mark_renderer_dirty();
    }

    pub(super) fn apply_view_distance(&mut self, chunks: i32) {
        let chunks = self.options.set_view_distance(chunks);
        self.render_dist = chunks;
        if let Some(session) = self.session.as_mut() {
            session.game.set_view_distance(chunks);
        }
        self.options.persist();
    }

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

    pub(crate) fn renderer_recreated(&mut self, renderer: &mut petramond_render::Renderer) {
        self.options.mark_renderer_dirty();
        self.apply_graphics(renderer);
        if let Some(session) = self.session.as_mut() {
            session
                .game
                .terrain_render_handoff()
                .request_full_reupload();
        }
    }
}
