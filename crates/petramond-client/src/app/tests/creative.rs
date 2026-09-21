use super::{app, cursor_over_slot, cursor_over_widget, TestApp};
use crate::app::creative::CreativeTab;
use crate::app::schematic_library::LibraryPage;
mod history;
mod notices;
use petramond::player::PlayerMode;
use petramond_world::{
    controls::Control,
    gui_state::GuiKind,
    item::{ItemStack, ItemType},
};

fn creative_app() -> TestApp {
    let mut app = app();
    app.server.sessions[0].player.set_mode(PlayerMode::Creative);
    app.add_to_inventory(ItemStack::new(ItemType::Dirt, 1));
    let state = app.server.build_self_state(0);
    app.game
        .as_mut()
        .unwrap()
        .apply_tick_update(Box::new(petramond::net::protocol::TickUpdate {
            self_state: Some(state),
            ..Default::default()
        }));
    app.handle_control(Control::ToggleInventory, true);
    app.handle_control(Control::ToggleInventory, false);
    assert_eq!(app.doc_ui_kind(), Some(GuiKind::Creative));
    app
}

fn open_items(app: &mut TestApp) {
    app.creative_menu.tab = CreativeTab::Items;
}

fn open_library(app: &mut TestApp) {
    app.creative_menu.tab = CreativeTab::Schematics;
    app.library_form.page = LibraryPage::Library;
}

fn open_save_page(app: &mut TestApp) {
    app.creative_menu.tab = CreativeTab::Schematics;
    app.library_form.page = LibraryPage::Save;
}

fn on_save_page(app: &TestApp) -> bool {
    app.creative_menu.tab == CreativeTab::Schematics && app.library_form.page == LibraryPage::Save
}

fn on_library_page(app: &TestApp) -> bool {
    app.creative_menu.tab == CreativeTab::Schematics
        && app.library_form.page == LibraryPage::Library
}

fn settle_actions(app: &mut TestApp) {
    app.apply_latched_actions_for_test();
    app.server.game_tick_step(&mut Default::default());
    app.apply_latched_actions_for_test();
}

#[test]
fn catalog_pickup_uses_the_inventory_cursor_and_regular_slot_operations() {
    let mut app = creative_app();
    let screen = (1280, 720);
    app.creative_menu.query = "petramond:stone".into();
    let source = cursor_over_widget(&mut app, screen, "creative_item", Some(0));
    app.set_cursor_position(source.0, source.1);
    app.click_screen_for_test(screen, 1.0);
    let held = app
        .menu_snapshot_for_test()
        .cursor
        .expect("catalog seeds normal cursor");
    assert_eq!(held.item, ItemType::Stone);
    assert!(app.inventory().slot(5).is_none());
    let target = cursor_over_slot(&mut app, screen, 5);
    app.set_cursor_position(target.0, target.1);
    app.right_click_screen_for_test(screen, 1.5);
    assert_eq!(app.menu_snapshot_for_test().slots[5].unwrap().count, 1);
    assert_eq!(
        app.menu_snapshot_for_test().cursor.unwrap().count,
        held.count - 1
    );
    app.click_screen_for_test(screen, 2.0);
    settle_actions(&mut app);
    assert_eq!(app.inventory().slot(5).copied(), Some(held));
    assert!(app.inventory().cursor().is_none());
    app.click_screen_for_test(screen, 2.5);
    assert_eq!(app.menu_snapshot_for_test().cursor, Some(held));
    let occupied = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(occupied.0, occupied.1);
    app.click_screen_for_test(screen, 3.0);
    settle_actions(&mut app);
    assert_eq!(app.inventory().slot(0).copied(), Some(held));
    assert_eq!(app.inventory().cursor().unwrap().item, ItemType::Dirt);
}

#[test]
fn a_catalog_stack_can_be_distributed_with_the_existing_inventory_drag() {
    use petramond_world::gui_state::{MenuSlot, PointerButton};
    let mut app = creative_app();
    let screen = (1280, 720);
    app.creative_menu.query = "petramond:stone".into();
    let source = cursor_over_widget(&mut app, screen, "creative_item", Some(0));
    app.set_cursor_position(source.0, source.1);
    app.click_screen_for_test(screen, 0.0);
    let count = app.menu_snapshot_for_test().cursor.unwrap().count;
    app.drag_screen_for_test(
        screen,
        1.0,
        PointerButton::Primary,
        &[
            MenuSlot::Inventory(5),
            MenuSlot::Inventory(6),
            MenuSlot::Inventory(7),
        ],
    );
    settle_actions(&mut app);
    let counts: Vec<_> = (5..8)
        .map(|slot| {
            let stack = app.inventory().slot(slot).unwrap();
            assert_eq!(stack.item, ItemType::Stone);
            stack.count
        })
        .collect();
    assert_eq!(counts.iter().copied().sum::<u8>(), count);
    assert!(counts.iter().max().unwrap() - counts.iter().min().unwrap() <= 1);
    assert!(app.inventory().cursor().is_none());
}

#[test]
fn catalog_clicks_discard_the_cursor_on_items_gaps_and_empty_searches() {
    let mut app = creative_app();
    let screen = (1280, 720);
    for case in 0..3 {
        app.creative_menu.query = "petramond:stone".into();
        let source = cursor_over_widget(&mut app, screen, "creative_item", Some(0));
        app.set_cursor_position(source.0, source.1);
        app.click_screen_for_test(screen, 0.0);
        assert!(app.menu_snapshot_for_test().cursor.is_some());
        if case == 2 {
            app.creative_menu.query = "no matching items here".into();
        }
        app.solve_menu_frame_for_test(screen);
        let grid = app.ui.out().rect("creative_catalog_scroll").unwrap();
        let target = if case == 0 {
            source
        } else {
            ((grid.x + grid.w / 2) as f32, (grid.y + grid.h / 2) as f32)
        };
        app.set_cursor_position(target.0, target.1);
        if case == 1 {
            app.right_click_screen_for_test(screen, 0.5);
        } else {
            app.click_screen_for_test(screen, 0.5);
        }
        assert!(app.menu_snapshot_for_test().cursor.is_none());
        settle_actions(&mut app);
        assert!(app.inventory().cursor().is_none(), "authoritative discard");
        assert_eq!(app.inventory().slot(0).unwrap().item, ItemType::Dirt);
    }
}

#[test]
fn schematic_save_is_a_library_action_and_back_keeps_the_draft() {
    let mut app = creative_app();
    let screen = (1280, 720);
    open_library(&mut app);
    let add = cursor_over_widget(&mut app, screen, "new_schematic", None);
    app.set_cursor_position(add.0, add.1);
    app.click_screen_for_test(screen, 0.0);
    assert!(on_save_page(&app));
    app.library_form.name = "Draft".into();
    app.game
        .as_mut()
        .unwrap()
        .world_tools
        .selection
        .selection
        .region([1, 2, 3], [1, 2, 3], false)
        .unwrap();
    let back = cursor_over_widget(&mut app, screen, "back_to_schematics", None);
    app.set_cursor_position(back.0, back.1);
    app.click_screen_for_test(screen, 0.5);
    assert!(on_library_page(&app));
    let add = cursor_over_widget(&mut app, screen, "new_schematic", None);
    app.set_cursor_position(add.0, add.1);
    app.click_screen_for_test(screen, 1.0);
    assert_eq!(app.library_form.name, "Draft");
    assert_eq!(app.game().world_tools.selection.selection.len(), 1);
}

#[test]
fn clearing_a_selection_in_the_menu_can_be_undone() {
    let mut app = creative_app();
    let screen = (1280, 720);
    open_save_page(&mut app);
    let tool = &mut app.game.as_mut().unwrap().world_tools.selection;
    tool.selection.region([0, 0, 0], [2, 2, 2], false).unwrap();
    tool.set_pending_corner([4, 4, 4]);
    let button = cursor_over_widget(&mut app, screen, "clear_selection", None);
    app.set_cursor_position(button.0, button.1);
    app.click_screen_for_test(screen, 0.0);
    let tool = &mut app.game.as_mut().unwrap().world_tools.selection;
    assert!(tool.selection.is_empty());
    assert!(!tool.has_pending_corner());
    assert!(tool.selection.undo());
    assert_eq!(tool.selection.len(), 27);
}

fn schematic_fixture() -> petramond::schematic::Schematic {
    use petramond::schematic::{CellData, ResolvedCell, Schematic, SchematicCell};
    use petramond_world::block::{Block, ShapeState};
    Schematic::from_cells(
        "Workshop".into(),
        [1; 3],
        vec![SchematicCell {
            pos: [0; 3],
            data: CellData::capture(&ResolvedCell {
                block: Block::Stone,
                state: ShapeState::NONE,
                fluid: 0,
                kv: Default::default(),
                container: None,
                furnace: None,
            }),
        }],
    )
    .unwrap()
}

#[test]
fn schematic_deletion_waits_for_confirmation_and_keeps_the_confirmed_identity() {
    use petramond::schematic::library;
    let mut app = creative_app();
    let screen = (1280, 720);
    let dir = library::directory();
    let fixture = schematic_fixture();
    let png = petramond_world::assets::read_bytes("textures/schematic_wand.png")
        .unwrap()
        .0;
    let path = library::save(&dir, &fixture, &png).unwrap();
    let other = library::save(&dir, &fixture, &png).unwrap();
    let start = std::time::Instant::now();
    while !app
        .game()
        .schematic_library
        .entries()
        .iter()
        .any(|e| e.path == other)
    {
        app.game.as_mut().unwrap().poll_schematic_library();
        assert!(start.elapsed().as_secs_f32() < 3.0, "library did not load");
        std::thread::yield_now();
    }
    let entries = app.game.as_mut().unwrap().schematic_library.entries_mut();
    // Other tests save into the shared process library while this pointer test runs.
    entries.retain(|e| e.path == path || e.path == other);
    let target = entries.iter().position(|e| e.path == path).unwrap();
    open_library(&mut app);
    app.solve_menu_frame_for_test(screen);
    let scroll = app.ui.out().rect("creative_library_scroll").unwrap();
    app.set_cursor_position(
        (scroll.x + scroll.w / 2) as f32,
        (scroll.y + scroll.h / 2) as f32,
    );
    app.add_scroll_delta(-100.0);
    app.solve_menu_frame_for_test(screen);
    let delete = cursor_over_widget(&mut app, screen, "delete_schematic", Some(target as u32));
    app.set_cursor_position(delete.0, delete.1);
    app.click_screen_for_test(screen, 0.0);
    assert!(path.is_file());
    assert_eq!(app.library_form.pending_delete.as_ref().unwrap().path, path);
    let cancel = cursor_over_widget(&mut app, screen, "cancel_delete", None);
    app.set_cursor_position(cancel.0, cancel.1);
    app.click_screen_for_test(screen, 0.5);
    assert!(app.library_form.pending_delete.is_none());
    assert!(path.is_file());
    let delete = cursor_over_widget(&mut app, screen, "delete_schematic", Some(target as u32));
    app.set_cursor_position(delete.0, delete.1);
    app.click_screen_for_test(screen, 1.0);
    app.game
        .as_mut()
        .unwrap()
        .schematic_library
        .entries_mut()
        .reverse();
    let confirm = cursor_over_widget(&mut app, screen, "confirm_delete", None);
    app.set_cursor_position(confirm.0, confirm.1);
    app.click_screen_for_test(screen, 1.5);
    let start = std::time::Instant::now();
    while path.exists() {
        app.game.as_mut().unwrap().poll_schematic_library();
        assert!(
            start.elapsed().as_secs_f32() < 3.0,
            "confirmed deletion did not complete"
        );
        std::thread::yield_now();
    }
    assert!(other.is_file());
    library::delete(&other).unwrap();
}

#[test]
fn schematic_save_returns_to_the_library_only_after_success() {
    use petramond::schematic::library;
    let mut app = creative_app();
    let png = petramond_world::assets::read_bytes("textures/schematic_wand.png")
        .unwrap()
        .0;
    let screen = (1280, 720);
    // (save succeeds, the player left for the Items tab meanwhile)
    for (success, left) in [(true, false), (false, false), (true, true)] {
        let mut fixture = schematic_fixture();
        fixture.name = format!("Save navigation {success} {left}");
        open_save_page(&mut app);
        app.library_form.name = fixture.name.clone();
        let game = app.game.as_mut().unwrap();
        game.schematic_captured(std::sync::Arc::new(fixture.clone()));
        // The first poll may spend itself listing the library.
        let pending = (0..4)
            .find_map(|_| {
                game.poll_schematic_library();
                game.schematic_library.pending_save()
            })
            .expect("the capture waits for its thumbnail");
        app.solve_menu_frame_for_test(screen);
        assert!(on_save_page(&app), "capture is not a completed save");
        if left {
            open_items(&mut app);
            app.library_form.page = LibraryPage::Save;
        }
        let thumbnail = if success {
            Ok(png.clone())
        } else {
            Err("Save failed".to_string())
        };
        let game = app.game.as_mut().unwrap();
        let jobs = game.jobs().clone();
        game.schematic_library
            .start_save(&jobs, pending, move || thumbnail);
        game.poll_schematic_library();
        let saved = game
            .schematic_library
            .entries()
            .iter()
            .find(|e| e.metadata.name == fixture.name)
            .cloned();
        match saved {
            Some(entry) => {
                assert!(success);
                assert_eq!(library::read(&entry.path).unwrap(), fixture);
                library::delete(&entry.path).unwrap();
            }
            None => {
                assert!(!success);
                assert_eq!(game.notice, "Save failed");
            }
        }
        app.solve_menu_frame_for_test(screen);
        if left {
            assert!(
                app.creative_menu.tab == CreativeTab::Items,
                "the player left Save"
            );
        } else {
            assert_eq!(on_library_page(&app), success);
            assert_eq!(on_save_page(&app), !success);
        }
        assert_eq!(app.library_form.name, fixture.name);
    }
}

#[test]
#[ignore = "manual native creative-menu screenshot and pointer playthrough"]
fn creative_menu_visual_check() {
    let mut app = creative_app();
    let screen = (1280, 960);
    let mut renderer = pollster::block_on(petramond_render::new_offscreen_renderer(
        screen.0,
        screen.1,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ));
    let dir = std::path::PathBuf::from(
        std::env::var_os("PETRAMOND_CREATIVE_QA").expect("capture directory"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let capture = |app: &mut TestApp, renderer: &mut petramond_render::Renderer, name: &str| {
        for _ in 0..3 {
            if app.doc_ui_kind().is_some() {
                app.solve_menu_frame_for_test(screen);
            }
            app.render(renderer);
        }
        let frame = renderer.capture_frame();
        image::save_buffer(
            dir.join(name),
            &frame.rgba,
            frame.width,
            frame.height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    };
    let source = cursor_over_widget(&mut app, screen, "creative_item", Some(2));
    app.set_cursor_position(source.0, source.1);
    capture(&mut app, &mut renderer, "creative-tooltip.png");
    app.click_screen_for_test(screen, 0.0);
    let target = cursor_over_slot(&mut app, screen, 5);
    app.set_cursor_position(target.0, target.1 - 50.0);
    capture(&mut app, &mut renderer, "creative-drag.png");
    app.set_cursor_position(target.0, target.1);
    app.click_screen_for_test(screen, 0.5);
    settle_actions(&mut app);
    capture(&mut app, &mut renderer, "creative-drop.png");
    open_save_page(&mut app);
    app.game
        .as_mut()
        .unwrap()
        .world_tools
        .selection
        .selection
        .region([0, 0, 0], [4, 3, 4], false)
        .unwrap();
    app.library_form.name = "Workshop".into();
    app.set_cursor_position(0.0, 0.0);
    let checkbox = cursor_over_widget(&mut app, screen, "include_air", None);
    app.set_cursor_position(checkbox.0, checkbox.1);
    app.click_screen_for_test(screen, 1.0);
    assert!(app.library_form.include_air);
    app.set_cursor_position(0.0, 0.0);
    capture(&mut app, &mut renderer, "creative-save.png");
    let save = cursor_over_widget(&mut app, screen, "save_schematic", None);
    app.set_cursor_position(save.0, save.1);
    capture(&mut app, &mut renderer, "creative-save-hover.png");
    use petramond::schematic::library;
    let entries = library::list(&dir).unwrap();
    let source = entries
        .iter()
        .find(|e| e.metadata.name == "Townhouse")
        .unwrap_or(&entries[0]);
    let mut saved = library::read(&source.path).unwrap();
    saved.name = "Native save verification".into();
    if let Ok(repeat) = std::env::var("PETRAMOND_CREATIVE_REPEAT") {
        let repeat: i32 = repeat.parse().unwrap();
        assert!((1..=16).contains(&repeat));
        let mut builder = petramond::schematic::SchematicBuilder::default();
        let [sx, sy, sz] = saved.size;
        for x in 0..repeat {
            for z in 0..repeat {
                for cell in saved.cells() {
                    builder
                        .insert(
                            [
                                cell.pos[0] + x * (sx + 2),
                                cell.pos[1],
                                cell.pos[2] + z * (sz + 2),
                            ],
                            cell.data.clone(),
                        )
                        .unwrap();
                }
            }
        }
        saved = builder
            .finish(
                saved.name.clone(),
                [(sx + 2) * repeat - 2, sy, (sz + 2) * repeat - 2],
            )
            .unwrap();
        eprintln!(
            "Native save fixture: {} cells, {:?}",
            saved.cell_count(),
            saved.size
        );
    }
    app.game
        .as_mut()
        .unwrap()
        .schematic_captured(std::sync::Arc::new(saved.clone()));
    let start = std::time::Instant::now();
    let saved_entry = loop {
        app.solve_menu_frame_for_test(screen);
        app.render(&mut renderer);
        if let Some(entry) = app
            .game()
            .schematic_library
            .entries()
            .iter()
            .find(|e| e.metadata.name == saved.name)
        {
            break entry.clone();
        }
        assert!(
            start.elapsed().as_secs_f32() < 30.0,
            "native save did not finish: {}",
            app.game().notice
        );
        std::thread::yield_now();
    };
    assert!(
        on_library_page(&app),
        "successful save returns to the library"
    );
    capture(&mut app, &mut renderer, "creative-after-save.png");
    assert_eq!(library::read(&saved_entry.path).unwrap(), saved);
    let preview = library::thumbnail(&saved_entry).unwrap();
    assert!(preview.rgba.chunks_exact(4).any(|p| p[3] != 0));
    assert!(!saved_entry.path.with_extension("png").exists());
    std::fs::write(
        dir.join("saved-native.llschematic"),
        std::fs::read(&saved_entry.path).unwrap(),
    )
    .unwrap();
    library::delete(&saved_entry.path).unwrap();
    *app.game.as_mut().unwrap().schematic_library.entries_mut() = entries;
    open_library(&mut app);
    let selected = app
        .game()
        .schematic_library
        .entries()
        .iter()
        .position(|e| e.metadata.name == "Townhouse")
        .unwrap_or(0);
    let card = cursor_over_widget(&mut app, screen, "schematic_card", Some(selected as u32));
    app.set_cursor_position(card.0, card.1);
    app.click_screen_for_test(screen, 1.1);
    assert!(!app.game().schematic_preview.is_up());
    assert!(app.library_form.pending_delete.is_none());
    let start = std::time::Instant::now();
    while app
        .game()
        .schematic_library
        .thumbnail(&app.game().schematic_library.entries()[selected])
        .is_none()
    {
        app.solve_menu_frame_for_test(screen);
        assert!(
            start.elapsed().as_secs_f32() < 5.0,
            "embedded thumbnail did not load"
        );
        std::thread::yield_now();
    }
    app.set_cursor_position(0.0, 0.0);
    capture(&mut app, &mut renderer, "creative-library.png");
    app.set_cursor_position(card.0, card.1);
    capture(&mut app, &mut renderer, "creative-card-hover.png");
    let add = cursor_over_widget(&mut app, screen, "new_schematic", None);
    app.set_cursor_position(add.0, add.1);
    capture(&mut app, &mut renderer, "creative-add-hover.png");
    let delete = cursor_over_widget(&mut app, screen, "delete_schematic", Some(selected as u32));
    app.set_cursor_position(delete.0, delete.1);
    capture(&mut app, &mut renderer, "creative-delete-hover.png");
    app.click_screen_for_test(screen, 1.15);
    app.set_cursor_position(0.0, 0.0);
    capture(&mut app, &mut renderer, "creative-delete-confirmation.png");
    let cancel = cursor_over_widget(&mut app, screen, "cancel_delete", None);
    app.set_cursor_position(cancel.0, cancel.1);
    app.click_screen_for_test(screen, 1.16);
    let preview = cursor_over_widget(&mut app, screen, "place_schematic", Some(selected as u32));
    app.set_cursor_position(preview.0, preview.1);
    capture(&mut app, &mut renderer, "creative-place-hover.png");
    app.click_screen_for_test(screen, 1.2);
    let start = std::time::Instant::now();
    while !app.screen.gameplay_enabled() {
        app.solve_menu_frame_for_test(screen);
        assert!(
            start.elapsed().as_secs_f32() < 3.0,
            "schematic did not load"
        );
        std::thread::yield_now();
    }
    assert!(app.screen.gameplay_enabled());
    assert!(app.game().schematic_preview.is_up());
    app.game.as_mut().unwrap().cancel_world_tools();
    app.handle_control(Control::ToggleInventory, true);
    app.handle_control(Control::ToggleInventory, false);
    open_items(&mut app);
    app.solve_menu_frame_for_test(screen);
    let source = cursor_over_widget(&mut app, screen, "creative_item", Some(2));
    app.set_cursor_position(source.0, source.1);
    app.add_scroll_delta(8.0);
    capture(&mut app, &mut renderer, "creative-scroll.png");
    let catalog = ItemType::all()
        .iter()
        .filter(|i| i.creative_visible())
        .map(|i| format!("{}\t{}", i.registry_name(), i.name()))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("catalog.tsv"), catalog).unwrap();
    for (query, filename) in [
        ("fallen branch", "creative-branches.png"),
        ("rail", "creative-rails.png"),
        ("forge:", "creative-forge.png"),
    ] {
        app.creative_menu.query = query.into();
        app.ui.ensure_active(GuiKind::Hotbar);
        app.set_cursor_position(0.0, 0.0);
        capture(&mut app, &mut renderer, filename);
    }
    app.close_screen();
    capture(&mut app, &mut renderer, "creative-hotbar.png");
    let wand = ItemType::by_name("petramond:schematic_wand").unwrap();
    app.add_to_inventory(ItemStack::new(wand, 1));
    let slot = (0..9)
        .find(|i| app.inventory().slot(*i).is_some_and(|s| s.item == wand))
        .unwrap();
    app.game.as_mut().unwrap().set_active_hotbar(slot as u8);
    capture(&mut app, &mut renderer, "creative-wand-mode.png");
    std::thread::sleep(std::time::Duration::from_millis(2450));
    capture(&mut app, &mut renderer, "creative-wand-fade.png");
    std::thread::sleep(std::time::Duration::from_millis(650));
    capture(&mut app, &mut renderer, "creative-wand-expired.png");
    app.game.as_mut().unwrap().adjust_tool(1);
    capture(&mut app, &mut renderer, "creative-wand-cell.png");
}

#[test]
fn ctrl_scroll_cycles_the_held_wand_without_changing_hotbar_slots() {
    use crate::game::selection_tool::SelectionMode;
    use petramond_world::controls::Modifiers;
    let mut app = creative_app();
    let wand = ItemType::by_name("petramond:schematic_wand").unwrap();
    app.add_to_inventory(ItemStack::new(wand, 1));
    app.close_screen();
    let slot = (0..9)
        .find(|i| app.inventory().slot(*i).is_some_and(|s| s.item == wand))
        .unwrap();
    app.game.as_mut().unwrap().set_active_hotbar(slot as u8);
    app.set_modifiers(Modifiers {
        ctrl: true,
        ..Default::default()
    });
    app.add_scroll_delta(0.5);
    assert!(app.game().world_tools.selection.mode() == SelectionMode::Region);
    app.add_scroll_delta(0.5);
    assert!(app.game().world_tools.selection.mode() == SelectionMode::Cell);
    app.add_scroll_delta(-1.0);
    assert!(app.game().world_tools.selection.mode() == SelectionMode::Region);
    app.add_scroll_delta(-1.0);
    assert!(app.game().world_tools.selection.mode() == SelectionMode::Extrude);
    app.add_scroll_delta(1.0);
    assert!(app.game().world_tools.selection.mode() == SelectionMode::Region);
    assert_eq!(app.input.take_hotbar_steps(), 0);
    assert_eq!(
        app.game().menu_read_model().inventory.active_slot(),
        slot as u8
    );
}

#[test]
fn asynchronous_schematic_loads_respect_replacement_and_menu_cancellation() {
    use petramond::schematic::library;
    let mut app = creative_app();
    let png = petramond_world::assets::read_bytes("textures/schematic_wand.png")
        .unwrap()
        .0;
    let first = library::save(&library::directory(), &schematic_fixture(), &png).unwrap();
    let bad = library::save(&library::directory(), &schematic_fixture(), &png).unwrap();
    let mut fixture = schematic_fixture();
    fixture.name = "Chosen structure".into();
    let chosen = library::save(&library::directory(), &fixture, &png).unwrap();
    let start = std::time::Instant::now();
    while !app
        .game()
        .schematic_library
        .entries()
        .iter()
        .any(|e| e.path == chosen)
    {
        app.game.as_mut().unwrap().poll_schematic_library();
        assert!(start.elapsed().as_secs_f32() < 3.0);
        std::thread::yield_now();
    }
    std::fs::write(&bad, b"damaged file").unwrap();
    let index = |app: &TestApp, path: &std::path::Path| {
        app.game()
            .schematic_library
            .entries()
            .iter()
            .position(|e| e.path == path)
            .unwrap()
    };
    let target = index(&app, &chosen);
    for path in [&first, &bad] {
        let old = index(&app, path);
        let game = app.game.as_mut().unwrap();
        game.cancel_world_tools();
        game.schematic_preview
            .begin_paste(std::sync::Arc::new(fixture.clone()));
        assert!(game.raise_schematic_preview(7));
        game.begin_schematic_paste(old);
        game.poll_schematic_library();
        game.begin_schematic_paste(target);
        let start = std::time::Instant::now();
        while !game.take_paste_preview_ready() {
            game.poll_schematic_library();
            assert!(
                start.elapsed().as_secs_f32() < 3.0,
                "newer load was discarded"
            );
            std::thread::yield_now();
        }
        assert_eq!(
            game.schematic_preview.schematic().map(|s| &**s),
            Some(&fixture)
        );
        assert_eq!(
            game.schematic_preview.vertical_offset(),
            0,
            "a newly loaded schematic resets height"
        );
        assert!(game.notice.is_empty());
    }
    app.game.as_mut().unwrap().cancel_world_tools();
    app.game.as_mut().unwrap().begin_schematic_paste(target);
    app.game.as_mut().unwrap().poll_schematic_library();
    app.close_screen();
    let game = app.game.as_mut().unwrap();
    game.schematic_library.request_thumbnails(&[target]);
    let entry = game.schematic_library.entries()[target].clone();
    let start = std::time::Instant::now();
    while game.schematic_library.thumbnail(&entry).is_none() {
        game.poll_schematic_library();
        assert!(start.elapsed().as_secs_f32() < 3.0);
        std::thread::yield_now();
    }
    assert!(!game.schematic_preview.is_up());
    assert!(!game.take_paste_preview_ready());
    for path in [first, bad, chosen] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn ctrl_scroll_raises_and_lowers_the_preview_before_wand_or_hotbar_bindings() {
    use crate::game::selection_tool::SelectionMode;
    use petramond_world::controls::Modifiers;
    let mut app = creative_app();
    app.close_screen();
    app.game
        .as_mut()
        .unwrap()
        .schematic_preview
        .begin_paste(std::sync::Arc::new(schematic_fixture()));
    app.set_modifiers(Modifiers {
        ctrl: true,
        ..Default::default()
    });
    app.add_scroll_delta(-0.5);
    assert_eq!(app.game().schematic_preview.vertical_offset(), 0);
    app.add_scroll_delta(-0.5);
    assert_eq!(app.game().schematic_preview.vertical_offset(), 1);
    app.add_scroll_delta(2.0);
    assert_eq!(app.game().schematic_preview.vertical_offset(), -1);
    assert_eq!(app.input.take_hotbar_steps(), 0);
    app.set_modifiers(Modifiers::default());
    app.add_scroll_delta(-1.0);
    assert_eq!(app.input.take_hotbar_steps(), -1);
    assert_eq!(app.game().schematic_preview.vertical_offset(), -1);
    let wand = ItemType::by_name("petramond:schematic_wand").unwrap();
    app.add_to_inventory(ItemStack::new(wand, 1));
    let slot = (0..9)
        .find(|i| app.inventory().slot(*i).is_some_and(|s| s.item == wand))
        .unwrap();
    app.game.as_mut().unwrap().set_active_hotbar(slot as u8);
    app.set_modifiers(Modifiers {
        ctrl: true,
        ..Default::default()
    });
    app.add_scroll_delta(-1.0);
    assert_eq!(app.game().schematic_preview.vertical_offset(), 0);
    assert!(app.game().world_tools.selection.mode() == SelectionMode::Region);
    assert_eq!(app.input.take_hotbar_steps(), 0);
    app.handle_control(Control::ToggleInventory, true);
    app.handle_control(Control::ToggleInventory, false);
    app.add_scroll_delta(-1.0);
    assert_eq!(
        app.game().schematic_preview.vertical_offset(),
        0,
        "menu scrolling must not move the preview"
    );
    app.close_screen();
    app.game.as_mut().unwrap().cancel_world_tools();
    app.add_scroll_delta(1.0);
    assert!(app.game().world_tools.selection.mode() == SelectionMode::Cell);
    assert_eq!(app.game().schematic_preview.vertical_offset(), 0);
}
