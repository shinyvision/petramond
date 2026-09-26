//! The Options flow's state: the persistent per-machine settings
//! (`client.json`), slider previews, the armed control remap, and whether
//! the renderer owes a settings refresh. Where the flow returns to is the
//! screen stack's business (the flow is pushed over the title or the pause
//! menu).
//! Screen controllers edit this directly; side effects outside it (the audio
//! mixer, the live game, the render distance of the next session) are the
//! App's, requested through shell commands.

use petramond::save::client::{AntiAliasing, ClientSettings};
use petramond_input::controls::Binding;
use petramond_input::keycode::KeyCode;

pub(super) struct OptionsState {
    /// Persistent per-machine settings (`client.json`): volumes, particles,
    /// key bindings. Every committed Options change stores the file.
    pub(super) settings: ClientSettings,
    /// Renderer-owned option values (fog/render distance, particle density)
    /// changed and must be pushed on the next render.
    renderer_dirty: bool,
    /// Slider positions mid-drag, shown by the Graphics readouts but not yet
    /// applied: applying per drag step would reallocate scene targets and
    /// reshape streaming on every pixel of travel.
    pub(super) anti_aliasing_preview: Option<AntiAliasing>,
    pub(super) view_distance_preview: Option<i32>,
    /// The action ID armed for remapping on the Options → Controls screen
    /// (`None` = not remapping; engine ids like `jump`, mod ids like
    /// `minimap:open_map`). While set, raw input is CAPTURED as the new
    /// binding instead of dispatching; ESC cancels.
    remap: Option<String>,
    /// The modifier key held down while remapping (a chord starter). If it
    /// releases with nothing else captured, the tap binds the modifier itself.
    remap_armed_mod: Option<KeyCode>,
}

impl OptionsState {
    pub(super) fn new(settings: ClientSettings) -> Self {
        Self {
            settings,
            renderer_dirty: true,
            anti_aliasing_preview: None,
            view_distance_preview: None,
            remap: None,
            remap_armed_mod: None,
        }
    }

    /// Write `client.json`. Suppressed under test (the suite must never
    /// rewrite the developer's real file). Merges into the current file so
    /// knobs the GUI doesn't own (fps caps, render scale, grade, identity)
    /// keep whatever the file says.
    pub(super) fn persist(&self) {
        if cfg!(test) {
            return;
        }
        let mut on_disk = petramond::save::client::load();
        on_disk.render_dist = self.settings.render_dist;
        on_disk.master_volume = self.settings.master_volume;
        on_disk.sound_volume = self.settings.sound_volume;
        on_disk.music_volume = self.settings.music_volume;
        on_disk.particles = self.settings.particles;
        on_disk.screen_shake = self.settings.screen_shake;
        on_disk.anti_aliasing = self.settings.anti_aliasing;
        on_disk.bindings = self.settings.bindings.clone();
        if let Err(e) = petramond::save::client::store(&on_disk) {
            log::warn!("could not write client.json: {e}");
        }
    }

    /// The renderer owes a settings refresh.
    pub(super) fn mark_renderer_dirty(&mut self) {
        self.renderer_dirty = true;
    }

    /// Take the renderer refresh flag.
    pub(super) fn take_renderer_dirty(&mut self) -> bool {
        std::mem::take(&mut self.renderer_dirty)
    }

    /// Leaving a category screen drops its unapplied slider previews.
    pub(super) fn clear_previews(&mut self) {
        self.anti_aliasing_preview = None;
        self.view_distance_preview = None;
    }

    /// Screen shake on or off: the renderer's half (camera bone, hand
    /// jitter) on the next render; the hurt jitter reads the setting every
    /// frame.
    pub(super) fn set_screen_shake(&mut self, on: bool) {
        if self.settings.screen_shake != on {
            self.settings.screen_shake = on;
            self.renderer_dirty = true;
        }
    }

    pub(super) fn set_anti_aliasing(&mut self, mode: AntiAliasing) {
        self.anti_aliasing_preview = None;
        if self.settings.anti_aliasing != mode {
            self.settings.anti_aliasing = mode;
            self.renderer_dirty = true;
        }
    }

    /// Commit a view distance (clamped to 4..=64 chunks); returns the value
    /// the live session must adopt.
    pub(super) fn set_view_distance(&mut self, chunks: i32) -> i32 {
        let chunks = chunks.clamp(4, 64);
        self.view_distance_preview = None;
        self.settings.render_dist = chunks;
        self.renderer_dirty = true;
        chunks
    }

    /// The armed remap's action id.
    pub(super) fn remap(&self) -> Option<&str> {
        self.remap.as_deref()
    }

    /// Arm the action id for remapping (clicking another action's button
    /// while one is armed switches — the previous remap cancels, per design).
    pub(super) fn begin_remap(&mut self, action_id: &str) {
        self.remap = Some(action_id.to_string());
        self.remap_armed_mod = None;
    }

    pub(super) fn cancel_remap(&mut self) {
        self.remap = None;
        self.remap_armed_mod = None;
    }

    /// The modifier held down while remapping, if any.
    pub(super) fn armed_mod(&self) -> Option<KeyCode> {
        self.remap_armed_mod
    }

    pub(super) fn arm_mod(&mut self, code: KeyCode) {
        self.remap_armed_mod = Some(code);
    }

    /// Bind `binding` to `action_id`, disarm, and persist.
    pub(super) fn finish_remap(&mut self, action_id: &str, binding: Binding) {
        self.settings.bindings.set_id(action_id, binding);
        self.cancel_remap();
        self.persist();
    }

    #[cfg(test)]
    pub(super) fn renderer_dirty(&self) -> bool {
        self.renderer_dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_graphics_value_dirties_the_renderer_once() {
        let mut options = OptionsState::new(ClientSettings::default());
        assert!(options.take_renderer_dirty(), "a fresh app pushes its settings");
        assert!(!options.take_renderer_dirty());
        let shake = options.settings.screen_shake;
        options.set_screen_shake(shake);
        assert!(!options.renderer_dirty(), "an unchanged value is not a change");
        options.set_screen_shake(!shake);
        assert!(options.take_renderer_dirty());
    }

    #[test]
    fn committing_a_view_distance_clamps_and_drops_the_preview() {
        let mut options = OptionsState::new(ClientSettings::default());
        options.view_distance_preview = Some(12);
        assert_eq!(options.set_view_distance(100), 64);
        assert_eq!(options.settings.render_dist, 64);
        assert_eq!(options.view_distance_preview, None);
    }

    #[test]
    fn remap_arms_one_action_at_a_time() {
        let mut options = OptionsState::new(ClientSettings::default());
        options.begin_remap("jump");
        options.arm_mod(KeyCode::ShiftLeft);
        options.begin_remap("sneak");
        assert_eq!(options.remap(), Some("sneak"));
        assert_eq!(options.armed_mod(), None, "switching drops the chord starter");
        options.cancel_remap();
        assert_eq!(options.remap(), None);
    }
}
