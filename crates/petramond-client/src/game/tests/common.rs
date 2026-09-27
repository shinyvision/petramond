use super::super::Game;
use crate::game::{GameEvents, GameInput};
use petramond::events::tick::TickEvents;
use petramond::net::handle::LoopbackServer;
use petramond::net::protocol::{ClientToServer, PlayerUpdate, TargetRef};
use petramond::server::game::ServerGame;
use petramond::server::player::ConnectedPlayer;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;
use petramond_world::inventory::Inventory;
use petramond_world::item::{ItemStack, ItemType};

/// The game test fixture: the client [`Game`] wired to a LOOPBACK
/// [`ServerHandle`](petramond::net::handle::ServerHandle) — the REAL message
/// channels, serviced synchronously by this harness instead of the server
/// thread (deterministic; the thread itself is covered by the handle
/// tests in `server/handle.rs`). Every client read/method resolves through
/// `Deref` to [`Game`].
///
/// The `ServerGame` is PRIVATE to the harness: tests reach the authoritative
/// side only through the narrow API below — the local session's player and
/// session record, the server world, messages sent as the local client, and
/// (for tests that drive individual tick stages) [`sim`](Self::sim) /
/// [`sim_mut`](Self::sim_mut). The server's internal layout (session
/// indices, where the world lives) is therefore known in this one file, and a
/// server refactor updates the harness instead of every test.
pub(super) struct TestGame {
    pub(super) game: Game,
    server: ServerGame,
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
    game_with_camera(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0))
}

pub(super) fn game_on_empty_chunk() -> TestGame {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server_player_mut().pos = WorldPos::new(8.5, 64.0, 8.5);
    game
}

pub(super) fn game_with_camera(cam: Camera) -> TestGame {
    assemble("", cam)
}

pub(super) fn game_for_world(world: &str) -> TestGame {
    assemble(
        world,
        Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0),
    )
}

fn assemble(world: &str, cam: Camera) -> TestGame {
    let (server, bootstrap) = crate::game::tests::bootstrap::build_session_inline(world, 1, 1);
    let (handle, pipe) = petramond::net::handle::ServerHandle::loopback();
    let game = Game::assemble(cam, handle, bootstrap);
    TestGame { game, server, pipe }
}

impl TestGame {
    pub(super) fn tick(&mut self, dt: f32, input: &GameInput) -> GameEvents {
        self.game.tick_send(dt, input);
        self.pump_server(dt);
        self.game.tick_receive(dt)
    }

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

    pub(super) fn send_server_message(&mut self, msg: petramond::net::protocol::ServerToClient) {
        let _ = self.pipe.outbox.send(msg);
    }

    pub(super) fn tick_recorded(
        &mut self,
        dt: f32,
        input: &GameInput,
    ) -> Vec<petramond::net::protocol::ServerToClient> {
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

    pub(super) fn drop_selected_item(&mut self, all: bool) {
        self.game.drop_selected_item(all);
        self.flush_outbox_for_test();
    }

    pub(super) fn throw_cursor(&mut self, amount: petramond::net::protocol::ThrowAmount) {
        self.game.throw_cursor(amount);
        self.flush_outbox_for_test();
    }

    pub(super) fn menu_click(
        &mut self,
        slot: petramond_world::gui_state::MenuSlot,
        button: petramond_world::gui_state::PointerButton,
        shift: bool,
        gather: bool,
    ) {
        self.game.menu_click(slot, button, shift, gather);
        self.flush_outbox_for_test();
    }

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

    pub(super) fn toggle_held_block_rotation(&mut self) {
        self.sync_self_view_for_test();
        self.game.toggle_held_block_rotation();
        self.sync_held_rotation_for_test();
    }

    pub(super) fn flush_outbox_for_test(&mut self) {
        for msg in self.game.net.take_outbox_for_test() {
            self.server.apply_message(0, msg);
        }
    }

    pub(super) fn apply_latched_actions_for_test(&mut self) {
        self.flush_outbox_for_test();
        self.server.apply_latched_actions_for_test();
        self.sync_self_view_for_test();
        self.sync_menu_view_for_test();
    }

    pub(super) fn sync_self_view_for_test(&mut self) {
        self.server.sessions_mut()[0]
            .replication_mut()
            .last_sent_inventory_revision = None;
        let state = self.server.build_self_state(0);
        self.game.replica.self_view.apply(&state, true);
    }

    pub(super) fn sync_menu_view_for_test(&mut self) {
        if let Some(sync) = self.server.build_menu_sync(0) {
            self.game.replica.menu_view.apply(sync);
        }
    }

    pub(super) fn sync_open_chests_for_test(&mut self) {
        let open = self.server.open_chests().into_iter().collect();
        self.game.fx.set_open_chests(open);
    }

    pub(super) fn sync_held_rotation_for_test(&mut self) {
        let sess = &mut self.server.sessions_mut()[0];
        let selected = sess.selected_item();
        sess.input_mut()
            .held_rotation
            .apply_wire(self.game.local.held_rotation.rotation, selected);
    }

    pub(super) fn inventory(&self) -> &Inventory {
        &self.server_player().inventory
    }

    pub(super) fn server_player(&self) -> &petramond::player::Player {
        self.session().player()
    }

    pub(super) fn server_player_mut(&mut self) -> &mut petramond::player::Player {
        self.session_mut().player_mut()
    }

    pub(super) fn session(&self) -> &ConnectedPlayer {
        self.session_at(0)
    }

    pub(super) fn session_mut(&mut self) -> &mut ConnectedPlayer {
        self.session_at_mut(0)
    }

    pub(super) fn session_at(&self, index: usize) -> &ConnectedPlayer {
        &self.server.sessions()[index]
    }

    pub(super) fn session_at_mut(&mut self, index: usize) -> &mut ConnectedPlayer {
        &mut self.server.sessions_mut()[index]
    }

    pub(super) fn server_world(&self) -> &petramond::world::ServerWorld {
        self.server.world()
    }

    pub(super) fn server_world_mut(&mut self) -> &mut petramond::world::ServerWorld {
        self.server.world_mut()
    }

    pub(super) fn server_world_tick(&mut self) {
        let recipes = self.server.recipes().clone();
        self.server.world_mut().game_tick(&recipes);
    }

    pub(super) fn send_to_server(&mut self, msg: ClientToServer) {
        self.server.apply_message(0, msg);
    }

    pub(super) fn sim(&self) -> &ServerGame {
        &self.server
    }

    pub(super) fn sim_mut(&mut self) -> &mut ServerGame {
        &mut self.server
    }

    pub(super) fn collect_to_cursor(&mut self) {
        self.server.sessions_mut()[0]
            .player_mut()
            .inventory
            .collect_to_cursor();
    }

    pub(super) fn set_mods_for_test(&mut self, mods: petramond::modding::ModHost) {
        self.server.replace_mod_host_for_test(mods);
    }

    pub(super) fn mods_for_test(&self) -> &petramond::modding::ModHost {
        self.server.mod_host()
    }
}

pub(super) fn player_update(game: &TestGame, gameplay: bool) -> PlayerUpdate {
    let p = game.server.sessions()[0].player();
    PlayerUpdate {
        transform: petramond::net::protocol::Transform {
            pos: p.pos,
            vel: p.vel,
            yaw: p.yaw,
            pitch: p.pitch,
        },
        on_ground: p.on_ground,
        sneak: false,
        gameplay,
        break_held: false,
        use_held: false,
        target: None,
        hotbar_slot: p.inventory.active_slot(),
        held_rotation: 0,
        wishdir: petramond_math::math::Vec3::ZERO,
        jump: false,
        sprint: false,
    }
}

pub(super) fn filled_inventory() -> Inventory {
    let mut inv = Inventory::new();
    inv.add(ItemStack::new(ItemType::Dirt, 64));
    inv
}

pub(super) fn give(game: &mut TestGame, item: ItemType, n: u8) {
    let mut inv = Inventory::new();
    inv.add(ItemStack::new(item, n));
    game.server_player_mut().inventory = inv;
}

pub(super) fn apply_drop_actions(game: &mut TestGame) -> TickEvents {
    game.flush_outbox_for_test();
    let mut events = TickEvents::default();
    game.server.tick_drops(0, &mut events);
    events
}

pub(super) fn hit(pos: IVec3, normal: IVec3) -> TargetRef {
    TargetRef::face(pos, normal)
}

pub(super) fn install_empty_chunk(game: &mut TestGame) {
    let pos = petramond_world::chunk::ChunkPos::new(0, 0);
    game.server.world_mut().clear_world();
    game.server
        .world_mut()
        .insert_chunk_for_test(pos, petramond_world::chunk::Chunk::new(0, 0));
}

pub(super) fn flat_floor_loaded_air<S: petramond::world::WorldSide>(
    world: &mut petramond::world::World<S>,
    floor: petramond_world::block::Block,
) {
    install_flat_floor(world, floor, true);
}

fn install_flat_floor<S: petramond::world::WorldSide>(
    world: &mut petramond::world::World<S>,
    floor: petramond_world::block::Block,
    loaded_air: bool,
) {
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    let pos = ChunkPos::new(0, 0);
    world.clear_world();
    if loaded_air {
        world.insert_empty_column_for_test(pos);
    }
    let mut chunk = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            chunk.set_block(x, 63, z, floor);
        }
    }
    world.insert_chunk_for_test(pos, chunk);
}

pub(super) fn set_server_view(game: &mut TestGame, eye: WorldPos, dir: Vec3) {
    let dir = dir.normalize();
    let sess = game.session_mut();
    sess.player_mut().pos = eye - Vec3::Y * petramond::player::EYE;
    sess.player_mut().yaw = dir.x.atan2(dir.z);
    sess.player_mut().pitch = dir.y.clamp(-1.0, 1.0).asin();
    let pos = sess.player().pos;
    sess.input_mut().claim_pos = pos;
    sess.input_mut().ticks_since_claim = 0;
}

pub(super) fn aim_server_at_mob(game: &mut TestGame, index: usize) {
    let mob = &game.server.world().mobs().instances()[index];
    let size = petramond::mob::def(mob.kind).size;
    let target = mob.pos + Vec3::Y * (size.height * 0.5);
    set_server_view(game, target - Vec3::Z * 2.0, Vec3::Z);
}

pub(super) fn count_item(inv: &Inventory, item: ItemType) -> u32 {
    (0..petramond_world::inventory::TOTAL_SLOTS)
        .filter_map(|i| inv.slot(i))
        .filter(|s| s.item == item)
        .map(|s| s.count as u32)
        .sum()
}
