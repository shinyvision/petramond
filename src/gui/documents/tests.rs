use super::*;

#[test]
fn engine_contracts_cover_every_container_kind() {
    // The contract is the load-time guard that keeps a bad document from
    // mis-routing clicks; every slot-bearing kind must pin its counts.
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
fn bindings_catalog_parses_and_covers_controller_kinds() {
    // assets/ui/bindings.json is the builder-facing data contract; keep
    // it shipping and covering every screen a controller populates.
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
    // The sleep and death overlays are engine screens: a document that
    // fails validation is skipped loudly at load and the screen would
    // draw (and route) nothing — pin that the shipped ones load.
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

/// The font's line box drives every document's vertical budget, so a font
/// swap (or one more label) must not push a shipped screen off the
/// smallest viewport the game scales to. Panels that legitimately scroll
/// absorb the difference; a panel that simply grew is a layout bug you
/// only see by opening that screen — and you see it as the Back button
/// sliced off the bottom edge, where nothing can click it.
///
/// Check every instance against ITS PARENT, not the root. The root is
/// `grow`, so it is the viewport by construction and an assertion against
/// it can never fail — that is how the Controls panel grew past the bottom
/// of the screen unnoticed. Parent-relative also catches the half of the
/// problem the screen edge hides: a tab page that outgrows its panel
/// paints its last row straight through the buttons below it.
///
/// Content inside a `scroll` is exempt — overflowing is what it is for.
#[test]
fn every_shipped_document_fits_the_smallest_viewport() {
    let mut overflowing = Vec::new();
    for scale in [1i32, 3] {
        for kind in SHELL_KINDS {
            walk_solved(*kind, scale, Seed::Ordinary, |n| {
                // Tooltips are placed by the runtime, which clamps them;
                // `abs` children are deliberately out of flow.
                if n.floating || n.rect.h == 0 {
                    return;
                }
                if n.inst.layout.abs.is_some() {
                    return;
                }
                let (top, bottom) = (n.parent.y, n.parent.y + n.parent.h);
                if (!n.scrolled && (n.rect.y < top || n.rect.y + n.rect.h > bottom))
                    || n.rect.x < n.parent.x
                    || n.rect.x + n.rect.w > n.parent.x + n.parent.w
                {
                    overflowing.push(format!(
                        "{kind:?} @scale {scale}: {:?} at {:?} outside parent content {:?}",
                        n.inst.node.kind, n.rect, n.parent,
                    ));
                }
            });
        }
    }
    assert!(
        overflowing.is_empty(),
        "documents overflow: {overflowing:#?}"
    );
}

/// The browser exists to stop the player scrolling and squinting, so the
/// grid has to actually show rows. Chrome added above it (a detail line, a
/// second label) comes straight out of this budget: the scroll is the only
/// grower, so it absorbs every pixel anything else takes.
#[test]
fn the_recipe_grid_shows_several_rows_at_the_smallest_viewport() {
    use petramond_ui::{
        FrameArgs, FrameOutput, FrameState, NoImages, UiMap, UiRuntime, UiState, UiValue,
    };
    let doc = doc_for(GuiKind::CraftingTable).expect("crafting table document loads");
    let rows: Vec<UiMap> = (0..120)
        .map(|_| {
            let mut row = UiMap::new();
            row.insert("enabled".into(), UiValue::Bool(true));
            row
        })
        .collect();
    let mut state = UiState::new();
    state.set("craft_recipes", UiValue::List(Arc::new(rows)));
    state.set("no_craft_results", UiValue::Bool(false));

    let screen = (1280u32, 720u32);
    let scale = crate::gui::gui_scale(screen) as i32;
    let runtime = UiRuntime::new(doc.doc, crate::gui::doc_theme::theme());
    let mut fs = FrameState::new();
    let mut out = FrameOutput::default();
    runtime.frame(
        FrameArgs {
            screen,
            scale,
            now: 0.0,
            state: &state,
            input: &[],
            clipboard: None,
            images: &NoImages,
            dim: None,
            preview: None,
        },
        &mut fs,
        &mut out,
    );

    // Count distinct cell rows actually inside the scroll viewport.
    let scroll = out.rect("craft_scroll").expect("scroll solves");
    let mut tops: Vec<i32> = out
        .named
        .iter()
        .filter(|(key, _)| key.id == "recipe")
        .map(|(_, r)| r.y)
        .filter(|y| *y >= scroll.y && *y < scroll.y + scroll.h)
        .collect();
    tops.sort_unstable();
    tops.dedup();
    // Three at 720p (the tightest case: the panel's 9-slice border alone
    // costs 24 logical px), more at 1080p where the viewport is taller.
    assert!(
        tops.len() >= 3,
        "only {} grid rows visible at 720p — the browser is being squeezed",
        tops.len()
    );
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
    // Below the compact breakpoint the two panels stack: crafting first,
    // then the inventory grids UNDER the CRAFT button, never beside it.
    let craft = output
        .named
        .iter()
        .find(|(key, _)| key.id == "craft")
        .expect("craft button solves")
        .1;
    assert!(
        output
            .slots
            .iter()
            .filter(|slot| slot.role == "player_inv" || slot.role == "hotbar")
            .all(|slot| slot.rect.y >= craft.y + craft.h),
        "small screens stack the crafting panel above the inventory panel"
    );
}

#[test]
fn documents_resolve_images_beside_the_document() {
    // create_world references its screenshot backdrop (and pixel.png
    // divider art elsewhere) relative to the document; loading must
    // resolve referenced images into the document's image table (they
    // feed TexId::DocImage by first-reference order).
    let doc = doc_for(GuiKind::CreateWorld).expect("create_world document loads");
    assert!(
        !doc.images.is_empty(),
        "the screenshot backdrop resolves beside the document"
    );
}

#[test]
fn minimap_client_documents_ship_and_validate() {
    for (key, class) in [
        ("minimap:create_waypoint", petramond_ui::DocClass::Screen),
        ("minimap:edit_waypoint", petramond_ui::DocClass::Screen),
    ] {
        let kind = crate::gui::intern_kind(key).expect("namespaced kind interns");
        let doc = doc_for(kind).unwrap_or_else(|| panic!("{key} document loads"));
        assert_eq!(doc.doc.class, class, "{key}");
    }
}

fn show_item_tip_nodes(node: &petramond_ui::Node) -> usize {
    usize::from(node.bind.visible.as_deref() == Some("show_item_tip"))
        + node.children.iter().map(show_item_tip_nodes).sum::<usize>()
}

/// The slot tooltip is engine chrome, not per-document copy: every
/// container-class document — an engine container's or a pack machine
/// panel's — gets exactly one injected at load, so a new GUI can never
/// forget it. A document binding `show_item_tip` itself keeps its own.
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

    // Shipped documents, engine and pack alike, come out of the registry
    // with the tooltip exactly once.
    for key in [
        "petramond:inventory",
        "petramond:crafting_table",
        "petramond:chest",
        "petramond:furnace",
        "forge:forging_furnace",
        "forge:anvil",
    ] {
        let kind = crate::gui::intern_kind(key).expect("kind interns");
        let doc = doc_for(kind).unwrap_or_else(|| panic!("{key} document loads"));
        assert_eq!(show_item_tip_nodes(&doc.doc.root), 1, "{key}");
    }
}

/// The BOUND slot tooltip injects like the item one: containers exactly
/// once (idempotent, yielding to a document's own chrome), screens not
/// at all.
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
    let kind = crate::gui::intern_kind("forge:anvil").expect("kind interns");
    let doc = doc_for(kind).expect("anvil document loads");
    assert_eq!(show_slot_tip_nodes(&doc.doc.root), 1, "forge:anvil");
}

/// The injected node binds exactly the `(text, palette)` pairs
/// [`slot_tip_keys`] names, in line-then-span order, for the whole
/// [`SLOT_TIP_LINES`] × [`SLOT_TIP_SPANS`] grid.
///
/// It cannot fail on a BUMP of either constant — node and expectation
/// derive from the same two numbers, which is the point of stating the
/// shape once. What it catches is the shape drifting from the naming:
/// a swapped `(line, span)` or `(text, palette)`, a row that forgot a
/// span, a key pattern changed in one place. The client half
/// (`item_tooltip.rs`) generates its keys through the same function and
/// cannot be reached from this crate, so its argument order is not
/// covered here.
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

/// A slot's runtime `accepts` mask spends one bit per authored filter, so
/// filter 33 could never be activated. Refuse it at load — a silently
/// inert filter is the failure this whole area exists to prevent.
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
    // The engine's furniture workbench ships no document of its own and
    // rides the same fallback.
    let doc = doc_for(GuiKind::FurnitureWorkbench).expect("furniture workbench screen loads");
    assert_eq!(doc.doc.kind, "petramond:crafting_table");
}

#[test]
fn a_misspelled_accepts_tag_is_a_document_error() {
    // The accepts check must stay on the non-interning lookup: the
    // interning `from_key` registers any namespaced typo as a fresh
    // empty tag, making this error unreachable.
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

/// A slot may name its group by row-DATA key as well as by tag, and the
/// two are validated by DIFFERENT rules on purpose: a tag has a registry
/// to be absent from, a data key has none (a row states one by carrying
/// it), so the only check a data key can carry is that it is namespaced.
/// Validating it like a tag would refuse every pack that ships its slot
/// before the rows that fill it.
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

    // Both forms in one list, since a slot admitting "fuel OR my metals"
    // is exactly the case the second form exists for.
    let specs = doc_container_specs(&doc(r#""petramond:fuel", {"data": "doctest:metal"}"#))
        .expect("mixed accepts resolve");
    assert_eq!(specs[0].accepts.len(), 2);
}

/// How long a value to seed every catalog `str` key with.
#[derive(Clone, Copy, PartialEq)]
enum Seed {
    /// A pack author's longest real summary. Widths must survive it: a row
    /// that cannot hold its text has to ellipsize, never push a widget out.
    Long,
    /// An ordinary value. HEIGHTS are judged against this — a wrapping
    /// label with a fixed width grows without bound in long text, so
    /// seeding long would only ever prove that arithmetic, not tell you
    /// whether the screen's structure fits.
    Ordinary,
}

/// Visibility keys a controller only ever sets one of. Seeding every bool
/// true would stack pages that never coexist — both tabs of World
/// Settings at once — and report an overflow no player can reach. Each
/// pair is checked BOTH ways instead (`Page::0` / `Page::1`).
const EXCLUSIVE: &[(&str, &str)] = &[
    ("tab_world", "tab_mods"),
    ("not_renaming", "renaming"),
    ("lan_closed", "lan_open"),
    ("is_host", "is_remote"),
    ("has_selection", "no_worlds"),
];

/// Keys whose whole point is an EMPTY screen ("No mod packs installed"),
/// which cannot be true while the list beside them is seeded with rows.
const EMPTY_STATE_KEYS: &[&str] = &["no_mods", "no_craft_results"];

/// Every catalog key of `kind` seeded, so a screen is judged with content
/// in it rather than empty. `page` picks a side of every [`EXCLUSIVE`]
/// pair.
fn seeded_state(kind: GuiKind, seed: Seed, page: usize) -> petramond_ui::UiState {
    use petramond_ui::{UiMap, UiState, UiValue};
    const LONG: &str =
        "A craftable rideable wooden chair, directional iron chains, a light-giving \
         chandelier, and a slate cauldron.";
    const ORDINARY: &str = "Nexo Test World";
    let (text, _) =
        petramond_world::assets::read_base_text("ui/bindings.json").expect("catalog ships");
    let v: serde_json::Value = serde_json::from_str(&text).expect("catalog is valid JSON");
    let mut state = UiState::new();
    let key = crate::gui::kind_key(kind).unwrap_or("");
    let Some(keys) = v["kinds"][key]["state"].as_object() else {
        return state;
    };
    let scalar = |ty: &str| match ty {
        "str" => Some(UiValue::Str(match seed {
            Seed::Long => LONG.into(),
            Seed::Ordinary => ORDINARY.to_string(),
        })),
        "bool" => Some(UiValue::Bool(true)),
        "i32" => Some(UiValue::I32(0)),
        "f32" => Some(UiValue::F32(0.5)),
        _ => None,
    };
    for (name, key) in keys {
        let value = match key["type"].as_str() {
            Some("list") => {
                let row: UiMap = key["item"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(f, ty)| Some((f.clone(), scalar(ty.as_str()?)?)))
                    .collect();
                UiValue::List(Arc::new(vec![row]))
            }
            Some(ty) => match scalar(ty) {
                Some(v) => v,
                None => continue,
            },
            None => continue,
        };
        state.set(name.clone(), value);
    }
    for (a, b) in EXCLUSIVE {
        let (on, off) = match page {
            0 => (a, b),
            _ => (b, a),
        };
        if state.get(on).is_some() {
            state.set((*on).to_string(), UiValue::Bool(true));
        }
        if state.get(off).is_some() {
            state.set((*off).to_string(), UiValue::Bool(false));
        }
    }
    for key in EMPTY_STATE_KEYS {
        if state.get(key).is_some() {
            state.set((*key).to_string(), UiValue::Bool(false));
        }
    }
    state
}

/// Every screen the shell can put in front of a player, so the two text
/// guards below cover the whole surface rather than the screens someone
/// happened to open.
const SHELL_KINDS: &[GuiKind] = &[
    GuiKind::Chest,
    GuiKind::Inventory,
    GuiKind::CraftingTable,
    GuiKind::Furnace,
    GuiKind::FurnitureWorkbench,
    GuiKind::Title,
    GuiKind::WorldSelect,
    GuiKind::WorldSettings,
    GuiKind::CreateWorld,
    GuiKind::DeleteWorld,
    GuiKind::Pause,
    GuiKind::Sleep,
    GuiKind::Death,
    GuiKind::ConnectServer,
    GuiKind::ModsMissing,
    GuiKind::ConnectionLost,
    GuiKind::Options,
    GuiKind::OptionsSound,
    GuiKind::OptionsControls,
    GuiKind::OptionsGraphics,
];

/// One solved instance handed to the guards below.
struct SolvedNode<'a, 'd> {
    inst: &'a petramond_ui::Inst<'d>,
    rect: petramond_ui::RectI,
    root: petramond_ui::RectI,
    /// The enclosing content box, or the viewport for the root.
    parent: petramond_ui::RectI,
    /// Inside a floating `tooltip` subtree (see `Solved::overlay`).
    floating: bool,
    /// Inside a `scroll` subtree, where overflowing IS the feature.
    scrolled: bool,
}

/// Solve one shipped document with seeded dynamic text at `scale`, then
/// hand every instance to `check`.
fn walk_solved(kind: GuiKind, scale: i32, seed: Seed, mut check: impl FnMut(SolvedNode<'_, '_>)) {
    for page in 0..2 {
        walk_solved_page(kind, scale, seed, page, &mut check);
    }
}

fn walk_solved_page(
    kind: GuiKind,
    scale: i32,
    seed: Seed,
    page: usize,
    check: &mut impl FnMut(SolvedNode<'_, '_>),
) {
    use petramond_ui::{solve, InstTree, ThemeEnv};
    let Some(doc) = doc_for(kind) else { return };
    let theme = crate::gui::doc_theme::theme();
    // The TIGHTEST viewport this scale ever solves into: `gui_scale` steps
    // up only once the window holds another whole 320×240, so every scale
    // bottoms out at that same logical box. Checking the wide end instead
    // would let a panel that only fits on a big monitor pass.
    let viewport = (320, 240);
    let state = seeded_state(kind, seed, page);
    // Resolve the responsive breakpoint exactly as the runtime does — a
    // document that stacks below 360px must be judged in the form it will
    // actually be arranged in.
    let compact = doc.doc.compact_active(viewport.0);
    let tree = InstTree::expand_form(&doc.doc, &state, compact);
    let env = ThemeEnv {
        theme: &theme,
        gui_scale: scale,
        image_size: &|_| None,
    };
    let solved = solve(&tree, &env, viewport, &|_| 0);
    // A `scroll` anywhere above an instance means overflowing its box is
    // the point, not a bug. Parents precede children in the arena.
    let mut scrolled = vec![false; tree.len()];
    for i in 0..tree.len() {
        let inst = tree.get(i as u32);
        if let Some(p) = inst.parent {
            scrolled[i] = scrolled[p as usize]
                || matches!(tree.get(p).node.kind, petramond_ui::NodeKind::Scroll { .. });
        }
    }
    for (i, &scrolled) in scrolled.iter().enumerate() {
        let inst = tree.get(i as u32);
        check(SolvedNode {
            inst,
            rect: solved.rects[i],
            root: solved.rects[0],
            parent: inst.parent.map_or(
                petramond_ui::RectI {
                    x: 0,
                    y: 0,
                    w: viewport.0,
                    h: viewport.1,
                },
                |p| {
                    use petramond_ui::LayoutEnv;
                    let parent = tree.get(p);
                    let pad = parent.layout.pad;
                    let border = env.container_insets(parent.node);
                    solved.rects[p as usize].inset(std::array::from_fn(|i| pad[i] + border[i]))
                },
            ),
            floating: solved.overlay[i],
            scrolled,
        });
    }
}

/// The recipe tooltip must GROW to whatever width the host says its
/// ingredient strip needs. Recipe ingredients are essential information —
/// the strip's own fallback when it runs short is to DROP the ones that
/// don't fit, which reads as a recipe with fewer ingredients than it has.
/// So the shipped documents bind the strip hook's `min_w` to the
/// published width, and this pins the whole chain: the binding present in
/// the document, resolved onto the instance, and honoured by layout.
#[test]
fn the_recipe_tooltip_grows_to_the_published_ingredient_width() {
    use petramond_ui::{solve, InstTree, ThemeEnv, UiValue};
    // Comfortably past the authored 84 floor, and inside what the
    // tooltip's own `max_w` can hold.
    const ASKED: i32 = 140;
    let theme = crate::gui::doc_theme::theme();
    for kind in [GuiKind::Inventory, GuiKind::CraftingTable] {
        let doc = doc_for(kind).expect("the recipe documents ship");
        let mut state = seeded_state(kind, Seed::Ordinary, 0);
        state.set("craft_tip_ingredients_w", UiValue::I32(ASKED));
        let tree = InstTree::expand_form(&doc.doc, &state, doc.doc.compact_active(320));
        let env = ThemeEnv {
            theme: &theme,
            gui_scale: 3,
            image_size: &|_| None,
        };
        let solved = solve(&tree, &env, (320, 240), &|_| 0);
        let strip = (0..tree.len())
            .find(|&i| tree.get(i as u32).node.id.as_deref() == Some("craft_tip_ingredients"));
        let strip = strip.unwrap_or_else(|| panic!("{kind:?} ships the ingredient strip hook"));
        assert!(
            solved.rects[strip].w >= ASKED,
            "{kind:?}: the strip asked for {ASKED} and got {} — ingredients would be hidden",
            solved.rects[strip].w
        );
    }
}

/// AUTHORED label text must fit the box the document gives it. The font is
/// layout's only sizing input, so one font swap turns every box that was
/// tuned to the old metrics into "Master V..." at once — and an ellipsis
/// on a caption nobody can widen at runtime is a bug, not a graceful
/// degradation. Bound text (world names, pack summaries, key bindings) is
/// data and ellipsizes by design; it is deliberately not checked here.
#[test]
fn authored_label_text_fits_the_box_the_document_gives_it() {
    use petramond_ui::NodeKind;
    let theme = crate::gui::doc_theme::theme();
    let mut clipped = Vec::new();
    for scale in [1i32, 3] {
        for kind in SHELL_KINDS {
            walk_solved(*kind, scale, Seed::Long, |n| {
                let NodeKind::Label {
                    text: Some(text),
                    wrap,
                    scale: label_scale,
                    small,
                } = &n.inst.node.kind
                else {
                    return;
                };
                // A run draws at `k` physical px per font pixel — one step
                // down for `small`. Convert both ways the way the solver
                // does, rounding the reservation UP.
                let k = match (*label_scale, *small) {
                    (heading, _) if heading > 1 => scale * heading as i32,
                    (_, true) => (scale - 1).max(1),
                    _ => scale,
                };
                let logical = |font_px: i32| (font_px * k + scale - 1) / scale;
                let font_px = |logical: i32| logical * scale / k;
                let font = theme.ui_font();
                let (need, have) = match wrap {
                    // A wrapping label is bounded by its box HEIGHT: it is
                    // the fixed-height ones (the remap hint) that clip.
                    true => (
                        logical(font.measure(text, Some(font_px(n.rect.w))).1),
                        n.rect.h,
                    ),
                    false => (logical(font.width(text)), n.rect.w),
                };
                if need > have {
                    clipped.push(format!(
                        "{kind:?} @scale {scale}: {text:?} needs {need}px, box is {have}px"
                    ));
                }
            });
        }
    }
    assert!(clipped.is_empty(), "clipped labels: {clipped:#?}");
}

/// However long the text that lands in a row, the row's WIDGETS stay on the
/// panel. Text is the layout's shock absorber (it ellipsizes); a checkbox
/// or a mod toggle pushed off the panel edge is unreachable, and a label
/// that keeps its natural width paints straight across the screen.
#[test]
fn long_dynamic_text_never_pushes_a_widget_off_its_screen() {
    let mut escaped = Vec::new();
    for scale in [1i32, 3] {
        for kind in SHELL_KINDS {
            walk_solved(*kind, scale, Seed::Long, |n| {
                // Tooltips float: the runtime places them at the pointer
                // and clamps them there, so the solver's parking spot says
                // nothing about where they land. Their WIDTH is bounded by
                // the document's `max_w`, checked below.
                if n.floating || n.rect.w == 0 {
                    return;
                }
                if n.rect.x < n.root.x || n.rect.x + n.rect.w > n.root.x + n.root.w {
                    escaped.push(format!(
                        "{kind:?} @scale {scale}: {:?} spans {}..{} outside {}..{}",
                        n.inst.node.kind,
                        n.rect.x,
                        n.rect.x + n.rect.w,
                        n.root.x,
                        n.root.x + n.root.w
                    ));
                }
            });
        }
    }
    assert!(escaped.is_empty(), "off-screen widgets: {escaped:#?}");

    // A floating panel is placed by the runtime, so what it owes is a
    // bounded natural size: an unbounded one covers the screen the moment
    // a pack ships a long recipe name. The item tip's cap is the ceiling —
    // at the tightest 320px viewport that is a panel beside the pointer,
    // never a screen cover.
    let mut unbounded = Vec::new();
    for kind in SHELL_KINDS {
        walk_solved(*kind, 3, Seed::Long, |n| {
            if matches!(n.inst.node.kind, petramond_ui::NodeKind::Tooltip { .. })
                && n.rect.w > ITEM_TIP_MAX_W
            {
                unbounded.push(format!("{kind:?}: floating panel is {}px wide", n.rect.w));
            }
        });
    }
    assert!(unbounded.is_empty(), "unbounded tooltips: {unbounded:#?}");
}

#[test]
fn foreign_namespace_documents_are_rejected_per_pack() {
    let kind = crate::gui::intern_kind("doctest:owned").unwrap();
    assert!(kind_permitted(kind, Some("doctest")).is_ok());
    assert!(kind_permitted(kind, Some("otherpack")).is_err());
    assert!(kind_permitted(kind, None).is_err());
    assert!(kind_permitted(GuiKind::Furnace, None).is_ok());
    assert!(kind_permitted(GuiKind::Title, Some("anypack")).is_ok());
}

/// A scratch dir of sheets for the collection tests below (the collector
/// resolves real files beside the document).
fn test_art_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "petramond-gui-doc-art-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
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

/// Image-backed buttons name document-local sheets exactly like `image`
/// nodes do: both must resolve beside the document and land in the same
/// first-reference-ordered table that feeds `TexId::DocImage`.
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
    std::fs::remove_dir_all(&dir).ok();
}

/// A statically named image that does not exist rejects the document —
/// the old "quad will not draw" log left the screen half-drawn with no
/// symptom until someone opened it. The empty-name + `bind.image`
/// runtime pattern stays legal: there is no file to resolve.
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
    std::fs::remove_dir_all(&dir).ok();
}

/// The framed-sheet guard: the grid must divide the sheet exactly, the
/// frame count stays under the cap, and the sheet fits the shared side
/// ceiling — each a document rejection, since the whole sheet uploads as
/// one texture.
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
    // Unframed sheets carry no grid contract: only framed ones are sized.
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
    std::fs::remove_dir_all(&dir).ok();
}

/// The first SEEN frames grid wins even when the first reference to the
/// sheet is unframed — an unframed duplicate must not launder a bad
/// sheet past the check, on a button any more than on an image.
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
    std::fs::remove_dir_all(&dir).ok();
}
