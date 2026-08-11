use super::*;

use crate::augments::{Entry, AUGMENTS_KEY, AUGMENT_KEY};

fn fit(tool: &str, overlay: &str, cost: u8, speed: f32, damage: f32) -> Fit {
    Fit {
        tool: tool.into(),
        tier: 4,
        speed_mult: speed,
        damage_mult: damage,
        cost,
        overlay: overlay.into(),
        overlays: Vec::new(),
        gentle: None,
        wear: None,
    }
}

fn spec() -> AnvilSpec {
    let mut s = AnvilSpec::default();
    let mut tip = fit("pickaxe", "forge:diamond_tip", 2, 1.5, 2.0);
    tip.overlays = vec![("stone".into(), "forge:diamond_tip_stone".into())];
    s.augments.insert("petramond:diamond".into(), vec![tip]);
    let mut inlay = fit("pickaxe", "forge:gold_inlay", 3, 0.8, 1.0);
    inlay.gentle = Some(25);
    s.augments
        .insert("petramond:gold_ingot".into(), vec![inlay]);
    s.augments.insert(
        "monsters:hushjaw_tooth".into(),
        vec![fit("pickaxe", "monsters:fang", 3, 1.0, 4.0 / 3.0)],
    );
    for (material, fits) in &s.augments {
        for f in fits {
            s.by_identity
                .entry(f.overlay.clone())
                .or_default()
                .push(f.clone());
            s.material_of.insert(f.overlay.clone(), material.clone());
        }
    }
    s.tools.insert(
        "petramond:stone_pickaxe".into(),
        (
            ToolSlots {
                family: "stone".into(),
                lockable: 0,
            },
            ToolStats {
                kind: "pickaxe".into(),
                tier: 2,
                speed: 4.0,
                damage: [1.0, 2.5],
            },
        ),
    );
    s.tools.insert(
        "petramond:iron_pickaxe".into(),
        (
            ToolSlots {
                family: "default".into(),
                lockable: 3,
            },
            ToolStats {
                kind: "pickaxe".into(),
                tier: 3,
                speed: 6.0,
                damage: [2.0, 4.0],
            },
        ),
    );
    s.tools.insert(
        "forge:gold_pickaxe".into(),
        (
            ToolSlots {
                family: "default".into(),
                lockable: 0,
            },
            ToolStats {
                kind: "pickaxe".into(),
                tier: 3,
                speed: 1.0,
                damage: [1.0, 1.0],
            },
        ),
    );
    s.nondestructive.insert("forge:gold_pickaxe".into());
    s.socket_items.insert("forge:petramond".into());
    s
}

fn stack(item: &str, count: u8) -> Option<ItemStackData> {
    Some(ItemStackData {
        item: item.into(),
        count,
        data: Vec::new(),
    })
}

fn with_record(item: &str, record: &str) -> Option<ItemStackData> {
    let mut s = stack(item, 1).unwrap();
    s.data
        .push((AUGMENTS_KEY.into(), record.as_bytes().to_vec()));
    Some(s)
}

fn get(stack: &ItemStackData, key: &str) -> Option<String> {
    stack
        .data
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| String::from_utf8(v.clone()).unwrap())
}

/// The swap sweep keys on the CURRENT tool's cell states: a swap that
/// shrinks capacity (or lands an occupied socket, or an unreadable
/// tool) sends stranded materials home like a tool pull, while stacks
/// in open cells stay staged.
#[test]
fn a_swapped_in_tool_ejects_stacks_from_cells_it_does_not_offer() {
    let s = spec();
    // Iron pickaxe, one socket carved: cells 1 and 2 are open.
    let mut slots = vec![
        with_record("petramond:iron_pickaxe", "1|"),
        stack("petramond:diamond", 2),
        stack("monsters:hushjaw_tooth", 3),
        None,
        None,
    ];
    assert_eq!(s.cells_to_eject(&slots), Vec::<usize>::new());

    // A fresh stone pickaxe offers only cell 1: the tooth is stranded.
    slots[SLOT_TOOL] = stack("petramond:stone_pickaxe", 1);
    assert_eq!(s.cells_to_eject(&slots), vec![2]);

    // A tool whose first socket is already occupied strands the
    // diamond resting there; the open cell 2 keeps its stage.
    slots[SLOT_TOOL] = with_record("petramond:iron_pickaxe", "3|forge:diamond_tip");
    assert_eq!(s.cells_to_eject(&slots), vec![1]);

    // An unreadable record (the pre-socket format) refuses every cell.
    slots[SLOT_TOOL] = with_record("petramond:iron_pickaxe", "forge:diamond_tip");
    assert_eq!(s.cells_to_eject(&slots), vec![1, 2]);
}

/// The occupied-socket gestures: a PRISTINE augment refuses repair (the
/// gesture opens at Excellent or lower — never waste a material on the
/// top band); the identity's own material then repairs at the install
/// rate consuming only what the bar needs; socket gems raise the mount
/// level to the cap keeping the condition's absolute quanta; a gem
/// resting on an occupied socket is never carve fuel.
#[test]
fn occupied_socket_gestures_repair_and_upgrade_on_the_drop() {
    let s = spec();
    let mut slots = vec![
        with_record("petramond:iron_pickaxe", "3|forge:diamond_tip@97"),
        stack("petramond:diamond", 3),
        None,
        None,
        None,
    ];
    s.tend_sockets(&mut slots);
    let rec = Record::of_stack(&slots[0].as_ref().unwrap().data).unwrap();
    assert_eq!(rec.entry_at(0).unwrap().cond, 97, "pristine refuses repair");
    assert_eq!(slots[1].as_ref().unwrap().count, 3, "nothing consumed");

    slots[0] = with_record("petramond:iron_pickaxe", "3|forge:diamond_tip@75");
    s.tend_sockets(&mut slots);
    let rec = Record::of_stack(&slots[0].as_ref().unwrap().data).unwrap();
    assert_eq!(rec.entry_at(0).unwrap().cond, 100, "clamped at full");
    assert_eq!(
        slots[1].as_ref().unwrap().count,
        2,
        "only the needed diamond consumed"
    );

    slots[1] = stack("forge:petramond", 5);
    s.tend_sockets(&mut slots);
    let rec = Record::of_stack(&slots[0].as_ref().unwrap().data).unwrap();
    let e = rec.entry_at(0).unwrap();
    assert_eq!(
        (e.lvl, e.cond),
        (3, 100),
        "levels cap; quanta stay absolute"
    );
    assert_eq!(
        slots[1].as_ref().unwrap().count,
        2,
        "three gems, three levels"
    );

    let slots = vec![
        with_record("petramond:iron_pickaxe", "0|forge:diamond_tip"),
        stack("forge:petramond", 1),
        None,
        None,
        None,
    ];
    assert!(
        s.carve(&slots).is_none(),
        "a gem on an occupied socket is an upgrade, not carve fuel"
    );
}

/// The three stamped keys and their arithmetic: the gate goes to the
/// edge, speed/damage multiply the base's RESOLVED values, the record is
/// positional per socket cell, and the apply consumes the fit's cost
/// from the cell it was staged in.
#[test]
fn a_staged_material_stamps_the_three_keys_with_the_edge_tier() {
    let s = spec();
    let slots = vec![
        stack("petramond:stone_pickaxe", 1),
        stack("petramond:diamond", 3),
        None,
        None,
        None,
    ];
    let (fitted, consumes) = s.apply_staged(&slots).expect("the pair fits");
    assert_eq!(consumes, vec![(1, 2)], "two diamonds out of cell 1");
    // The recorded identity is the canonical overlay; the ART is the
    // stone-family drawing, because the stone pickaxe's silhouette is not
    // the iron family's.
    assert_eq!(
        get(&fitted, AUGMENTS_KEY).as_deref(),
        Some("0|forge:diamond_tip")
    );
    assert_eq!(
        get(&fitted, OVERLAY_DATA_KEY).as_deref(),
        Some("forge:diamond_tip_stone")
    );
    assert_eq!(
        get(&fitted, TOOL_OVERRIDE_KEY).as_deref(),
        Some(r#"{"tier":4,"speed":6.0000,"damage":[2.0000,5.0000]}"#)
    );
    // Keys are sorted — the canonical order the ABI ingest expects.
    let keys: Vec<&String> = fitted.data.iter().map(|(k, _)| k).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

/// The refusals: an unknown tool, a short stack, a repeated identity, a
/// locked cell, a gentle fit on an innately-gentle tool, and a record
/// this build cannot reason about all leave the slots exactly as they
/// are.
#[test]
fn what_does_not_fit_is_left_alone() {
    let s = spec();
    let sockets = |a, b, c, d| vec![a, b, c, d];
    // Not an augmentable tool.
    let slots = [
        vec![stack("petramond:stick", 1), stack("petramond:diamond", 9)],
        sockets(None, None, None, None),
    ]
    .concat();
    assert!(s.apply_staged(&slots[..SLOTS]).is_none());
    // Too little material: one diamond against a cost of two.
    let slots = vec![
        stack("petramond:stone_pickaxe", 1),
        stack("petramond:diamond", 1),
        None,
        None,
        None,
    ];
    assert!(s.apply_staged(&slots).is_none(), "cost gates the apply");
    // The same augment type never repeats: a tip already on the record
    // blocks a second diamond even in a different open cell.
    let slots = vec![
        with_record("petramond:iron_pickaxe", "3|forge:diamond_tip"),
        None,
        stack("petramond:diamond", 9),
        None,
        None,
    ];
    assert!(s.apply_staged(&slots).is_none(), "identity occupancy");
    // A LOCKED cell stages nothing: the stone pickaxe has no lockable
    // sockets and its one open cell is cell 1.
    let slots = vec![
        stack("petramond:stone_pickaxe", 1),
        None,
        stack("petramond:diamond", 9),
        None,
        None,
    ];
    assert!(s.apply_staged(&slots).is_none(), "cell 2 is absent");
    // A gentle fit on a tool whose row is innately gentle is no fit.
    let slots = vec![
        stack("forge:gold_pickaxe", 1),
        stack("petramond:gold_ingot", 9),
        None,
        None,
        None,
    ];
    assert!(s.apply_staged(&slots).is_none(), "gold-on-gold refused");
    // A record naming an identity this build does not know (a richer
    // pack set wrote it) refuses further augments rather than guessing.
    let slots = vec![
        with_record("petramond:iron_pickaxe", "0|gone:mod_augment"),
        stack("petramond:diamond", 9),
        None,
        None,
        None,
    ];
    assert!(s.apply_staged(&slots).is_none(), "unknown identity refused");
    // The pre-socket record format is a record this build cannot
    // re-encode faithfully: refused the same way.
    let slots = vec![
        with_record("petramond:iron_pickaxe", "forge:diamond_tip"),
        stack("petramond:diamond", 9),
        None,
        None,
        None,
    ];
    assert!(s.apply_staged(&slots).is_none(), "old format refused");
}

/// One identity offered by SEVERAL open cells stages once, in the cell
/// that can afford it. A single diamond used to claim the identity for
/// cell 1 and the five in cell 2 were never staged at all, so the button
/// sat disabled and the panel hinted "Needs 2" beside enough material.
#[test]
fn an_unaffordable_cell_never_shadows_a_stocked_one() {
    let s = spec();
    let slots = vec![
        with_record("petramond:iron_pickaxe", "3|"),
        stack("petramond:diamond", 1),
        stack("petramond:diamond", 5),
        None,
        None,
    ];
    let staged = s.staged(&slots);
    assert_eq!(staged.len(), 1, "one identity, one staged fit");
    assert_eq!(
        (staged[0].0, staged[0].2),
        (2, 5),
        "the stocked cell takes the slot"
    );
    let (fitted, consumes) = s.apply_staged(&slots).expect("cell 2 can afford the tip");
    assert_eq!(consumes, vec![(2, 2)]);
    assert_eq!(
        get(&fitted, AUGMENTS_KEY).as_deref(),
        Some("3|,forge:diamond_tip"),
        "the identity lands in the cell that paid for it"
    );

    // With no cell able to afford it the FIRST still stages, so the panel
    // keeps hinting the shortfall instead of going quiet.
    let short = vec![
        with_record("petramond:iron_pickaxe", "3|"),
        stack("petramond:diamond", 1),
        stack("petramond:diamond", 1),
        None,
        None,
    ];
    let staged = s.staged(&short);
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].0, 1, "the first cell keeps the hint");
    assert!(s.apply_staged(&short).is_none());
}

/// Several materials staged at once land in THEIR cells, the record is
/// positional, and the stamped override is RECOMPUTED from the base row
/// plus the full record — a stale (even nonsensical) previous stamp on
/// the stack has no bearing on the result.
#[test]
fn staged_materials_apply_positionally_and_recompute_from_the_base() {
    let s = spec();
    let mut tool = with_record("petramond:iron_pickaxe", "2|forge:diamond_tip").unwrap();
    // A stale stamp from a rebalanced past — recompute must ignore it.
    tool.data.push((
        TOOL_OVERRIDE_KEY.into(),
        br#"{"tier":9,"speed":99.0,"damage":[9.0,9.0]}"#.to_vec(),
    ));
    let slots = vec![
        Some(tool),
        None,
        stack("petramond:gold_ingot", 3),
        stack("monsters:hushjaw_tooth", 3),
        None,
    ];
    let (fitted, consumes) = s.apply_staged(&slots).expect("two open cells stage");
    assert_eq!(consumes, vec![(2, 3), (3, 3)]);
    assert_eq!(
        get(&fitted, AUGMENTS_KEY).as_deref(),
        Some("2|forge:diamond_tip,forge:gold_inlay,monsters:fang"),
        "identities sit in the cells the materials were staged in"
    );
    // Base 6.0 speed × 1.5 (tip) × 0.8 (inlay) × 1.0 (fang) = 7.2;
    // damage [2,4] × 2.0 × 1.3333.
    assert_eq!(
        get(&fitted, TOOL_OVERRIDE_KEY).as_deref(),
        Some(r#"{"tier":4,"speed":7.2000,"damage":[5.3333,10.6667]}"#)
    );
    assert_eq!(
        get(&fitted, OVERLAY_DATA_KEY).as_deref(),
        Some("forge:diamond_tip,forge:gold_inlay,monsters:fang")
    );
}

/// Carving: a socket material beside a tool with locked sockets left
/// opens ONE more (restamping only the record), stops at the row's
/// lockable count, and does nothing for a tool with none.
#[test]
fn a_socket_material_carves_one_locked_socket_per_step() {
    let s = spec();
    let slots = vec![
        stack("petramond:iron_pickaxe", 1),
        None,
        None,
        stack("forge:petramond", 2),
        None,
    ];
    let (carved, cell) = s.carve(&slots).expect("iron has lockable sockets");
    assert_eq!(cell, 3, "consumed from the cell the gem sits in");
    assert_eq!(get(&carved, AUGMENTS_KEY).as_deref(), Some("1|"));
    assert!(
        get(&carved, TOOL_OVERRIDE_KEY).is_none(),
        "carving alone stamps no engine override"
    );
    // Fully carved: no further carve.
    let slots = vec![
        with_record("petramond:iron_pickaxe", "3|"),
        stack("forge:petramond", 2),
        None,
        None,
        None,
    ];
    assert!(s.carve(&slots).is_none(), "lockable is the cap");
    // No lockable sockets at all.
    let slots = vec![
        stack("petramond:stone_pickaxe", 1),
        stack("forge:petramond", 2),
        None,
        None,
        None,
    ];
    assert!(s.carve(&slots).is_none());
    // No tool: the gem sits inert.
    let slots = vec![None, stack("forge:petramond", 2), None, None, None];
    assert!(s.carve(&slots).is_none());
}

/// The socket-cell decision table: occupied shows its ghost, carved
/// cells are open, uncarved lockable cells are locked, and everything
/// past the row's count is absent.
#[test]
fn cell_states_follow_the_record_and_the_rows_lockable_count() {
    let tool_slots = ToolSlots {
        family: "default".into(),
        lockable: 3,
    };
    let rec = Record::parse(b"1|,forge:diamond_tip").unwrap();
    assert!(matches!(
        AnvilSpec::cell_state(&tool_slots, &rec, 0),
        CellState::Open
    ));
    assert!(matches!(
        AnvilSpec::cell_state(&tool_slots, &rec, 1),
        CellState::Occupied("forge:diamond_tip")
    ));
    assert!(matches!(
        AnvilSpec::cell_state(&tool_slots, &rec, 2),
        CellState::Locked
    ));
    assert!(matches!(
        AnvilSpec::cell_state(&tool_slots, &rec, 3),
        CellState::Locked
    ));
    let stone = ToolSlots {
        family: "stone".into(),
        lockable: 0,
    };
    let fresh = Record::default();
    assert!(matches!(
        AnvilSpec::cell_state(&stone, &fresh, 0),
        CellState::Open
    ));
    assert!(matches!(
        AnvilSpec::cell_state(&stone, &fresh, 1),
        CellState::Absent
    ));
}

/// The socket tooltip's SPAN FORMAT: two lines of two `palette|text` spans,
/// only the level and condition WORDS coloured — and the display name is
/// stripped of the three separators, which are structural and unescapable.
/// A name is row data this pack does not own, so one containing a `|` would
/// otherwise fold the tooltip's layout on the engine side.
#[test]
fn the_socket_tip_colours_only_the_words_and_strips_the_separators() {
    let mut s = spec();
    s.names
        .insert("forge:diamond_tip".into(), "Dia|mond\tTip".into());
    let tip = s.socket_tip(&Entry {
        id: "forge:diamond_tip".into(),
        cond: 50,
        lvl: 1,
    });
    assert_eq!(
        tip,
        "accent|Great\ttext| DiamondTip\ntext|Condition: \twarn|Worn",
        "level word on its palette, name plain and de-separated, condition word on its own"
    );
}

/// THE ADMISSION MASK IS BIT POSITIONS OVER THE DOCUMENT'S OWN FILTER
/// LIST, and a mod cannot read its document's filters at runtime — so
/// this is the only place the two halves are compared. It also pins the
/// per-socket state keys the panel binds against the ones
/// `publish_stage` writes: an off-by-one there is a cell that never
/// updates, with nothing to fail.
#[test]
fn the_panels_socket_cells_author_the_filters_these_bits_index() {
    const DOC: &str = include_str!("../../pack/ui/documents/anvil.gui.json");
    let doc = json::Value::parse(DOC).expect("the shipped panel document is valid JSON");
    let mut frames = Vec::new();
    socket_frames(
        doc.get("root").expect("the document has a root node"),
        &mut frames,
    );
    assert_eq!(
        frames.len(),
        SOCKETS,
        "the machine publishes state for {SOCKETS} socket cells; the document authors {}",
        frames.len()
    );
    for (socket, frame) in frames.iter().enumerate() {
        let slot = child(frame, "slot").expect("a socket cell holds its slot");
        let filters: Vec<&str> = slot
            .get("accepts")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().filter_map(|f| f.get("data")?.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(
            filters,
            vec![AUGMENT_KEY, SOCKET_ITEM_KEY],
            "socket {socket}: ACC_AUGMENT ({ACC_AUGMENT}) and ACC_SOCKET ({ACC_SOCKET}) are BIT \
             POSITIONS over this authored list. Reorder it and admission inverts silently — \
             locked cells would take augment materials and open cells only the gem — with no \
             compile error and no log line. Fix the document, or renumber the bits with it."
        );
        assert_eq!(
            bound(slot, "accepts"),
            Some(panel::sock_key(socket, "acc")),
            "socket {socket} binds a mask key the machine never publishes"
        );
        assert_eq!(
            child(frame, "hook").and_then(|h| bound(h, "item")),
            Some(panel::sock_key(socket, "ghost")),
            "socket {socket}'s ghost hook"
        );
        assert_eq!(
            child(frame, "image").and_then(|i| bound(i, "frame")),
            Some(panel::sock_key(socket, "st")),
            "socket {socket}'s state chrome"
        );
    }
}

/// The socket cells in document order: a node is one when it holds a slot
/// whose `accepts` is bound (the bare tool slot's filter is authored only).
fn socket_frames<'a>(node: &'a json::Value, out: &mut Vec<&'a json::Value>) {
    let Some(children) = node.get("children").and_then(|c| c.as_array()) else {
        return;
    };
    if children.iter().any(|c| bound(c, "accepts").is_some()) {
        out.push(node);
    }
    for child in children {
        socket_frames(child, out);
    }
}

fn child<'a>(node: &'a json::Value, kind: &str) -> Option<&'a json::Value> {
    node.get("children")?
        .as_array()?
        .iter()
        .find(|c| c.get("type").and_then(|t| t.as_str()) == Some(kind))
}

fn bound(node: &json::Value, what: &str) -> Option<String> {
    Some(node.get("bind")?.get(what)?.as_str()?.to_owned())
}
