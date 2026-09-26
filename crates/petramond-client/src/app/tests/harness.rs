use super::{cursor_over_menu, test_recipe};
use crate::app::App;
use petramond::net::handle::LoopbackServer;
use petramond::server::game::ServerGame;
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;
use petramond_world::gui_state::{MenuSlot, PointerButton};
use petramond_world::inventory::Inventory;
use petramond_world::item::{ItemStack, ItemType};

/// The app test fixture: a real [`App`] whose game session rides a LOOPBACK
/// server pipe, with the `ServerGame` held here — the same shape as the game
/// tests' `TestGame` (`src/game/tests/common.rs`), so app tests keep driving
/// latched actions and asserting session state synchronously.
pub(super) struct TestApp {
    pub(super) app: App,
    server: ServerGame,
    #[allow(dead_code)]
    pub(super) pipe: LoopbackServer,
}

impl std::ops::Deref for TestApp {
    type Target = App;
    fn deref(&self) -> &App {
        &self.app
    }
}

impl std::ops::DerefMut for TestApp {
    fn deref_mut(&mut self) -> &mut App {
        &mut self.app
    }
}

impl TestApp {
    pub(super) fn new(app: App, server: ServerGame, pipe: LoopbackServer) -> Self {
        Self { app, server, pipe }
    }

    /// Start a second loopback world in the SAME App after its first session
    /// ended. This exercises the real adoption path and fresh session scope.
    pub(super) fn restart_session(&mut self) {
        assert!(self.app.session.is_none());
        let (server, bootstrap) = crate::game::tests::bootstrap::build_session_inline("", 1, 1);
        let (handle, pipe) = petramond::net::handle::ServerHandle::loopback();
        let game = crate::game::Game::assemble(
            Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0),
            handle,
            bootstrap,
        );
        self.server = server;
        self.pipe = pipe;
        self.app.adopt_game(game);
    }

    pub(super) fn set_server_player_mode(&mut self, mode: petramond::player::PlayerMode) {
        self.server.sessions_mut()[0].player_mut().set_mode(mode);
    }

    pub(super) fn sync_server_self_state(&mut self) {
        let state = self.server.build_self_state(0);
        self.app.game_mut().apply_tick_update(Box::new(
            petramond::net::protocol::TickUpdate::new(0, 0).with(state),
        ));
    }

    pub(super) fn tick_server(&mut self) {
        self.server.game_tick_step(&mut Default::default());
    }

    pub(super) fn send_and_pump(
        &mut self,
        msgs: Vec<petramond::net::protocol::ClientToServer>,
    ) -> Vec<petramond::net::protocol::ServerToClient> {
        for msg in msgs {
            self.server.apply_message(0, msg);
        }
        self.server.pump(0.0, &mut Vec::new()).msgs
    }

    pub(super) fn mark_lan_opened(&mut self) {
        self.server.open_to_lan_for_test();
    }

    pub(super) fn set_server_player_pos(&mut self, pos: petramond_math::world_pos::WorldPos) {
        self.server.sessions_mut()[0].player_mut().pos = pos;
    }

    pub(super) fn server_craft_craftable_only(&self) -> bool {
        self.server.sessions()[0].player().craft_craftable_only
    }

    /// Click the open document menu at the current cursor and then apply the
    /// latched container edit / drop, standing in for the game tick that
    /// resolves it in play. Returns whether a document menu consumed the click
    /// (false with no menu open, so the click would fall through to gameplay).
    pub(super) fn click_screen_for_test(&mut self, screen: (u32, u32), now: f64) -> bool {
        self.press_screen_for_test(screen, now, PointerButton::Primary)
    }

    /// Right-click counterpart of [`click_screen_for_test`](Self::click_screen_for_test).
    pub(super) fn right_click_screen_for_test(&mut self, screen: (u32, u32), now: f64) -> bool {
        self.press_screen_for_test(screen, now, PointerButton::Secondary)
    }

    pub(super) fn press_screen_for_test(
        &mut self,
        screen: (u32, u32),
        now: f64,
        button: PointerButton,
    ) -> bool {
        if !self.screen.ui_open() {
            return false;
        }
        let kind = self.doc_ui_kind().expect("open menu is document-backed");
        self.app.set_pointer_button(button, true);
        self.app.set_pointer_button(button, false);
        self.app.drive_doc_menu(kind, screen, now);
        self.apply_latched_actions_for_test();
        true
    }

    /// Hold one physical menu button across an ordered set of real document
    /// slot cells, release it, then apply the resulting atomic drag action.
    pub(super) fn drag_screen_for_test(
        &mut self,
        screen: (u32, u32),
        now: f64,
        button: PointerButton,
        slots: &[MenuSlot],
    ) {
        assert!(slots.len() >= 2);
        let points: Vec<_> = slots
            .iter()
            .map(|&slot| cursor_over_menu(self, screen, slot))
            .collect();
        self.app.set_cursor_position(points[0].0, points[0].1);
        self.app.set_pointer_button(button, true);
        for &(x, y) in &points[1..] {
            self.app.set_cursor_position(x, y);
        }
        self.app.set_pointer_button(button, false);
        let kind = self.doc_ui_kind().expect("open menu is document-backed");
        self.app.drive_doc_menu(kind, screen, now);
        self.apply_latched_actions_for_test();
    }

    /// Flush the game's queued messages to the server, apply the latched
    /// actions, and refresh the replicated read models — what the game tests'
    /// harness does, reached through the App.
    /// One app frame with the loopback server pumped afterwards, standing in
    /// for one iteration of the production server thread: terrain streams
    /// into the replica with one frame of latency. The app clock is backdated
    /// one fixed tick so each headless frame banks a real tick (streaming
    /// requests and acks ride ticks).
    /// Returns (client→server, server→client) message counts and tallies the
    /// message variants — stream-health diagnostics.
    pub(super) fn frame_and_pump_recorded(
        &mut self,
        screen: (u32, u32),
        kinds: &mut std::collections::BTreeMap<String, usize>,
    ) -> (usize, usize) {
        self.app.last -= 0.05;
        self.app.update_frame(screen);
        let mut inbox: Vec<petramond::net::protocol::ClientToServer> = Vec::new();
        while let Ok(msg) = self.pipe.inbox.try_recv() {
            inbox.push(msg);
        }
        let sent = inbox.len();
        for msg in &inbox {
            let name = format!("{msg:?}");
            let name = name.split(&['(', ' ', '{'][..]).next().unwrap_or("?");
            *kinds.entry(format!("c->s {name}")).or_default() += 1;
        }
        let out = self.server.pump(0.05, &mut inbox);
        let received = out.msgs.len();
        for msg in out.msgs {
            let name = format!("{msg:?}");
            let name = name.split(&['(', ' ', '{'][..]).next().unwrap_or("?");
            *kinds.entry(format!("s->c {name}")).or_default() += 1;
            let _ = self.pipe.outbox.send(msg);
        }
        (sent, received)
    }

    pub(super) fn apply_latched_actions_for_test(&mut self) {
        let game = self.app.game_mut();
        for msg in game.take_outbox_for_test() {
            self.server.apply_message(0, msg);
        }
        self.server.apply_latched_actions_for_test();
        // Refresh the replicated self/menu views the way the next batch would.
        self.server.sessions_mut()[0]
            .replication_mut()
            .last_sent_inventory_revision = None;
        let state = self.server.build_self_state(0);
        let sync = self.server.build_menu_sync(0);
        game.apply_views_for_test(&state, sync);
    }

    /// Build the exact content snapshot the renderer receives, including an
    /// in-progress slot gesture's presentation-only distribution overlay.
    pub(super) fn menu_snapshot_for_test(&self) -> petramond::gui::UiSnapshot {
        let preview = self.app.ui.menu_drag_preview();
        let preview = preview
            .as_ref()
            .map(|(slots, button)| (slots.as_slice(), *button));
        super::super::ui_snapshot::build(
            self.app.session.as_ref().map(|s| &s.game),
            self.app.screen,
            self.app.controls.pointer.cursor(),
            preview,
        )
    }

    /// The SESSION inventory — the authoritative one the sim mutates.
    pub(super) fn inventory(&self) -> &Inventory {
        &self.server.sessions()[0].player().inventory
    }

    pub(super) fn add_to_inventory(&mut self, stack: ItemStack) {
        self.server.sessions_mut()[0]
            .player_mut()
            .inventory
            .add(stack);
        // Recipe affordance is presentation-side and therefore reads the
        // replicated inventory, just like the real client after a batch.
        self.server.sessions_mut()[0]
            .replication_mut()
            .last_sent_inventory_revision = None;
        let state = self.server.build_self_state(0);
        let sync = self.server.build_menu_sync(0);
        self.app.game_mut().apply_views_for_test(&state, sync);
    }

    pub(super) fn install_test_crafting_catalog(
        &mut self,
        recipes: Vec<petramond_world::crafting::CraftingRecipe>,
    ) {
        self.server
            .install_recipes_for_test(petramond_world::crafting::Recipes::new(
                recipes.clone(),
                Vec::new(),
            ));
        self.app.game_mut().set_crafting_catalog_for_test(
            petramond_world::crafting::CraftingCatalog::new(recipes),
        );
    }

    pub(super) fn install_test_crafting_recipe(&mut self) {
        self.install_test_crafting_catalog(vec![test_recipe(
            "test:sticks",
            ItemType::Coal,
            ItemStack::new(ItemType::Stick, 2),
        )]);
    }
}
