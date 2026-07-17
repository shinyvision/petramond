//! App-facing plumbing for the session's client-mod runtime: per-frame
//! driving, bound-action/UI/canvas event dispatch into the owning mod, and
//! read access to published overlays, images, views, and queued commands.
//! Policy lives in [`crate::modding::client`]; `Game` only threads the
//! replica through.

use super::Game;

impl Game {
    pub(crate) fn drive_client_mods(
        &mut self,
        dt: f32,
        screen: (u32, u32),
        open_gui: Option<&str>,
        open_canvas: Option<&str>,
    ) {
        let frame = mod_api::ClientFrameData {
            dt: dt.max(0.0),
            player_pos: self.player.pos.to_array(),
            yaw: self.player.yaw,
            pitch: self.player.pitch,
            screen: [screen.0, screen.1],
            open_gui: open_gui.map(str::to_owned),
            open_canvas: open_canvas.map(str::to_owned),
        };
        self.client_mods.frame(&self.replica, frame);
    }

    /// Dispatch a mod-registered bound action edge (`mod_id:action`) to its
    /// owning client mod.
    pub(crate) fn client_mod_action(&mut self, full_id: &str, pressed: bool) -> bool {
        self.client_mods.action(&self.replica, full_id, pressed)
    }

    /// The session's mod-registered remappable key actions, for the app's
    /// action table: `(full_id, label, category, default binding)`.
    pub(crate) fn client_bindable_actions(
        &self,
    ) -> Vec<(String, String, String, crate::controls::Binding)> {
        self.client_mods
            .key_actions()
            .iter()
            .map(|a| {
                (
                    a.full_id.clone(),
                    a.label.clone(),
                    a.category.clone(),
                    crate::controls::Binding::key(a.default_code),
                )
            })
            .collect()
    }

    pub(crate) fn release_client_mod_keys(&mut self) {
        self.client_mods.release_all_keys(&self.replica);
    }

    pub(crate) fn client_mod_ui_event(&mut self, kind_key: &str, event: mod_api::ClientUiEvent) {
        self.client_mods.ui_event(&self.replica, kind_key, event);
    }

    pub(crate) fn client_mod_canvas_event(
        &mut self,
        canvas_key: &str,
        event: mod_api::ClientCanvasEvent,
    ) {
        self.client_mods
            .canvas_event(&self.replica, canvas_key, event);
    }

    pub(crate) fn client_mod_canvas_scroll(
        &mut self,
        canvas_key: &str,
        x: f32,
        y: f32,
        delta: f32,
    ) {
        self.client_mods
            .canvas_scroll(&self.replica, canvas_key, x, y, delta);
    }

    pub(crate) fn client_mod_overlays(&self) -> &[crate::modding::ClientOverlayRegistration] {
        self.client_mods.overlays()
    }

    pub(crate) fn client_mod_image(
        &self,
        image_key: &str,
    ) -> Option<crate::modding::ClientImageData> {
        self.client_mods.image(image_key)
    }

    pub(crate) fn client_mod_canvas_view(
        &self,
        canvas_key: &str,
    ) -> Option<crate::modding::client::ClientCanvasView> {
        self.client_mods.canvas_view(canvas_key)
    }

    pub(crate) fn client_mod_view(
        &self,
        kind_key: &str,
    ) -> Option<crate::modding::client::ClientUiView> {
        self.client_mods.view_for(kind_key)
    }

    pub(crate) fn take_client_mod_commands(&mut self) -> Vec<crate::modding::ClientCommand> {
        self.client_mods.take_commands()
    }

    /// Every client mod's desired looping-sound gains this frame.
    pub(crate) fn client_mod_sound_loops(&self, out: &mut Vec<(crate::audio::Sound, f32)>) {
        self.client_mods.sound_loops(out);
    }

    /// The combined client-mod post mood `[darken, desaturate]`.
    pub(crate) fn client_mod_mood(&self) -> [f32; 2] {
        self.client_mods.mood()
    }
}
