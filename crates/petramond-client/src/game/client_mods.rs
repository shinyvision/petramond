//! App-facing plumbing for the session's client-mod runtime: per-frame
//! driving, bound-action/UI/canvas event dispatch into the owning mod, and
//! read access to published overlays, images, views, and queued commands.
//! Policy lives in [`petramond::modding::client`]; `Game` only threads the
//! replica through.

use super::Game;

impl Game {
    /// Drive the session's client mods for one frame: `frame` as the app
    /// assembled it, with this game's player filled in.
    pub fn drive_client_mods(
        &mut self,
        mut frame: mod_api::ClientFrameData,
        presented_view: mod_api::ClientViewStateData,
    ) {
        frame.player_pos = self.local.player.pos.to_array();
        frame.yaw = self.local.player.yaw;
        frame.pitch = self.local.player.pitch;
        // The per-frame hook gets the SAME actor snapshot the prediction
        // dispatches publish, so a mod's rule is one predicate that reads
        // `player_state()` wherever it runs — and knows which player it is
        // acting for, which is what lets it address the pose call.
        //
        // The swing facts ride with them: the one-shots latched since the
        // last hook (the SAME edges the animators play, on their own
        // latch — see `LocalHand::take_swing_events`), the mining level read live —
        // the exact shape of the server's roster build.
        let swing = mod_api::HandSwing {
            mining: self.replica.self_view.mining.is_some(),
            ..self.hand.take_swing_events()
        };
        let actor = self.client_actor_snapshot(self.local.predicted_input.sneak, swing);
        self.client_mods.presented().lock().local_player =
            Some(mod_api::PlayerId(self.replica.entities.self_id().0));
        self.client_mods.frame(
            &self.replica.world,
            &actor,
            &self.replica.self_view.inventory,
            frame,
            presented_view,
        );
    }

    /// Deliver the mod cues this batch carried for us (`EmitEventTo`) into
    /// their owning client mods, with the SAME actor snapshot the per-frame
    /// hook publishes — the cue is about this player, and its handler poses
    /// this player.
    ///
    /// Drained where the batch lands, not where the frame is assembled: the
    /// cue exists so a pack can start presenting something on the frame it
    /// hears about it.
    pub fn deliver_client_mod_events(
        &mut self,
        events: &[petramond::net::protocol::ClientEventMsg],
    ) {
        if events.is_empty() {
            return;
        }
        let actor =
            self.client_actor_snapshot(self.local.predicted_input.sneak, Default::default());
        for ev in events {
            self.client_mods.mod_event(
                &self.replica.world,
                &actor,
                &self.replica.self_view.inventory,
                &ev.key,
                &ev.data,
            );
        }
    }

    /// Bake the SIM geometry of any custom-shape cells the replica
    /// dirtied (server deltas ingested this frame) via their `client_wasm`, so
    /// the client's physics/prediction reads the same collision the server does.
    pub fn bake_client_custom_shapes(&mut self) {
        self.client_mods.bake_custom_shapes(&mut self.replica.world);
    }

    /// Dispatch a mod-registered bound action edge (`mod_id:action`) to its
    /// owning client mod.
    pub fn client_mod_action(
        &mut self,
        full_id: &str,
        pressed: bool,
        at: petramond::modding::client::keys::KeyContext<'_>,
    ) -> bool {
        self.client_mods
            .action(Some(&self.replica.world), full_id, pressed, at)
    }

    /// Issue `call` as client mod `mod_id` would, beside this replica.
    #[cfg(test)]
    pub(crate) fn client_call_for_test(
        &mut self,
        mod_id: &str,
        call: mod_api::HostCall,
    ) -> Option<mod_api::HostRet> {
        self.client_mods
            .call_as_for_test(mod_id, Some(&self.replica.world), call)
    }

    /// This session's client mods, for the reads the app shares with the
    /// shell's (the same questions asked of whichever runtime is current).
    pub fn client_mod_runtime(&self) -> &petramond::modding::client::ClientModRuntime {
        &self.client_mods
    }

    pub fn release_client_mod_keys(&mut self) {
        self.client_mods.release_all_keys(Some(&self.replica.world));
    }

    pub fn client_mod_ui_event(&mut self, kind_key: &str, event: mod_api::ClientUiEvent) {
        self.client_mods
            .ui_event(Some(&self.replica.world), kind_key, event);
    }

    pub fn client_mod_canvas_event(&mut self, canvas_key: &str, event: mod_api::ClientCanvasEvent) {
        self.client_mods
            .canvas_event(Some(&self.replica.world), canvas_key, event);
    }

    pub fn client_mod_canvas_scroll(&mut self, canvas_key: &str, x: f32, y: f32, delta: f32) {
        self.client_mods
            .canvas_scroll(Some(&self.replica.world), canvas_key, x, y, delta);
    }

    pub fn client_mod_overlays(&self) -> &[petramond::modding::ClientOverlayRegistration] {
        self.client_mods.overlays()
    }

    pub fn client_mod_image(&self, image_key: &str) -> Option<petramond::modding::ClientImageData> {
        self.client_mods.image(image_key)
    }

    /// Every client mod's world marks, in the order they draw (see
    /// `ClientModRuntime::for_each_world_mark`).
    pub fn for_each_client_world_mark(
        &self,
        f: impl FnMut(&mod_api::ClientWorldMark, Option<&petramond::modding::ClientImageData>),
    ) {
        self.client_mods.for_each_world_mark(f);
    }

    /// Stand in for client mod `mod_id`'s own `ClientWorldMarksSet`.
    #[cfg(test)]
    pub(crate) fn set_client_world_marks_for_test(
        &mut self,
        mod_id: &str,
        marks: Vec<mod_api::ClientWorldMark>,
    ) -> bool {
        self.client_mods.set_world_marks_for_test(mod_id, marks)
    }

    pub fn client_mod_canvas_view(
        &self,
        canvas_key: &str,
    ) -> Option<petramond::modding::client::ClientCanvasView> {
        self.client_mods.canvas_view(canvas_key)
    }

    pub fn client_mod_view(
        &self,
        kind_key: &str,
    ) -> Option<petramond::modding::client::ClientUiView> {
        self.client_mods.view_for(kind_key)
    }

    pub fn take_client_mod_commands(&mut self) -> Vec<petramond::modding::ClientCommand> {
        self.client_mods.take_commands()
    }

    /// Every client mod's desired looping-sound gains this frame.
    pub fn client_mod_sound_loops(
        &self,
        out: &mut Vec<(petramond_world::sound_registry::Sound, f32)>,
    ) {
        self.client_mods.sound_loops(out);
    }

    /// The combined client-mod post mood `[darken, desaturate]`.
    pub fn client_mod_mood(&self) -> [f32; 2] {
        self.client_mods.mood()
    }
}
