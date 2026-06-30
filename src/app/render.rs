use super::{now_seconds, ui_snapshot, App};
use crate::render::{HeldItemFrame, Renderer};

impl App {
    /// Draw the current frame. The host calls this only when [`update`](Self::update)
    /// (or its periodic keep-alive) decided the frame would differ from the last one,
    /// so drawing is fully decoupled from the simulation tick.
    pub fn render(&mut self, renderer: &mut Renderer) {
        let now = now_seconds();
        // The hand animation advances by render time (not sim time); clamp so a long
        // idle gap before the first active frame can't jump a swing mid-flight.
        let dt = ((now - self.last_render) as f32).clamp(0.0, 0.1);
        self.last_render = now;
        let screen_size = renderer.screen_size();

        let last_pose = {
            let frame = self.game.client_frame(now);
            renderer.update_uniforms(
                frame.camera,
                frame.environment.fog,
                frame.environment.time,
                frame.environment.underwater,
            );
            renderer.set_selection(frame.selection);
            let hand = std::mem::take(&mut self.hand);
            renderer.set_held_item(HeldItemFrame {
                item: frame.held_item.item,
                mining: frame.held_item.mining,
                broke_block: hand.broke,
                placed: hand.placed,
                swung: hand.swung,
                dt,
            });
            frame.camera_pose
        };
        // Build the neutral read snapshot, then bake it into render wire structs.
        {
            let presentation = self.presentation.snapshot(&self.game);
            renderer.set_break_overlay(presentation.break_overlay);
            self.scene.bake(&presentation);
        }
        self.scene.upload(renderer);
        renderer.set_ui(ui_snapshot::build(
            &self.game,
            self.screen,
            screen_size,
            self.pointer.cursor(),
        ));

        {
            let mut terrain = self.game.terrain_render_handoff();
            renderer.sync_meshes(&mut terrain);
        }
        renderer.render();

        // Remember the drawn view so the next `update` can tell a still camera (idle)
        // from a moved one and redraw only on change.
        self.last_pose = Some(last_pose);
    }
}
