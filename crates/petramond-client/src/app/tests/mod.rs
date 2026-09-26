use super::session::Session;
use super::App;
use crate::game::Game;
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;
use petramond_world::gui_state::MenuSlot;
use petramond_world::item::{ItemStack, ItemType};

mod harness;
use harness::TestApp;

mod connect;
mod controls;
mod creative;
mod drops;
mod gui_routing;
mod overlays;
mod perf;
mod session_flow;
mod sounds;

impl App {
    /// Whether a game session is running.
    fn has_session(&self) -> bool {
        self.session.is_some()
    }

    /// The running session (panics without one: the fixture loads a world).
    fn sess(&self) -> &Session {
        self.session.as_ref().expect("test app has a loaded game")
    }

    fn sess_mut(&mut self) -> &mut Session {
        self.session.as_mut().expect("test app has a loaded game")
    }

    fn game(&self) -> &Game {
        &self.sess().game
    }

    fn game_mut(&mut self) -> &mut Game {
        &mut self.sess_mut().game
    }

    /// The session's recipe/item hooks from the last solved document frame.
    fn doc_hooks_for_test(&mut self) -> &[petramond::gui::DocHook] {
        self.ui.refresh_doc_geometry();
        self.ui.doc_geometry().1
    }

    /// One footstep pass over `rows` at `tick`, against the session's
    /// cadence state.
    fn tick_footsteps_for_test(
        &mut self,
        listener: petramond_audio::SpatialListener,
        rows: &[crate::animation::FootstepSource],
        tick: u64,
    ) {
        let session = self.session.as_mut().expect("test app has a loaded game");
        self.sound
            .tick_footsteps(&mut session.sounds, listener, rows, tick);
    }

    /// One idle-mob-sound pass over `mobs` at `tick`, against the session's
    /// schedule.
    fn tick_idle_mob_sounds_for_test(
        &mut self,
        listener: petramond_audio::SpatialListener,
        mobs: &[crate::game::presentation::MobPresentation],
        tick: u64,
    ) {
        let session = self.session.as_mut().expect("test app has a loaded game");
        self.sound
            .tick_idle_mob_sounds(&mut session.sounds, listener, mobs, tick);
    }

    /// Solve one input-free document frame for the open menu so its slot rects
    /// are available on `self.ui.out()`.
    fn solve_menu_frame_for_test(&mut self, screen: (u32, u32)) {
        let kind = self.doc_ui_kind().expect("open menu is document-backed");
        self.drive_doc_menu(kind, screen, 0.0);
    }
}

/// One inventory-tier test recipe: consume 1 `ingredient` → `result`.
fn test_recipe(
    key: &str,
    ingredient: ItemType,
    result: ItemStack,
) -> petramond_world::crafting::CraftingRecipe {
    use petramond_world::crafting::{
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
    let (server, bootstrap) =
        crate::game::tests::bootstrap::build_session_inline("", 1, render_dist);
    let (handle, pipe) = petramond::net::handle::ServerHandle::loopback();
    let game = Game::assemble(
        Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0),
        handle,
        bootstrap,
    );
    let mut app = App::new(
        Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0),
        render_dist,
    );
    app.adopt_game(game);
    TestApp::new(app, server, pipe)
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
        let hit = petramond::gui::Role::from_key(&slot.role)
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
