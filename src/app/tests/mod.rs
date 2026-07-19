use super::App;
use crate::camera::Camera;
use crate::controls::PointerButton;
use crate::game::Game;
use crate::gui::MenuSlot;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;
use crate::server::game::ServerGame;
use crate::server::handle::LoopbackServer;

mod connect;
mod controls;
mod drops;
mod gui_routing;
mod overlays;
mod perf;
mod sounds;

impl App {
    fn game(&self) -> &Game {
        self.game.as_ref().expect("test app has a loaded game")
    }

    /// Solve one input-free document frame for the open menu so its slot rects
    /// are available on `self.ui.out()`.
    fn solve_menu_frame_for_test(&mut self, screen: (u32, u32)) {
        let kind = self.doc_ui_kind().expect("open menu is document-backed");
        self.drive_doc_menu(kind, screen, 0.0);
    }
}

/// The app test fixture: a real [`App`] whose game session rides a LOOPBACK
/// server pipe, with the `ServerGame` held here — the same shape as the game
/// tests' `TestGame` (`src/game/tests/common.rs`), so app tests keep driving
/// latched actions and asserting session state synchronously.
struct TestApp {
    app: App,
    server: ServerGame,
    #[allow(dead_code)]
    pipe: LoopbackServer,
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
    /// Click the open document menu at the current cursor and then apply the
    /// latched container edit / drop, standing in for the game tick that
    /// resolves it in play. Returns whether a document menu consumed the click
    /// (false with no menu open, so the click would fall through to gameplay).
    fn click_screen_for_test(&mut self, screen: (u32, u32), now: f64) -> bool {
        self.press_screen_for_test(screen, now, PointerButton::Primary)
    }

    /// Right-click counterpart of [`click_screen_for_test`](Self::click_screen_for_test).
    fn right_click_screen_for_test(&mut self, screen: (u32, u32), now: f64) -> bool {
        self.press_screen_for_test(screen, now, PointerButton::Secondary)
    }

    fn press_screen_for_test(
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
    fn drag_screen_for_test(
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
    fn frame_and_pump_recorded(
        &mut self,
        screen: (u32, u32),
        kinds: &mut std::collections::BTreeMap<String, usize>,
    ) -> (usize, usize) {
        self.app.last -= 0.05;
        self.app.update_frame(screen);
        let mut inbox: Vec<crate::net::protocol::ClientToServer> = Vec::new();
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

    fn apply_latched_actions_for_test(&mut self) {
        let game = self.app.game.as_mut().expect("test app has a loaded game");
        for msg in game.take_outbox_for_test() {
            self.server.apply_message(0, msg);
        }
        self.server.apply_latched_actions_for_test();
        // Refresh the replicated self/menu views the way the next batch would.
        self.server.sessions[0].last_sent_inventory_revision = None;
        let state = self.server.build_self_state(0);
        let sync = self.server.build_menu_sync(0);
        game.apply_views_for_test(&state, sync);
    }

    /// Build the exact content snapshot the renderer receives, including an
    /// in-progress slot gesture's presentation-only distribution overlay.
    fn menu_snapshot_for_test(&self) -> crate::gui::UiSnapshot {
        let preview = self.app.ui.menu_drag_preview();
        let preview = preview
            .as_ref()
            .map(|(slots, button)| (slots.as_slice(), *button));
        super::ui_snapshot::build(
            self.app.game.as_ref(),
            self.app.screen,
            self.app.pointer.cursor(),
            preview,
        )
    }

    /// The SESSION inventory — the authoritative one the sim mutates.
    fn inventory(&self) -> &Inventory {
        &self.server.sessions[0].player.inventory
    }

    fn add_to_inventory(&mut self, stack: ItemStack) {
        self.server.sessions[0].player.inventory.add(stack);
        // Recipe affordance is presentation-side and therefore reads the
        // replicated inventory, just like the real client after a batch.
        self.server.sessions[0].last_sent_inventory_revision = None;
        let state = self.server.build_self_state(0);
        let sync = self.server.build_menu_sync(0);
        self.app
            .game
            .as_mut()
            .expect("test app has a loaded game")
            .apply_views_for_test(&state, sync);
    }

    fn install_test_crafting_catalog(&mut self, recipes: Vec<crate::crafting::CraftingRecipe>) {
        self.server.recipes =
            crate::crafting::Recipes::new(recipes.clone(), Vec::new(), Vec::new());
        self.app
            .game
            .as_mut()
            .expect("test app has a loaded game")
            .set_crafting_catalog_for_test(crate::crafting::CraftingCatalog::new(recipes));
    }

    fn install_test_crafting_recipe(&mut self) {
        self.install_test_crafting_catalog(vec![test_recipe(
            "test:sticks",
            ItemType::Coal,
            ItemStack::new(ItemType::Stick, 2),
        )]);
    }
}

/// One inventory-tier test recipe: consume 1 `ingredient` → `result`.
fn test_recipe(
    key: &str,
    ingredient: ItemType,
    result: ItemStack,
) -> crate::crafting::CraftingRecipe {
    use crate::crafting::{
        CraftingIngredient, CraftingRecipe, CraftingStation, IngredientSelector, IngredientUse,
    };
    CraftingRecipe::new(
        key.into(),
        CraftingStation::Inventory,
        vec![CraftingIngredient {
            selector: IngredientSelector::Item(ingredient),
            count: 1,
            use_mode: IngredientUse::Consume,
        }],
        result,
    )
}

/// Keep test saves and client-mod storage out of the real user data dir.
/// Every test in this process gets the same per-process temp root, so the
/// benign parallel re-sets all write one identical value. Call before
/// computing any data-dir-derived path.
fn ensure_test_data_dir() {
    std::env::set_var(
        "PETRAMOND_DATA_DIR",
        std::env::temp_dir().join(format!("petramond-test-data-{}", std::process::id())),
    );
}

fn app() -> TestApp {
    app_with_render_dist(1)
}

fn app_with_render_dist(render_dist: i32) -> TestApp {
    ensure_test_data_dir();
    let (server, bootstrap) = crate::game::session::build_session_inline("", 1, render_dist);
    let (handle, pipe) = crate::server::handle::ServerHandle::loopback();
    let game = Game::assemble(
        Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0),
        handle,
        bootstrap,
    );
    let mut app = App::new(
        Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0),
        render_dist,
    );
    app.adopt_game(game);
    TestApp { app, server, pipe }
}

/// An app whose player holds one full stack in hotbar slot 0 - the starting
/// inventory is empty now, so inventory-interaction tests seed a stack first.
fn app_with_grass() -> TestApp {
    let mut app = app();
    app.add_to_inventory(ItemStack::new(ItemType::Grass, 64));
    app
}

/// The cursor pixel over the slot cell the open menu's DOCUMENT resolves to
/// `want`, from the real solved layout — tests never pin document pixel
/// positions.
fn cursor_over_menu(app: &mut App, screen: (u32, u32), want: MenuSlot) -> (f32, f32) {
    app.solve_menu_frame_for_test(screen);
    for slot in &app.ui.out().slots {
        let hit = crate::gui::Role::from_key(&slot.role)
            .and_then(|role| role.menu_slot(slot.index as usize));
        if hit == Some(want) {
            let r = slot.rect;
            return (r.x as f32 + r.w as f32 * 0.5, r.y as f32 + r.h as f32 * 0.5);
        }
    }
    panic!("no document slot cell maps to {want:?}");
}

fn cursor_over_slot(app: &mut App, screen: (u32, u32), slot: usize) -> (f32, f32) {
    cursor_over_menu(app, screen, MenuSlot::Inventory(slot))
}

fn cursor_over_craft_result(app: &mut App, screen: (u32, u32)) -> (f32, f32) {
    cursor_over_menu(app, screen, MenuSlot::CraftResult)
}

/// Center of one real document widget instance. `item` identifies a repeated
/// list row; `None` addresses an ordinary singleton widget.
fn cursor_over_widget(
    app: &mut App,
    screen: (u32, u32),
    id: &str,
    item: Option<u32>,
) -> (f32, f32) {
    app.solve_menu_frame_for_test(screen);
    let (_, rect) = app
        .ui
        .out()
        .named
        .iter()
        .find(|(key, _)| key.id == id && key.item == item)
        .unwrap_or_else(|| panic!("no document widget {id:?} row {item:?}"));
    (
        rect.x as f32 + rect.w as f32 * 0.5,
        rect.y as f32 + rect.h as f32 * 0.5,
    )
}

/// A point inside the open menu's panel rectangle that is NOT over any slot.
fn panel_gap_point(app: &mut App, screen: (u32, u32)) -> (f32, f32) {
    app.solve_menu_frame_for_test(screen);
    let out = app.ui.out();
    let panel = out.panel_rect;
    for y in panel.y..panel.y + panel.h {
        for x in panel.x..panel.x + panel.w {
            let c = (x as f32 + 0.5, y as f32 + 0.5);
            let on_slot = out.slots.iter().any(|s| {
                c.0 >= s.rect.x as f32
                    && c.0 < (s.rect.x + s.rect.w) as f32
                    && c.1 >= s.rect.y as f32
                    && c.1 < (s.rect.y + s.rect.h) as f32
            });
            if !on_slot {
                return c;
            }
        }
    }
    panic!("no in-panel, off-slot point found");
}

