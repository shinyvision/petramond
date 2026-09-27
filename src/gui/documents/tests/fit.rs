//! The document fit guards: every screen the shell shows and every document
//! a pack in `mods-src/` ships, solved in every window of [`views`] with its
//! catalog's keys seeded.

use super::super::*;
use crate::gui::GuiKind;
use petramond_ui::Node;

/// The font's line box drives every document's vertical budget, so a font
/// swap (or one more label) must not push a shipped screen off any window
/// the game lays it out in. Panels that legitimately scroll
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
/// The rule itself (scroll/tooltip/abs exemptions) is the shared
/// [`petramond_ui::contract::overflows`] the gui-builder also runs; this
/// judges every shipped document in every window of [`views`].
#[test]
fn every_shipped_document_fits_every_window() {
    let screens = screens();
    let views = views(&screens);
    let mut overflowing = Vec::new();
    for screen in &screens {
        let kind = &screen.name;
        solve_views(screen, &views, Seed::Ordinary, |v| {
            for o in petramond_ui::contract::overflows(v.tree, v.solved, v.env) {
                overflowing.push(format!(
                    "{kind} @{}: {} at {:?} outside parent content {:?}",
                    v.view,
                    describe(v.tree.get(o.inst).node),
                    o.rect,
                    o.content,
                ));
            }
        });
    }
    assert!(
        overflowing.is_empty(),
        "documents overflow: {overflowing:#?}"
    );
}

/// One window the guards solve in: the logical box the game lays a
/// document out in, and the gui scale it draws that box at.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct View {
    size: (i32, i32),
    scale: i32,
}

impl std::fmt::Display for View {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = self.size;
        write!(f, "{w}x{h} scale {}", self.scale)
    }
}

/// Physical windows every document is solved in, each at the gui scale the
/// game picks for it.
const WINDOWS: &[(u32, u32)] = &[
    // The tightest box, 320×240 logical, at every scale: text rounds to
    // logical px differently at each.
    (320, 240),
    (640, 480),
    (960, 720),
    (1280, 960),
    // Common desktops. The scale caps at 4, so the big ones get the widest
    // logical boxes, where anything sized by a share of the screen is
    // stretched furthest.
    (1024, 768),
    (1280, 720),
    (1366, 768),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
    (5120, 2880),
    // Odd aspects: 4:3 at scale 2, 16:10, ultrawide, portrait.
    (800, 600),
    (1920, 1200),
    (2560, 1080),
    (3440, 1440),
    (1080, 1920),
];

/// Every view the guards solve in: [`WINDOWS`], plus a window just below
/// and at every `compact_below_w` a shipped document declares. The smallest
/// window is NOT always the worst: just above its breakpoint a responsive
/// document is in its wide form with the least room that form ever gets,
/// and a side panel there is narrower than the compact form's at 320×240.
fn views(screens: &[Screen]) -> Vec<View> {
    let mut windows = WINDOWS.to_vec();
    for w in screens.iter().filter_map(|s| s.doc.compact_below_w) {
        // Scale 3 is what a 720p-class window at that width picks.
        for scale in [1, 3] {
            for w in [w - 1, w] {
                windows.push(((w * scale) as u32, (240 * scale) as u32));
            }
        }
    }
    let mut views: Vec<View> = windows
        .into_iter()
        .map(|window| {
            let scale = crate::gui::gui_scale(window) as i32;
            View {
                size: (window.0 as i32 / scale, window.1 as i32 / scale),
                scale,
            }
        })
        .collect();
    views.sort();
    views.dedup();
    views
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

/// Visibility keys a controller only ever sets ONE of, as n-way groups.
/// Seeding every bool true would stack pages that never coexist — both tabs
/// of World Settings at once — and report an overflow no player can reach.
/// Each group is checked member by member instead (see [`page_members`]),
/// on screen keys and on list-row fields alike. A member may name several
/// keys that are on together (`a+b`). Put a group's MAIN state first: every
/// other group's members are seen beside it.
const EXCLUSIVE: &[&[&str]] = &[
    &["tab_world", "tab_mods"],
    &["not_renaming", "renaming"],
    &["lan_closed", "lan_open"],
    &["is_host", "is_remote"],
    &["has_selection", "no_worlds"],
    &["signed_out", "is_signed_in"],
    // Creative's three browsing tabs, and its delete confirm over them.
    &[
        "catalog_tab+browsing",
        "selection_tab+browsing",
        "library_tab+browsing",
        "confirming_delete",
    ],
    // The content browser's pages and confirm variants, and its stamp kinds
    // with the entry's action and trash columns.
    &[
        "list_page",
        "confirm_page+confirming_destroy+cancel_left",
        "confirm_page+confirming_go+shows_go+cancel_left",
        "confirm_page+confirming_exit+shows_go",
    ],
    &[
        "is_header",
        "is_message",
        "is_entry+collapsed+can_get+can_delete",
        "is_entry+collapsed+can_replace+can_delete",
        "is_entry+expanded+has_world_note+can_undo",
        "is_entry+collapsed+busy",
    ],
    &["has_icon", "no_icon"],
];

/// Every key an exclusive group names.
fn group_keys(group: &[String]) -> impl Iterator<Item = &str> + '_ {
    group.iter().flat_map(|member| member.split('+'))
}

/// Which member of each group a page shows. Page 0 shows every group's
/// first member; each later page moves ONE group to one of its other
/// members. Every member is seen, each beside the other groups' main states
/// — never paired with a state it cannot coexist with, which cycling every
/// group at once did (the empty-list state only ever beside a confirm page).
fn page_members(groups: &[Vec<String>], page: usize) -> Vec<usize> {
    let mut members = vec![0; groups.len()];
    let mut left = page;
    for (at, group) in groups.iter().enumerate() {
        let others = group.len().saturating_sub(1);
        if (1..=others).contains(&left) {
            members[at] = left;
            break;
        }
        left = left.saturating_sub(others);
    }
    members
}

/// Turn each group's chosen member on and the rest off, for whichever of
/// the group's keys `has` says exist.
fn apply_groups(
    groups: &[Vec<String>],
    members: &[usize],
    has: impl Fn(&str) -> bool,
    mut set: impl FnMut(&str, bool),
) {
    for (group, &member) in groups.iter().zip(members) {
        let on: Vec<&str> = group[member].split('+').collect();
        for key in group_keys(group).filter(|k| has(k)) {
            set(key, on.contains(&key));
        }
    }
}

/// The screen's groups that name a key it has, on the screen or in a row.
fn present_groups(screen: &Screen, state: &petramond_ui::UiState) -> Vec<Vec<String>> {
    let has = |key: &str| {
        state.get(key).is_some()
            || state.keys().any(|k| {
                matches!(state.get(k), Some(petramond_ui::UiValue::List(rows))
                    if rows.iter().any(|row| row.contains_key(key)))
            })
    };
    screen
        .groups()
        .into_iter()
        .filter(|group| group_keys(group).any(has))
        .collect()
}

/// How many pages a screen needs: one, plus one per group member past the
/// first.
fn page_count(screen: &Screen) -> usize {
    let state = base_state(screen, Seed::Ordinary);
    let groups = present_groups(screen, &state);
    1 + groups
        .iter()
        .map(|g| g.len().saturating_sub(1))
        .sum::<usize>()
}

/// Keys whose whole point is an EMPTY screen ("No mod packs installed"),
/// which cannot be true while the list beside them is seeded with rows.
const EMPTY_STATE_KEYS: &[&str] = &["no_mods", "no_craft_results"];

/// Every catalog key of `screen` seeded, so a screen is judged with content
/// in it rather than empty. `page` picks a side of every exclusive group.
/// A key whose catalog entry names its `longest` value is seeded with that
/// (the widest text its controller ever publishes) whatever the seed; a bool
/// whose entry says `"seed": false` is never on while the screen is up.
fn seeded_state(screen: &Screen, seed: Seed, page: usize) -> petramond_ui::UiState {
    use petramond_ui::{UiMap, UiValue};
    let mut state = base_state(screen, seed);
    let groups = present_groups(screen, &state);
    let members = page_members(&groups, page);
    let keys: Vec<String> = state.keys().map(str::to_owned).collect();
    for name in &keys {
        if let Some(UiValue::List(rows)) = state.get(name) {
            let rows: Vec<UiMap> = rows
                .iter()
                .map(|row| {
                    let mut row = row.clone();
                    let present: Vec<String> = row.keys().cloned().collect();
                    apply_groups(
                        &groups,
                        &members,
                        |k| present.iter().any(|p| p == k),
                        |k, on| {
                            row.insert(k.to_owned(), UiValue::Bool(on));
                        },
                    );
                    row
                })
                .collect();
            state.set(name.clone(), UiValue::List(Arc::new(rows)));
        }
    }
    apply_groups(
        &groups,
        &members,
        |k| keys.iter().any(|name| name == k),
        |k, on| state.set(k.to_owned(), UiValue::Bool(on)),
    );
    for key in EMPTY_STATE_KEYS {
        if state.get(key).is_some() {
            state.set((*key).to_string(), UiValue::Bool(false));
        }
    }
    state
}

/// Every catalog key of `screen` at its seed, before any group is applied.
fn base_state(screen: &Screen, seed: Seed) -> petramond_ui::UiState {
    use petramond_ui::{UiMap, UiState, UiValue};
    const LONG: &str =
        "A craftable rideable wooden chair, directional iron chains, a light-giving \
         chandelier, and a slate cauldron.";
    const ORDINARY: &str = "Nexo Test World";
    let mut state = UiState::new();
    let Some(keys) = screen.catalog["state"].as_object() else {
        return state;
    };
    let scalar = |entry: &serde_json::Value| {
        let (ty, longest) = match entry {
            serde_json::Value::String(ty) => (ty.as_str(), None),
            entry => (entry["type"].as_str()?, entry["longest"].as_str()),
        };
        match ty {
            "str" => Some(UiValue::Str(match (longest, seed) {
                (Some(longest), _) => longest.into(),
                (None, Seed::Long) => LONG.into(),
                (None, Seed::Ordinary) => ORDINARY.to_string(),
            })),
            "bool" => Some(UiValue::Bool(entry["seed"].as_bool().unwrap_or(true))),
            "i32" => Some(UiValue::I32(0)),
            "f32" => Some(UiValue::F32(0.5)),
            _ => None,
        }
    };
    for (name, key) in keys {
        let value = match key["type"].as_str() {
            Some("list") => {
                let row: UiMap = key["item"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(f, entry)| Some((f.clone(), scalar(entry)?)))
                    .collect();
                UiValue::List(Arc::new(vec![row]))
            }
            Some(_) => match scalar(key) {
                Some(v) => v,
                None => continue,
            },
            None => continue,
        };
        state.set(name.clone(), value);
    }
    state
}

/// One document the guards solve: a screen the shell puts in front of a
/// player, or a document a pack in `mods-src/` ships.
struct Screen {
    name: String,
    doc: Arc<Document>,
    /// The kind's catalog entry: `state` keys (each a type, or `{type,
    /// longest}`), and the pack's own `exclusive` groups beside [`EXCLUSIVE`].
    catalog: serde_json::Value,
}

impl Screen {
    /// [`EXCLUSIVE`] plus the catalog's own groups. An empty member turns
    /// every key of its group off (no popup open, no confirm up).
    fn groups(&self) -> Vec<Vec<String>> {
        let own = self.catalog["exclusive"].as_array().into_iter().flatten();
        EXCLUSIVE
            .iter()
            .map(|g| g.iter().map(|m| (*m).to_owned()).collect())
            .chain(own.filter_map(|g| {
                let members = g.as_array()?.iter();
                Some(
                    members
                        .filter_map(|m| Some(m.as_str()?.to_owned()))
                        .collect(),
                )
            }))
            .collect()
    }
}

/// A shell screen with the engine catalog's entry for its kind.
fn engine_screen(kind: GuiKind) -> Option<Screen> {
    let (text, _) =
        petramond_world::assets::read_base_text("ui/bindings.json").expect("catalog ships");
    let v: serde_json::Value = serde_json::from_str(&text).expect("catalog is valid JSON");
    let key = crate::gui::kind_key(kind).unwrap_or("");
    Some(Screen {
        name: format!("{kind:?}"),
        doc: doc_for(kind)?.doc,
        catalog: v["kinds"][key].clone(),
    })
}

/// Every document a pack in `mods-src/` ships, validated as the game loads
/// it (a document the game would refuse fails here), with the
/// catalog the pack keeps beside its documents (`pack/ui/bindings.json`).
fn pack_screens() -> Vec<Screen> {
    let theme = crate::gui::doc_theme::theme();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("mods-src");
    let mut packs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("mods-src lists")
        .flatten()
        .map(|e| e.path().join("pack"))
        .filter(|p| p.join("pack.json").is_file())
        .collect();
    packs.sort();
    let mut screens = Vec::new();
    for pack in packs {
        let manifest = std::fs::read_to_string(pack.join("pack.json")).expect("pack.json reads");
        let manifest: serde_json::Value =
            serde_json::from_str(&manifest).expect("pack.json parses");
        let id = manifest["id"].as_str().expect("pack.json names its id");
        let catalog: serde_json::Value = std::fs::read_to_string(pack.join("ui/bindings.json"))
            .map(|t| serde_json::from_str(&t).expect("a pack catalog is valid JSON"))
            .unwrap_or_default();
        let dir = pack.join("ui/documents");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".gui.json"))
            .collect();
        files.sort();
        for file in files {
            let (_, doc, _) = read_document(&file, &dir, Some(id), &theme)
                .unwrap_or_else(|e| panic!("{} does not load: {e}", file.display()));
            screens.push(Screen {
                name: format!("{id}/{}", file.file_name().unwrap().to_string_lossy()),
                catalog: catalog["kinds"][doc.kind.as_str()].clone(),
                doc: Arc::new(doc),
            });
        }
    }
    screens
}

/// Every screen the guards cover: the whole shell, and every pack document
/// in the repo.
fn screens() -> Vec<Screen> {
    let mut screens: Vec<Screen> = SHELL_KINDS
        .iter()
        .filter_map(|&k| engine_screen(k))
        .collect();
    screens.extend(pack_screens());
    screens
}

/// Every screen the shell can put in front of a player, so the two text
/// guards below cover the whole surface rather than the screens someone
/// happened to open.
const SHELL_KINDS: &[GuiKind] = &[
    GuiKind::Creative,
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
    GuiKind::Account,
    GuiKind::AccountSignIn,
    GuiKind::Content,
    GuiKind::ModsMissing,
    GuiKind::ConnectionLost,
    GuiKind::Options,
    GuiKind::OptionsSound,
    GuiKind::OptionsControls,
    GuiKind::OptionsGraphics,
];

/// A node as a failure names it: its kind, and its id and bindings when it
/// has them, so the report says which key to look at.
fn describe(node: &Node) -> String {
    let mut out = format!("{:?}", node.kind);
    if let Some(id) = &node.id {
        out.push_str(&format!(" #{id}"));
    }
    if let Ok(serde_json::Value::Object(bind)) = serde_json::to_value(&node.bind) {
        for (prop, key) in bind.iter().filter(|(_, key)| !key.is_null()) {
            out.push_str(&format!(" {prop}={key}"));
        }
    }
    out
}

/// One solved instance handed to the guards below.
struct SolvedNode<'a, 'd> {
    inst: &'a petramond_ui::Inst<'d>,
    rect: petramond_ui::RectI,
    root: petramond_ui::RectI,
    /// Inside a floating `tooltip` subtree (see `Solved::overlay`).
    floating: bool,
    view: View,
}

/// One document laid out in one view, handed to the guards below.
struct SolvedView<'a, 'd> {
    tree: &'a petramond_ui::InstTree<'d>,
    solved: &'a petramond_ui::Solved,
    env: &'a petramond_ui::ThemeEnv<'a>,
    view: View,
}

/// Solve one shipped document with seeded dynamic text in every view, and
/// hand each solve to `check`: every page of [`EXCLUSIVE`], and each
/// anchored tooltip once, shown as its widget's (first stamp's) hover would
/// show it. Each document is solved in the form the runtime would arrange
/// it in at that view's width (`compact_below_w`).
fn solve_views(
    screen: &Screen,
    views: &[View],
    seed: Seed,
    mut check: impl FnMut(&SolvedView<'_, '_>),
) {
    use petramond_ui::{solve, InstKey, InstTree, ThemeEnv};
    let doc = &screen.doc;
    let theme = crate::gui::doc_theme::theme();
    let hovers: Vec<Option<InstKey>> = std::iter::once(None)
        .chain(
            tooltip_anchors(&doc.root)
                .into_iter()
                .map(|id| Some(InstKey { id, item: Some(0) })),
        )
        .collect();
    for page in 0..page_count(screen) {
        let state = seeded_state(screen, seed, page);
        for hover in &hovers {
            for compact in [false, true] {
                let mut at = views
                    .iter()
                    .filter(|v| doc.compact_active(v.size.0) == compact)
                    .peekable();
                if at.peek().is_none() {
                    continue;
                }
                // The tree depends on the form, not the window: expand it
                // once and solve it in every view of that form.
                let tree = InstTree::expand_form_hover(doc, &state, compact, hover.as_ref());
                for &view in at {
                    let env = ThemeEnv {
                        theme: &theme,
                        gui_scale: view.scale,
                        image_size: &|_| None,
                    };
                    let solved = solve(&tree, &env, view.size, &|_| 0);
                    check(&SolvedView {
                        tree: &tree,
                        solved: &solved,
                        env: &env,
                        view,
                    });
                }
            }
        }
    }
}

/// [`solve_views`], one instance at a time.
fn walk_solved(
    screen: &Screen,
    views: &[View],
    seed: Seed,
    mut check: impl FnMut(SolvedNode<'_, '_>),
) {
    solve_views(screen, views, seed, |v| {
        for i in 0..v.tree.len() {
            check(SolvedNode {
                inst: v.tree.get(i as u32),
                rect: v.solved.rects[i],
                root: v.solved.rects[0],
                floating: v.solved.overlay[i],
                view: v.view,
            });
        }
    });
}

/// The widget ids every `hover`-anchored tooltip in a document hangs on.
fn tooltip_anchors(node: &Node) -> Vec<String> {
    let mut out = Vec::new();
    if let petramond_ui::NodeKind::Tooltip {
        hover: Some(anchor),
    } = &node.kind
    {
        out.push(anchor.clone());
    }
    for child in &node.children {
        out.extend(tooltip_anchors(child));
    }
    out.dedup();
    out
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
        let screen = engine_screen(kind).expect("the recipe documents ship");
        let mut state = seeded_state(&screen, Seed::Ordinary, 0);
        state.set("craft_tip_ingredients_w", UiValue::I32(ASKED));
        let viewport = petramond_ui::contract::SMALLEST_VIEWPORT;
        let tree =
            InstTree::expand_form(&screen.doc, &state, screen.doc.compact_active(viewport.0));
        let env = ThemeEnv {
            theme: &theme,
            gui_scale: 3,
            image_size: &|_| None,
        };
        let solved = solve(&tree, &env, viewport, &|_| 0);
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
/// Authored button captions are held to the same rule.
#[test]
fn authored_label_text_fits_the_box_the_document_gives_it() {
    use petramond_ui::{LayoutEnv, NodeKind, ThemeEnv};
    let screens = screens();
    let views = views(&screens);
    let theme = crate::gui::doc_theme::theme();
    let font = theme.ui_font();
    let mut clipped = Vec::new();
    for screen in &screens {
        let kind = &screen.name;
        walk_solved(screen, &views, Seed::Long, |n| {
            let (view, scale) = (n.view, n.view.scale);
            // An authored button caption is cut the same way when its box
            // is narrower than the caption with the face's padding.
            if let NodeKind::Button {
                text: Some(text),
                image: None,
                ..
            } = &n.inst.node.kind
            {
                let env = ThemeEnv {
                    theme: &theme,
                    gui_scale: scale,
                    image_size: &|_| None,
                };
                let need = env.leaf_size(n.inst.node, Some(text), None, None).0;
                if n.inst.node.bind.text.is_none() && need > n.rect.w {
                    clipped.push(format!(
                        "{kind} @{view}: button {text:?} needs {need}px, box is {}px",
                        n.rect.w
                    ));
                }
                return;
            }
            let NodeKind::Label {
                text: Some(text),
                wrap,
                scale: label_scale,
                small,
                ..
            } = &n.inst.node.kind
            else {
                return;
            };
            // A run draws at `k` physical px per font pixel — one step down
            // for `small`. Convert both ways the way the solver does,
            // rounding the reservation UP.
            let k = match (*label_scale, *small) {
                (heading, _) if heading > 1 => scale * heading as i32,
                (_, true) => (scale - 1).max(1),
                _ => scale,
            };
            let logical = |font_px: i32| (font_px * k + scale - 1) / scale;
            let font_px = |logical: i32| logical * scale / k;
            let (need, have) = match wrap {
                // A wrapping label is bounded by its box HEIGHT: it is the
                // fixed-height ones (the remap hint) that clip.
                true => (
                    logical(font.measure(text, Some(font_px(n.rect.w))).1),
                    n.rect.h,
                ),
                false => (logical(font.width(text)), n.rect.w),
            };
            if need > have {
                clipped.push(format!(
                    "{kind} @{view}: {text:?} needs {need}px, box is {have}px"
                ));
            }
        });
    }
    assert!(clipped.is_empty(), "clipped labels: {clipped:#?}");
}

/// However long the text that lands in a row, the row's WIDGETS stay on the
/// panel. Text is the layout's shock absorber (it ellipsizes); a checkbox
/// or a mod toggle pushed off the panel edge is unreachable, and a label
/// that keeps its natural width paints straight across the screen.
///
/// Tooltips float: the runtime places them at the pointer and clamps them
/// there, so the solver's parking spot says nothing about where they land.
/// What a floating panel owes is a bounded natural size: an unbounded one
/// covers the screen the moment a pack ships a long recipe name. The item
/// tip's cap is the ceiling — at the tightest 320px viewport that is a
/// panel beside the pointer, never a screen cover.
#[test]
fn long_dynamic_text_never_pushes_a_widget_off_its_screen() {
    let screens = screens();
    let views = views(&screens);
    let mut escaped = Vec::new();
    let mut unbounded = Vec::new();
    for screen in &screens {
        let kind = &screen.name;
        walk_solved(screen, &views, Seed::Long, |n| {
            if matches!(n.inst.node.kind, petramond_ui::NodeKind::Tooltip { .. })
                && n.rect.w > ITEM_TIP_MAX_W
            {
                unbounded.push(format!(
                    "{kind} @{}: floating panel is {}px wide",
                    n.view, n.rect.w
                ));
            }
            if n.floating || n.rect.w == 0 {
                return;
            }
            if n.rect.x < n.root.x || n.rect.x + n.rect.w > n.root.x + n.root.w {
                escaped.push(format!(
                    "{kind} @{}: {} spans {}..{} outside {}..{}",
                    n.view,
                    describe(n.inst.node),
                    n.rect.x,
                    n.rect.x + n.rect.w,
                    n.root.x,
                    n.root.x + n.root.w
                ));
            }
        });
    }
    assert!(escaped.is_empty(), "off-screen widgets: {escaped:#?}");
    assert!(unbounded.is_empty(), "unbounded tooltips: {unbounded:#?}");
}
