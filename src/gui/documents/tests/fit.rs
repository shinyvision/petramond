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

const WINDOWS: &[(u32, u32)] = &[
    (320, 240),
    (640, 480),
    (960, 720),
    (1280, 960),
    (1024, 768),
    (1280, 720),
    (1366, 768),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
    (5120, 2880),
    (800, 600),
    (1920, 1200),
    (2560, 1080),
    (3440, 1440),
    (1080, 1920),
];

fn views(screens: &[Screen]) -> Vec<View> {
    let mut windows = WINDOWS.to_vec();
    for w in screens.iter().filter_map(|s| s.doc.compact_below_w) {
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

#[derive(Clone, Copy, PartialEq)]
enum Seed {
    Long,
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
    &[
        "catalog_tab+browsing",
        "selection_tab+browsing",
        "library_tab+browsing",
        "confirming_delete",
    ],
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

fn group_keys(group: &[String]) -> impl Iterator<Item = &str> + '_ {
    group.iter().flat_map(|member| member.split('+'))
}

/// Which member of each group each page shows. Page 0 shows every group's first member; each later
/// page moves one group to another member. Every member gets shown next to the other groups' main
/// states and never beside a state it can't coexist with. Cycling all groups at once did that: the
/// empty-list state only ever showed up next to a confirm page.
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

fn page_count(screen: &Screen) -> usize {
    let state = base_state(screen, Seed::Ordinary);
    let groups = present_groups(screen, &state);
    1 + groups
        .iter()
        .map(|g| g.len().saturating_sub(1))
        .sum::<usize>()
}

const EMPTY_STATE_KEYS: &[&str] = &["no_mods", "no_craft_results"];

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

struct Screen {
    name: String,
    doc: Arc<Document>,
    catalog: serde_json::Value,
}

impl Screen {
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

fn screens() -> Vec<Screen> {
    let mut screens: Vec<Screen> = SHELL_KINDS
        .iter()
        .filter_map(|&k| engine_screen(k))
        .collect();
    screens.extend(pack_screens());
    screens
}

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

struct SolvedNode<'a, 'd> {
    inst: &'a petramond_ui::Inst<'d>,
    rect: petramond_ui::RectI,
    root: petramond_ui::RectI,
    floating: bool,
    view: View,
}

struct SolvedView<'a, 'd> {
    tree: &'a petramond_ui::InstTree<'d>,
    solved: &'a petramond_ui::Solved,
    env: &'a petramond_ui::ThemeEnv<'a>,
    view: View,
}

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

#[test]
fn the_recipe_tooltip_grows_to_the_published_ingredient_width() {
    use petramond_ui::{solve, InstTree, ThemeEnv, UiValue};
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
            let k = match (*label_scale, *small) {
                (heading, _) if heading > 1 => scale * heading as i32,
                (_, true) => (scale - 1).max(1),
                _ => scale,
            };
            let logical = |font_px: i32| (font_px * k + scale - 1) / scale;
            let font_px = |logical: i32| logical * scale / k;
            let (need, have) = match wrap {
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

/// Text can be as long as it likes, but the row's widgets stay on the panel; the text ellipsizes.
/// A checkbox pushed off the edge can't be clicked, and a label at natural width runs across the
/// whole screen.
///
/// Tooltips float and get clamped to the pointer at runtime, so solver placement doesn't matter for
/// them. Their natural size still needs a bound, or one long recipe name covers the screen. The
/// item tip cap is that bound, and at 320px it's still a panel by the pointer.
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
