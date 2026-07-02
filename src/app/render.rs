use super::{now_seconds, ui_snapshot, App};
use crate::render::{HeldItemFrame, Renderer};

impl App {
    /// Draw the current frame. The host calls this once per [`update`](Self::update);
    /// the simulation tick itself runs inside `update`, not here.
    pub fn render(&mut self, renderer: &mut Renderer) {
        let now = now_seconds();
        // The hand animation advances by render time (not sim time); clamp so a long
        // idle gap before the first active frame can't jump a swing mid-flight.
        let dt = ((now - self.last_render) as f32).clamp(0.0, 0.1);
        self.last_render = now;
        let screen_size = renderer.screen_size();

        if self.renderer_world_clear_pending {
            renderer.clear_world_state();
            self.renderer_world_clear_pending = false;
        }

        let shell = self.shell_ui_snapshot(screen_size, self.pointer.cursor());

        let Some(game) = self.game.as_mut() else {
            renderer.set_crosshair_visible(false);
            renderer.set_hand_visible(false);
            renderer.update_uniforms(&self.shell_camera, [0.60, 0.82, 1.00], now as f32, false);
            renderer.set_ui(ui_snapshot::build(
                None,
                self.screen,
                screen_size,
                self.pointer.cursor(),
                shell,
            ));
            renderer.render();
            return;
        };

        renderer.set_crosshair_visible(self.screen.gameplay_enabled());
        renderer.set_hand_visible(!matches!(self.screen, crate::app::AppScreen::Pause));

        {
            let frame = game.client_frame(now);
            renderer.update_uniforms(
                frame.camera,
                frame.environment.fog,
                frame.environment.time,
                frame.environment.underwater,
            );
            renderer.set_selection(
                self.screen
                    .gameplay_enabled()
                    .then_some(frame.selection)
                    .flatten(),
            );
            let hand = std::mem::take(&mut self.hand);
            renderer.set_held_item(HeldItemFrame {
                item: frame.held_item.item,
                mining: frame.held_item.mining,
                broke_block: hand.broke,
                placed: hand.placed,
                swung: hand.swung,
                dt,
            });
        }
        // Build the neutral read snapshot, then bake it into render wire structs.
        {
            let presentation = self.presentation.snapshot(game);
            renderer.set_break_overlay(presentation.break_overlay);
            self.scene.bake(&presentation);
        }
        self.scene.upload(renderer);
        renderer.set_ui(ui_snapshot::build(
            Some(game),
            self.screen,
            screen_size,
            self.pointer.cursor(),
            shell,
        ));

        {
            let mut terrain = game.terrain_render_handoff();
            renderer.sync_meshes(&mut terrain);
        }
        renderer.render();
    }
}
