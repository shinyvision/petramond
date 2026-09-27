use petramond::save::client::{AntiAliasing, ClientSettings};
use petramond_input::controls::Binding;
use petramond_input::keycode::KeyCode;

pub(super) struct OptionsState {
    pub(super) settings: ClientSettings,
    renderer_dirty: bool,
    pub(super) anti_aliasing_preview: Option<AntiAliasing>,
    pub(super) view_distance_preview: Option<i32>,
    remap: Option<String>,
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

    pub(super) fn mark_renderer_dirty(&mut self) {
        self.renderer_dirty = true;
    }

    pub(super) fn take_renderer_dirty(&mut self) -> bool {
        std::mem::take(&mut self.renderer_dirty)
    }

    pub(super) fn clear_previews(&mut self) {
        self.anti_aliasing_preview = None;
        self.view_distance_preview = None;
    }

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

    pub(super) fn set_view_distance(&mut self, chunks: i32) -> i32 {
        let chunks = chunks.clamp(4, 64);
        self.view_distance_preview = None;
        self.settings.render_dist = chunks;
        self.renderer_dirty = true;
        chunks
    }

    pub(super) fn remap(&self) -> Option<&str> {
        self.remap.as_deref()
    }

    pub(super) fn begin_remap(&mut self, action_id: &str) {
        self.remap = Some(action_id.to_string());
        self.remap_armed_mod = None;
    }

    pub(super) fn cancel_remap(&mut self) {
        self.remap = None;
        self.remap_armed_mod = None;
    }

    pub(super) fn armed_mod(&self) -> Option<KeyCode> {
        self.remap_armed_mod
    }

    pub(super) fn arm_mod(&mut self, code: KeyCode) {
        self.remap_armed_mod = Some(code);
    }

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
        assert!(
            options.take_renderer_dirty(),
            "a fresh app pushes its settings"
        );
        assert!(!options.take_renderer_dirty());
        let shake = options.settings.screen_shake;
        options.set_screen_shake(shake);
        assert!(
            !options.renderer_dirty(),
            "an unchanged value is not a change"
        );
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
        assert_eq!(
            options.armed_mod(),
            None,
            "switching drops the chord starter"
        );
        options.cancel_remap();
        assert_eq!(options.remap(), None);
    }
}
