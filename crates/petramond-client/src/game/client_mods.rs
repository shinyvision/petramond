use super::Game;

impl Game {
    pub fn drive_client_mods(
        &mut self,
        mut frame: mod_api::ClientFrameData,
        presented_view: mod_api::ClientViewStateData,
    ) {
        frame.player_pos = self.local.player.pos.to_array();
        frame.yaw = self.local.player.yaw;
        frame.pitch = self.local.player.pitch;
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

    pub fn bake_client_custom_shapes(&mut self) {
        self.client_mods.bake_custom_shapes(&mut self.replica.world);
    }

    pub fn client_mod_action(
        &mut self,
        full_id: &str,
        pressed: bool,
        at: petramond::modding::client::keys::KeyContext<'_>,
    ) -> bool {
        self.client_mods
            .action(Some(&self.replica.world), full_id, pressed, at)
    }

    #[cfg(test)]
    pub(crate) fn client_call_for_test(
        &mut self,
        mod_id: &str,
        call: mod_api::HostCall,
    ) -> Option<mod_api::HostRet> {
        self.client_mods
            .call_as_for_test(mod_id, Some(&self.replica.world), call)
    }

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

    pub fn for_each_client_world_mark(
        &self,
        f: impl FnMut(&mod_api::ClientWorldMark, Option<&petramond::modding::ClientImageData>),
    ) {
        self.client_mods.for_each_world_mark(f);
    }

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

    pub fn client_mod_sound_loops(
        &self,
        out: &mut Vec<(petramond_world::sound_registry::Sound, f32)>,
    ) {
        self.client_mods.sound_loops(out);
    }

    pub fn client_mod_mood(&self) -> [f32; 2] {
        self.client_mods.mood()
    }
}
