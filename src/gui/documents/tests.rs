use super::*;

mod fit;

#[test]
fn engine_contracts_cover_every_container_kind() {
    for (kind, total) in [
        (GuiKind::Chest, 27 + 27 + 9),
        (GuiKind::Inventory, 27 + 9 + 1 + 1),
        (GuiKind::CraftingTable, 27 + 9 + 1),
        (GuiKind::Furnace, 27 + 9 + 3),
        (GuiKind::Hotbar, 9 + 1),
    ] {
        let contract = contract_for(kind);
        let sum: usize = contract.roles.iter().map(|(_, n)| n).sum();
        assert_eq!(sum, total, "{kind:?}");
    }
    assert!(contract_for(GuiKind::Pause).roles.is_empty());
}

#[test]
fn the_shared_kind_table_is_the_engine_kind_registry() {
    let table: Vec<&str> = petramond_ui::contract::ENGINE_KINDS
        .iter()
        .map(|k| k.key)
        .collect();
    assert_eq!(table, petramond_world::gui_state::engine_kind_keys());
    for key in table {
        let kind = crate::gui::intern_kind(key).expect("engine key interns");
        assert!(!kind.is_registered(), "{key} is an engine kind");
    }
    assert_eq!(
        petramond_ui::contract::ENGINE_NAMESPACE,
        petramond_world::registry::ENGINE_NAMESPACE
    );
}

#[test]
fn shipped_documents_use_their_table_class() {
    for kind in petramond_ui::contract::ENGINE_KINDS {
        let gui_kind = crate::gui::intern_kind(kind.key).expect("engine key interns");
        let Some(doc) = doc_entry_for(gui_kind) else {
            continue;
        };
        assert_eq!(doc.doc.class, kind.class, "{}", kind.key);
    }
}

#[test]
fn bindings_catalog_parses_and_covers_controller_kinds() {
    let (text, _) = petramond_world::assets::read_base_text("ui/bindings.json")
        .expect("bindings catalog ships");
    let v: serde_json::Value = serde_json::from_str(&text).expect("catalog is valid JSON");
    let kinds = v["kinds"].as_object().expect("catalog has kinds");
    for key in [
        "petramond:title",
        "petramond:world_select",
        "petramond:world_settings",
        "petramond:create_world",
        "petramond:delete_world",
        "petramond:pause",
        "petramond:sleep",
        "petramond:death",
        "petramond:hotbar",
        "petramond:furnace",
        "petramond:connect_server",
        "petramond:account",
        "petramond:account_sign_in",
        "petramond:content",
        "petramond:mods_missing",
        "petramond:connection_lost",
    ] {
        assert!(
            kinds.contains_key(key),
            "{key} missing from bindings catalog"
        );
    }
}

#[test]
fn overlay_documents_ship_and_validate() {
    for kind in [GuiKind::Sleep, GuiKind::Death] {
        assert!(doc_for(kind).is_some(), "{kind:?} document loads");
    }
}

#[test]
fn crafting_browser_documents_ship_and_validate() {
    for kind in [GuiKind::Inventory, GuiKind::CraftingTable] {
        let doc = doc_for(kind).unwrap_or_else(|| panic!("{kind:?} document loads"));
        assert!(
            doc.doc
                .role_slots()
                .iter()
                .all(|(role, _)| role != "craft_input"),
            "the removed grid role must not return"
        );
    }
}

#[test]
fn crafting_table_browser_shrinks_before_inventory_leaves_the_viewport() {
    use petramond_ui::{
        FrameArgs, FrameOutput, FrameState, NoImages, UiMap, UiRuntime, UiState, UiValue,
    };

    let doc = doc_for(GuiKind::CraftingTable).expect("crafting table document loads");
    let rows = (0..12)
        .map(|_| {
            let mut row = UiMap::new();
            row.insert("name".into(), UiValue::Str("Recipe".into()));
            row.insert("enabled".into(), UiValue::Bool(true));
            row
        })
        .collect();
    let mut state = UiState::new();
    state.set("craft_recipes", UiValue::List(Arc::new(rows)));
    state.set("no_craft_results", UiValue::Bool(false));
    state.set("can_craft", UiValue::Bool(false));

    let runtime = UiRuntime::new(doc.doc, crate::gui::doc_theme::theme());
    let mut frame_state = FrameState::new();
    let mut output = FrameOutput::default();
    runtime.frame(
        FrameArgs {
            screen: (1280, 720),
            scale: 3,
            now: 0.0,
            state: &state,
            input: &[],
            clipboard: None,
            images: &NoImages,
            dim: None,
            preview: None,
        },
        &mut frame_state,
        &mut output,
    );

    assert!(
        output
            .slots
            .iter()
            .all(|slot| slot.rect.y >= 0 && slot.rect.y + slot.rect.h <= 720),
        "responsive recipe scroll must keep every inventory slot on-screen"
    );
}

#[test]
fn inventory_browser_shrinks_before_inventory_leaves_the_viewport() {
    use petramond_ui::{
        FrameArgs, FrameOutput, FrameState, NoImages, UiMap, UiRuntime, UiState, UiValue,
    };

    let doc = doc_for(GuiKind::Inventory).expect("inventory document loads");
    let rows = (0..12)
        .map(|_| {
            let mut row = UiMap::new();
            row.insert(
                "name".into(),
                UiValue::Str("An intentionally long recipe name".into()),
            );
            row.insert("enabled".into(), UiValue::Bool(true));
            row
        })
        .collect();
    let mut state = UiState::new();
    state.set("craft_recipes", UiValue::List(Arc::new(rows)));
    state.set("no_craft_results", UiValue::Bool(false));
    state.set("can_craft", UiValue::Bool(false));

    let runtime = UiRuntime::new(doc.doc, crate::gui::doc_theme::theme());
    let mut frame_state = FrameState::new();
    let mut output = FrameOutput::default();
    runtime.frame(
        FrameArgs {
            screen: (960, 720),
            scale: 3,
            now: 0.0,
            state: &state,
            input: &[],
            clipboard: None,
            images: &NoImages,
            dim: None,
            preview: None,
        },
        &mut frame_state,
        &mut output,
    );

    assert!(
        output
            .slots
            .iter()
            .all(|slot| slot.rect.x >= 0 && slot.rect.x + slot.rect.w <= 960),
        "responsive recipe panel must keep every inventory slot on-screen"
    );
    assert!(
        output
            .slots
            .iter()
            .all(|slot| slot.rect.y >= 0 && slot.rect.y + slot.rect.h <= 720),
        "the compact stacked form must keep every inventory slot on-screen"
    );
}

#[test]
fn documents_resolve_images_beside_the_document() {
    let doc = doc_for(GuiKind::CreateWorld).expect("create_world document loads");
    assert!(
        !doc.images.is_empty(),
        "the screenshot backdrop resolves beside the document"
    );
}

fn show_item_tip_nodes(node: &petramond_ui::Node) -> usize {
    usize::from(node.bind.visible.as_deref() == Some("show_item_tip"))
        + node.children.iter().map(show_item_tip_nodes).sum::<usize>()
}

#[test]
fn container_documents_get_the_item_tooltip_injected_at_load() {
    let mut container = Document::from_json(
        r#"{ "format": 1, "kind": "doctest:c", "class": "container",
             "root": { "type": "frame" } }"#,
    )
    .unwrap();
    inject_item_tooltip(&mut container);
    assert_eq!(show_item_tip_nodes(&container.root), 1);
    inject_item_tooltip(&mut container);
    assert_eq!(
        show_item_tip_nodes(&container.root),
        1,
        "injection is idempotent (and yields to a document's own chrome)"
    );

    let mut screen = Document::from_json(
        r#"{ "format": 1, "kind": "doctest:s", "class": "screen",
             "root": { "type": "frame" } }"#,
    )
    .unwrap();
    inject_item_tooltip(&mut screen);
    assert_eq!(
        show_item_tip_nodes(&screen.root),
        0,
        "screens carry no slot chrome"
    );

    for key in [
        "petramond:inventory",
        "petramond:crafting_table",
        "petramond:chest",
        "petramond:furnace",
    ] {
        let kind = crate::gui::intern_kind(key).expect("kind interns");
        let doc = doc_for(kind).unwrap_or_else(|| panic!("{key} document loads"));
        assert_eq!(show_item_tip_nodes(&doc.doc.root), 1, "{key}");
    }
}

#[test]
fn container_documents_get_the_slot_tooltip_injected_at_load() {
    fn show_slot_tip_nodes(node: &Node) -> usize {
        usize::from(node.bind.visible.as_deref() == Some(SHOW_SLOT_TIP))
            + node.children.iter().map(show_slot_tip_nodes).sum::<usize>()
    }
    let mut container = Document::from_json(
        r#"{ "format": 1, "kind": "doctest:c", "class": "container",
             "root": { "type": "frame" } }"#,
    )
    .unwrap();
    inject_slot_tooltip(&mut container);
    inject_slot_tooltip(&mut container);
    assert_eq!(show_slot_tip_nodes(&container.root), 1, "idempotent");
    let mut screen = Document::from_json(
        r#"{ "format": 1, "kind": "doctest:s", "class": "screen",
             "root": { "type": "frame" } }"#,
    )
    .unwrap();
    inject_slot_tooltip(&mut screen);
    assert_eq!(show_slot_tip_nodes(&screen.root), 0, "screens carry none");
}

#[test]
fn the_injected_slot_tooltip_binds_exactly_the_published_span_keys() {
    let mut doc = Document::from_json(
        r#"{ "format": 1, "kind": "doctest:c", "class": "container",
             "root": { "type": "frame" } }"#,
    )
    .unwrap();
    inject_slot_tooltip(&mut doc);
    let mut bound: Vec<(String, String)> = Vec::new();
    doc.root.visit(&mut |node| {
        if let (Some(text), Some(palette)) = (&node.bind.text, &node.bind.palette) {
            bound.push((text.clone(), palette.clone()));
        }
    });
    let expected: Vec<(String, String)> = (0..SLOT_TIP_LINES)
        .flat_map(|line| (0..SLOT_TIP_SPANS).map(move |span| slot_tip_keys(line, span)))
        .collect();
    assert_eq!(bound, expected);
}

#[test]
fn a_slot_may_not_declare_more_filters_than_the_mask_has_bits() {
    let doc = |n: usize| {
        let accepts: Vec<String> = (0..n)
            .map(|i| format!(r#"{{"data": "doctest:f{i}"}}"#))
            .collect();
        Document::from_json(&format!(
            r#"{{ "format": 1, "kind": "doctest:machine", "class": "container",
                 "root": {{ "type": "slot", "role": "container", "accepts": [{}] }} }}"#,
            accepts.join(", ")
        ))
        .expect("test document parses")
    };
    let specs = doc_container_specs(&doc(MAX_SLOT_FILTERS)).expect("a full mask's worth loads");
    assert_eq!(specs[0].accepts.len(), MAX_SLOT_FILTERS);
    let err = doc_container_specs(&doc(MAX_SLOT_FILTERS + 1)).unwrap_err();
    assert!(err.contains("accepts filters; the cap is"), "{err}");
}

#[test]
fn a_registered_station_kind_falls_back_to_the_crafting_table_document() {
    let kind = crate::gui::intern_kind("doctest:bench_station").unwrap();
    assert!(
        doc_for(kind).is_none(),
        "an unregistered mod kind has no document"
    );
    petramond_world::crafting::CraftingStation::from_key("doctest:bench_station")
        .expect("station registers");
    let doc = doc_for(kind).expect("station kinds are backed by the crafting table document");
    assert_eq!(doc.doc.kind, "petramond:crafting_table");
    let doc = doc_for(GuiKind::FurnitureWorkbench).expect("furniture workbench screen loads");
    assert_eq!(doc.doc.kind, "petramond:crafting_table");
}

#[test]
fn a_misspelled_accepts_tag_is_a_document_error() {
    let doc = |accepts: &str| {
        Document::from_json(&format!(
            r#"{{ "format": 1, "kind": "doctest:machine", "class": "container",
                 "root": {{ "type": "slot", "role": "container", "accepts": ["{accepts}"] }} }}"#
        ))
        .expect("test document parses")
    };
    assert!(doc_container_specs(&doc("petramond:fuel")).is_ok());
    let err = doc_container_specs(&doc("doctest:no_such_tag")).unwrap_err();
    assert!(err.contains("unknown item tag"), "{err}");
}

#[test]
fn a_slot_may_accept_a_data_key_and_it_must_be_namespaced() {
    let doc = |accepts: &str| {
        Document::from_json(&format!(
            r#"{{ "format": 1, "kind": "doctest:machine", "class": "container",
                 "root": {{ "type": "slot", "role": "container", "accepts": [{accepts}] }} }}"#
        ))
        .expect("test document parses")
    };
    let specs = doc_container_specs(&doc(r#"{"data": "doctest:metal"}"#))
        .expect("an unheard-of data key is legal");
    assert_eq!(
        specs[0].accepts,
        vec![petramond_world::container::SlotFilter::Data(
            "doctest:metal"
        )]
    );
    let err = doc_container_specs(&doc(r#"{"data": "metal"}"#)).unwrap_err();
    assert!(err.contains("namespaced"), "{err}");

    let specs = doc_container_specs(&doc(r#""petramond:fuel", {"data": "doctest:metal"}"#))
        .expect("mixed accepts resolve");
    assert_eq!(specs[0].accepts.len(), 2);
}

#[test]
fn foreign_namespace_documents_are_rejected_per_pack() {
    use petramond_ui::contract::kind_permitted;
    let kind = crate::gui::intern_kind("doctest:owned").unwrap();
    let key = crate::gui::kind_key(kind).unwrap();
    assert!(kind.is_registered());
    assert!(kind_permitted(key, Some("doctest")).is_ok());
    assert!(kind_permitted(key, Some("otherpack")).is_err());
    assert!(kind_permitted(key, None).is_err());
    for engine in [GuiKind::Furnace, GuiKind::Title] {
        let key = crate::gui::kind_key(engine).unwrap();
        assert!(kind_permitted(key, None).is_ok());
        assert!(kind_permitted(key, Some("anypack")).is_ok());
    }
}

fn test_art_dir(tag: &str) -> petramond_util::test_dirs::TestScratchDir {
    petramond_util::test_dirs::TestScratchDir::new(&format!("gui-doc-art-{tag}"))
}

fn write_png(path: &std::path::Path, w: u32, h: u32) {
    image::RgbaImage::new(w, h).save(path).expect("png writes");
}

fn art_doc(root: &str) -> Document {
    Document::from_json(&format!(
        r#"{{ "format": 1, "kind": "doctest:art", "class": "screen", "root": {root} }}"#
    ))
    .expect("test document parses")
}

#[test]
fn button_images_are_collected_beside_the_document() {
    let dir = test_art_dir("button-collect");
    write_png(&dir.join("flame.png"), 8, 4);
    write_png(&dir.join("go.png"), 6, 3);
    let doc = art_doc(
        r#"{ "type": "column", "children": [
              { "type": "image", "image": "flame.png", "frames": [2, 2] },
              { "type": "button", "id": "go", "image": "go.png", "frames": [3, 1] }
            ] }"#,
    );
    let images = collect_doc_images(&doc, &dir).expect("both sheets resolve");
    let names: Vec<&str> = images.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["flame.png", "go.png"]);
    assert_eq!(images[0].size, (8, 4));
    assert_eq!(images[1].size, (6, 3));
}

#[test]
fn missing_static_art_rejects_but_bound_images_are_fileless() {
    let dir = test_art_dir("missing");
    let err = collect_doc_images(
        &art_doc(r#"{ "type": "image", "image": "nope.png" }"#),
        &dir,
    )
    .unwrap_err();
    assert!(err.contains("missing art nope.png"), "{err}");
    let bound = collect_doc_images(
        &art_doc(r#"{ "type": "image", "image": "", "bind": { "image": "icon" } }"#),
        &dir,
    )
    .expect("a runtime-bound image needs no file");
    assert!(bound.is_empty());
}

#[test]
fn bad_frame_sheets_reject_the_document() {
    let dir = test_art_dir("frames");
    write_png(&dir.join("ok.png"), 8, 4);
    write_png(&dir.join("ragged.png"), 10, 4);
    write_png(&dir.join("many.png"), 65, 1);
    write_png(&dir.join("huge.png"), 700, 8);

    let doc = |image: &str, frames: &str| {
        art_doc(&format!(
            r#"{{ "type": "image", "image": "{image}", "frames": {frames} }}"#
        ))
    };
    collect_doc_images(&doc("ok.png", "[2, 2]"), &dir).expect("an even grid passes");
    collect_doc_images(
        &art_doc(r#"{ "type": "image", "image": "huge.png" }"#),
        &dir,
    )
    .expect("an unframed sheet skips the framed-sheet checks");

    let err = collect_doc_images(&doc("ragged.png", "[4, 1]"), &dir).unwrap_err();
    assert!(err.contains("does not divide evenly"), "{err}");
    let err = collect_doc_images(&doc("many.png", "[65, 1]"), &dir).unwrap_err();
    assert!(err.contains("frames; the cap is"), "{err}");
    let err = collect_doc_images(&doc("huge.png", "[7, 1]"), &dir).unwrap_err();
    assert!(err.contains("side cap"), "{err}");
}

#[test]
fn an_unframed_reference_does_not_hide_a_sheet_grid() {
    let dir = test_art_dir("launder");
    write_png(&dir.join("ragged.png"), 10, 4);
    let doc = art_doc(
        r#"{ "type": "column", "children": [
              { "type": "image", "image": "ragged.png" },
              { "type": "button", "id": "go", "image": "ragged.png", "frames": [4, 1] }
            ] }"#,
    );
    let err = collect_doc_images(&doc, &dir).unwrap_err();
    assert!(err.contains("does not divide evenly"), "{err}");
}

#[test]
fn creative_document_keeps_hotbar_slots_outside_its_tabs() {
    let doc = doc_for(GuiKind::Creative).expect("creative document loads and validates");
    assert_eq!(doc.doc.role_slots(), vec![("hotbar".to_string(), 9)]);
}

#[test]
fn the_title_launcher_dock_fits_zero_one_and_many_entries() {
    use petramond_ui::{solve, InstTree, RectI, ThemeEnv, UiMap, UiValue};
    let doc = doc_for(GuiKind::Title).expect("title document loads");
    let theme = crate::gui::doc_theme::theme();
    let font = theme.ui_font();
    let label = "M".repeat(petramond_world::assets::LAUNCH_LABEL_MAX);
    let viewport = (320, 240);
    let overlaps = |a: RectI, b: RectI| {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    };
    let inside = |a: RectI, b: RectI| {
        a.x >= b.x && a.y >= b.y && a.x + a.w <= b.x + b.w && a.y + a.h <= b.y + b.h
    };
    let mut failures = Vec::new();
    for scale in [1i32, 3] {
        for n in [0usize, 1, 3, 12] {
            let mut state = petramond_ui::UiState::new();
            let rows: Vec<UiMap> = (0..n)
                .map(|i| {
                    let mut row = UiMap::new();
                    row.insert("icon".into(), UiValue::Str(format!("launch_icon:p{i}")));
                    row
                })
                .collect();
            state.set("has_launchers", UiValue::Bool(n > 0));
            state.set("launchers", UiValue::List(Arc::new(rows)));
            state.set("launch_tip", UiValue::Str(label.clone()));
            let tree = InstTree::expand_form(&doc.doc, &state, doc.doc.compact_active(viewport.0));
            let env = ThemeEnv {
                theme: &theme,
                gui_scale: scale,
                image_size: &|_| None,
            };
            let solved = solve(&tree, &env, viewport, &|_| 0);
            let find = |id: &str| {
                (0..tree.len() as u32).find(|&i| tree.get(i).node.id.as_deref() == Some(id))
            };
            let what = format!("{n} entries @scale {scale}");
            let Some(list) = find("launchers") else {
                if n > 0 {
                    failures.push(format!("{what}: no dock"));
                }
                continue;
            };
            if n == 0 {
                failures.push(format!("{what}: a dock with nothing in it"));
                continue;
            }
            let scroll = tree
                .get(list)
                .parent
                .expect("the list sits in the dock's scroll");
            let panel = tree
                .get(scroll)
                .parent
                .expect("the scroll sits in the dock panel");
            let menu = tree.get(find("start").unwrap()).parent.unwrap();
            let screen = RectI {
                x: 0,
                y: 0,
                w: viewport.0,
                h: viewport.1,
            };
            let (panel_rect, scroll_rect) =
                (solved.rects[panel as usize], solved.rects[scroll as usize]);
            if overlaps(panel_rect, solved.rects[menu as usize]) || !inside(panel_rect, screen) {
                failures.push(format!(
                    "{what}: dock {panel_rect:?} is over the menu or off screen"
                ));
            }
            let stamps = &tree.get(list).children;
            let overflowing = stamps
                .iter()
                .any(|&c| !inside(solved.rects[c as usize], scroll_rect));
            let lane = if overflowing { 8 } else { 0 };
            for &c in stamps {
                let r = solved.rects[c as usize];
                if r.x < scroll_rect.x || r.x + r.w > scroll_rect.x + scroll_rect.w - lane {
                    failures.push(format!(
                        "{what}: button {r:?} runs under the scroll bar of {scroll_rect:?}"
                    ));
                }
            }
            let tip = (0..tree.len() as u32)
                .find(|&i| {
                    matches!(
                        tree.get(i).node.kind,
                        petramond_ui::NodeKind::Tooltip { .. }
                    )
                })
                .expect("the dock's tooltip");
            for &c in &tree.get(tip).children {
                let r = solved.rects[c as usize];
                let (_, need) = font.measure(&label, Some(r.w));
                if need > r.h {
                    failures.push(format!(
                        "{what}: the label needs {need}px, its box is {r:?}"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "launcher dock: {failures:#?}");
}
