use super::super::tick::TickEvents;
use super::super::Game;
use crate::camera::Camera;
use crate::game::{GameEvents, GameInput};
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
use crate::mathh::{IVec3, Vec3};
use crate::net::protocol::{ClientToServer, PlayerUpdate, TargetRef};
use crate::server::game::ServerGame;
use crate::server::handle::LoopbackServer;

/// The game test fixture: the client [`Game`] wired to a LOOPBACK
/// [`ServerHandle`](crate::server::handle::ServerHandle) — the REAL message
/// channels, serviced synchronously by this harness instead of the server
/// thread (deterministic; the thread itself is covered by the Phase D handle
/// tests in `server/handle.rs`). The `ServerGame` is held here so tests keep
/// driving sim stages and asserting server state directly (`game.server.…`),
/// while every client read/method resolves through `Deref` to [`Game`].
pub(super) struct TestGame {
    pub(super) game: Game,
    pub(super) server: ServerGame,
    pipe: LoopbackServer,
}

impl std::ops::Deref for TestGame {
    type Target = Game;
    fn deref(&self) -> &Game {
        &self.game
    }
}

impl std::ops::DerefMut for TestGame {
    fn deref_mut(&mut self) -> &mut Game {
        &mut self.game
    }
}

pub(super) fn game() -> TestGame {
    game_with_camera(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0))
}

/// The fixture with an explicit camera (the WASM child tests spawn near their
/// build site).
pub(super) fn game_with_camera(cam: Camera) -> TestGame {
    let (server, bootstrap) = crate::game::session::build_session("", 1, 1);
    let (handle, pipe) = crate::server::handle::ServerHandle::loopback();
    let game = Game::assemble(cam, handle, bootstrap);
    TestGame { game, server, pipe }
}

impl TestGame {
    /// One full production frame with the pipe serviced synchronously in the
    /// middle: the client's send half, one server pump over the SAME dt, then
    /// the client's receive half — byte-for-byte the pre-thread `Game::tick`
    /// semantics (a fixed tick executes iff `dt` banks one).
    pub(super) fn tick(&mut self, dt: f32, input: &GameInput) -> GameEvents {
        self.game.tick_send(dt, input);
        self.pump_server(dt);
        self.game.tick_receive(dt)
    }

    /// Service the server end of the loopback pipe once, standing in for one
    /// iteration of the server thread's loop.
    pub(super) fn pump_server(&mut self, dt: f32) {
        let mut inbox: Vec<ClientToServer> = Vec::new();
        while let Ok(msg) = self.pipe.inbox.try_recv() {
            inbox.push(msg);
        }
        let out = self.server.pump(dt, &mut inbox);
        for msg in out.msgs {
            let _ = self.pipe.outbox.send(msg);
        }
    }

    /// Inject a raw server→client message into the loopback pipe, as if the
    /// server thread had sent it — for tests asserting how the client applies
    /// a hand-crafted batch.
    pub(super) fn send_server_message(&mut self, msg: crate::net::protocol::ServerToClient) {
        let _ = self.pipe.outbox.send(msg);
    }

    /// [`Self::tick`], but returning a copy of every server→client message
    /// the pump forwarded — the streaming-path observability the section
    /// cache tests assert on (payload copies are `Arc` bumps).
    pub(super) fn tick_recorded(
        &mut self,
        dt: f32,
        input: &GameInput,
    ) -> Vec<crate::net::protocol::ServerToClient> {
        self.game.tick_send(dt, input);
        let mut inbox: Vec<ClientToServer> = Vec::new();
        while let Ok(msg) = self.pipe.inbox.try_recv() {
            inbox.push(msg);
        }
        let out = self.server.pump(dt, &mut inbox);
        let recorded = out.msgs.clone();
        for msg in out.msgs {
            let _ = self.pipe.outbox.send(msg);
        }
        self.game.tick_receive(dt);
        recorded
    }

    // --- Wrapped Game action methods. In production these queue messages the
    // next frame sends; stage-driving tests expect them latched on the server
    // immediately (the old `#[cfg(test)]` inline flushes), so the wrappers
    // (inherent methods win over `Deref`) forward and flush.

    pub(super) fn drop_selected_item(&mut self, all: bool) {
        self.game.drop_selected_item(all);
        self.flush_outbox_for_test();
    }

    pub(super) fn throw_cursor_stack(&mut self) {
        self.game.throw_cursor_stack();
        self.flush_outbox_for_test();
    }

    pub(super) fn throw_cursor_one(&mut self) {
        self.game.throw_cursor_one();
        self.flush_outbox_for_test();
    }

    pub(super) fn menu_click(
        &mut self,
        slot: crate::gui::MenuSlot,
        button: crate::controls::PointerButton,
        shift: bool,
        gather: bool,
    ) {
        self.game.menu_click(slot, button, shift, gather);
        self.flush_outbox_for_test();
    }

    /// Queue + latch the close, then apply it the way the next tick would.
    pub(super) fn close_open_menu(&mut self) {
        self.game.close_open_menu();
        self.apply_latched_actions_for_test();
    }

    pub(super) fn request_wake(&mut self) {
        self.game.request_wake();
        self.flush_outbox_for_test();
    }

    pub(super) fn request_respawn(&mut self) {
        self.game.request_respawn();
        self.flush_outbox_for_test();
    }

    /// Mirrors the production call plus the replicated-view/session syncs the
    /// frame pump would provide around it.
    pub(super) fn toggle_held_block_rotation(&mut self) {
        // Stage-driven tests mutate the session inventory directly; stand in
        // for the batch that would have refreshed the replicated view.
        self.sync_self_view_for_test();
        self.game.toggle_held_block_rotation();
        self.sync_held_rotation_for_test();
    }

    // --- Test-only state bridges (the pre-Phase-D `Game` helpers, now living
    // on the harness because only it can see both halves).

    /// Hand the queued outbox messages to the server, standing in for the
    /// message send `Game::tick` performs each frame.
    pub(super) fn flush_outbox_for_test(&mut self) {
        for msg in std::mem::take(&mut self.game.outbox) {
            self.server.apply_message(0, msg);
        }
    }

    /// Apply the player actions latched this frame — container edits and item
    /// drops — standing in for the game tick that resolves them in play, then
    /// refresh the replicated read models the way the tick's batch would.
    pub(super) fn apply_latched_actions_for_test(&mut self) {
        self.flush_outbox_for_test();
        self.server.apply_latched_actions_for_test();
        self.sync_self_view_for_test();
        self.sync_menu_view_for_test();
    }

    /// Build session 0's `SelfState` exactly as the pump would and apply it —
    /// for tests that drive tick stages directly (no frame pump) and then
    /// assert the client-side read models. Forces the inventory body (tests
    /// replace whole `Inventory` values, which resets the revision the
    /// on-change gate compares).
    pub(super) fn sync_self_view_for_test(&mut self) {
        self.server.sessions[0].last_sent_inventory_revision = None;
        let state = self.server.build_self_state(0);
        self.game.self_view.apply(&state);
    }

    /// Mirror of the next batch's `menu_sync`, for tests that drive menu
    /// sessions without frames.
    pub(super) fn sync_menu_view_for_test(&mut self) {
        if let Some(sync) = self.server.build_menu_sync(0) {
            self.game.menu_view.apply(sync);
        }
    }

    /// Mirror of what the next batch's `TickUpdate.open_chests` does, for
    /// tests that drive `chest_viewers` directly (no frame pump).
    pub(super) fn sync_open_chests_for_test(&mut self) {
        self.game.open_chests = self.server.chest_viewers.keys().copied().collect();
    }

    /// Mirror of what the next frame's `PlayerUpdate` does with the rotation
    /// counter, for tests that drive tick stages without frames.
    pub(super) fn sync_held_rotation_for_test(&mut self) {
        let sess = &mut self.server.sessions[0];
        let selected = sess.selected_item();
        sess.held_rotation
            .apply_wire(self.game.held_rotation.rotation, selected);
    }

    /// The SESSION inventory — the authoritative one the sim mutates.
    pub(super) fn inventory(&self) -> &Inventory {
        &self.server.sessions[0].player.inventory
    }

    /// Double-click gather: top up the cursor-held stack with every matching
    /// item in the inventory. See [`Inventory::collect_to_cursor`].
    pub(super) fn collect_to_cursor(&mut self) {
        self.server.sessions[0].player.inventory.collect_to_cursor();
    }

    /// Test injection: replace the mod host (e.g. with a WAT guest) so the GUI
    /// click dispatch plumbing can be driven without compiled mods.
    pub(super) fn set_mods_for_test(&mut self, mods: crate::modding::ModHost) {
        self.server.mods = mods;
    }

    pub(super) fn mods_for_test(&self) -> &crate::modding::ModHost {
        &self.server.mods
    }
}

/// A `PlayerUpdate` mirroring the session player's current transform (what the
/// in-process client sends on an ordinary frame), with gameplay input live.
/// Tests tweak fields to drive the message path.
pub(super) fn player_update(game: &TestGame, gameplay: bool) -> PlayerUpdate {
    let p = &game.server.sessions[0].player;
    PlayerUpdate {
        pos: p.pos,
        vel: p.vel,
        yaw: p.yaw,
        pitch: p.pitch,
        on_ground: p.on_ground,
        sneak: false,
        gameplay,
        break_held: false,
        use_held: false,
        target: None,
        hotbar_slot: p.inventory.active_slot(),
        held_rotation: 0,
        wishdir: crate::mathh::Vec3::ZERO,
        jump: false,
        sprint: false,
    }
}

/// A hotbar slot filled with one full demo stack, for tests that need the
/// player holding something (the real starting inventory is empty).
pub(super) fn filled_inventory() -> Inventory {
    let mut inv = Inventory::new();
    inv.add(ItemStack::new(ItemType::Dirt, 64));
    inv
}

pub(super) fn apply_drop_actions(game: &mut TestGame) -> TickEvents {
    // The action methods queue MESSAGES now; hand them to the server the way
    // `Game::tick` would before applying the drop stage.
    game.flush_outbox_for_test();
    let mut events = TickEvents::default();
    game.server.tick_drops(0, &mut events);
    events
}

/// A latched look target, as `apply_player_update` would leave it.
pub(super) fn hit(pos: IVec3, normal: IVec3) -> TargetRef {
    TargetRef { block: pos, normal }
}

pub(super) fn install_empty_chunk(game: &mut TestGame) {
    let pos = crate::chunk::ChunkPos::new(0, 0);
    game.server.world.clear_world();
    game.server
        .world
        .insert_chunk_for_test(pos, crate::chunk::Chunk::new(0, 0));
}

/// Put the authoritative session eye at `eye`, looking along `dir`, with an
/// exact matching movement claim so reach validation uses that position.
pub(super) fn set_server_view(game: &mut TestGame, eye: Vec3, dir: Vec3) {
    let dir = dir.normalize();
    let sess = &mut game.server.sessions[0];
    sess.player.pos = eye - Vec3::Y * crate::player::EYE;
    sess.player.yaw = dir.x.atan2(dir.z);
    sess.player.pitch = dir.y.clamp(-1.0, 1.0).asin();
    sess.claim_pos = sess.player.pos;
    sess.ticks_since_claim = 0;
}

/// Aim the authoritative session through the centre segment of mob `index`,
/// close enough for an honest target click.
pub(super) fn aim_server_at_mob(game: &mut TestGame, index: usize) {
    let mob = &game.server.world.mobs().instances()[index];
    let size = crate::mob::def(mob.kind).size;
    let target = mob.pos + Vec3::Y * (size.height * 0.5);
    set_server_view(game, target - Vec3::Z * 2.0, Vec3::Z);
}

pub(super) fn count_item(inv: &Inventory, item: ItemType) -> u32 {
    (0..crate::inventory::TOTAL_SLOTS)
        .filter_map(|i| inv.slot(i))
        .filter(|s| s.item == item)
        .map(|s| s.count as u32)
        .sum()
}
