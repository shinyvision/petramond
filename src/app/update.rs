use super::{now_seconds, App};
use crate::game::CameraPose;
use crate::render::Renderer;

impl App {
    /// Advance input and the simulation for this wake, and report whether the frame
    /// must be redrawn. The sim is decoupled from drawing: the host calls this every
    /// wake (at least at the tick rate) whether or not it then draws, so
    /// [`Game::tick`](crate::game::Game::tick)'s fixed-step accumulator holds the world
    /// at 20 TPS regardless of frame rate. A redraw is needed when input changed
    /// something ([`dirty`](Self::dirty)), the view moved, a hand action fired or is
    /// still animating, terrain is (re)meshing, or anything on screen is animating
    /// (from the game client-frame read model). Slow sky drift is left to the host's
    /// keep-alive redraw.
    pub fn update(&mut self, renderer: &Renderer) -> bool {
        let now = now_seconds();
        let dt = (now - self.last) as f32;
        self.last = now;

        let screen_size = renderer.screen_size();
        self.recenter_pointer_if_pending(screen_size);

        // Route inventory clicks before reading game input, so a right-click
        // consumed by the open inventory never also fires block placement.
        if self.pointer.left_clicked() && self.route_screen_click(screen_size, now) {
            self.pointer.clear_left_click();
        }
        if self.pointer.right_clicked() && self.route_screen_right_click(screen_size, now) {
            self.pointer.clear_right_click();
        }

        // Sampled BEFORE the tick: `game.tick` runs the mesh budget, which drains the
        // dirty-mesh queue into built-but-unuploaded meshes that `render` uploads. Reading
        // it here keeps build + upload in the same frame, so a changed chunk can never
        // settle without being drawn.
        let frame_before_tick = self.game.client_frame_before_tick();

        let game_input = self.take_game_input();
        let events = self.game.tick(dt, &game_input);
        self.handle_open_screen_events(&events);
        let (mining_block, camera_pose, visually_active) = {
            let frame = self.game.client_frame(now);
            (
                frame.held_item.mining_block,
                frame.camera_pose,
                frame.activity.visually_active,
            )
        };
        self.play_game_event_sounds(&events, mining_block, now);
        self.pointer.clear_edges();
        let event_presentation = self.latch_game_event_hand_triggers(&events);
        // `dirty` is peeked-and-cleared here: a redraw consumes the pending-input flag.
        std::mem::take(&mut self.dirty)
            || event_presentation.acted
            || renderer.hand_animation_active()
            || frame_before_tick.mesh_pending
            || self.camera_moved(camera_pose)
            || visually_active
    }

    /// Whether the camera pose changed since the last [`render`](Self::render). At rest
    /// on the ground the pose is reproduced bit-for-bit, so this is `false` when idle;
    /// `None` (before the first draw) counts as moved so the opening frame is drawn.
    fn camera_moved(&self, pose: CameraPose) -> bool {
        self.last_pose != Some(pose)
    }
}
