use std::time::{Duration, Instant};

use petramond::content::{Dirs, InstallRecord, Kind, ListingRow, Tier};
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;
use petramond_ui::{solve, InstKey, InstTree, LayoutEnv, ThemeEnv, UiState, UiValue};
use petramond_util::test_dirs::TestScratchDir;
use petramond_world::gui_state::GuiKind;

use super::rows::{Action, Local};
use super::*;
use crate::app::content::{ContentSession, Pending, PendingKind, Phase};
use crate::app::ExitKind;

fn listing_row(id: &str, name: &str, kind: Kind, sha: char, bytes: u64) -> ListingRow {
    ListingRow {
        content_id: 1,
        mod_id: id.to_owned(),
        kind,
        name: name.to_owned(),
        summary: format!("{name} does things."),
        description: format!("{name} does things, at length."),
        version: "1.1.0".to_owned(),
        byte_size: bytes,
        sha256: sha.to_string().repeat(64),
        download_path: format!("/api/v1/content/1/download?{id}"),
        icon_path: None,
    }
}

fn local(id: &str, name: &str, tier: Tier) -> Local {
    Local {
        key: id.to_owned(),
        dir: id.to_owned(),
        id: Some(id.to_owned()),
        name: name.to_owned(),
        summary: format!("{name} does things."),
        description: String::new(),
        version: Some("1.0.0".to_owned()),
        tier,
        refusal: None,
        record: None,
        touches_world: true,
        dependencies: Vec::new(),
    }
}

fn record(id: &str, kind: Kind, sha: char, content_id: Option<i64>) -> InstallRecord {
    InstallRecord {
        format: 1,
        id: id.to_owned(),
        kind,
        content_id,
        name: id.to_owned(),
        version: "1.0.0".to_owned(),
        archive_sha256: sha.to_string().repeat(64),
        archive_bytes: 20 * 1024 * 1024,
        pack_json_sha256: "0".repeat(64),
        installed_ms: 0,
    }
}

fn in_ctx<R>(app: &mut App, f: impl FnOnce(&mut ScreenCtx) -> R) -> R {
    let now = app.now();
    let mut ctx = ScreenCtx::new(
        &mut app.shell,
        &mut app.options,
        (&mut app.content, &app.content_report),
        &app.controls.action_table,
        &mut app.ui,
        super::super::SessionFacts::default(),
        now,
    );
    let out = f(&mut ctx);
    for command in ctx.into_commands() {
        app.run_shell_command(command);
    }
    out
}

fn app_with(tag: &str, locals: Vec<Local>, signed_in: bool) -> (TestScratchDir, App) {
    let root = TestScratchDir::new(&format!("content-browser-{tag}"));
    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.content = ContentSession::for_test(Dirs {
        mods: root.join("mods"),
        content: root.join("content"),
    });
    if signed_in {
        app.shell.account.saved = Some(petramond::account::SavedSignIn {
            username: "explorer".to_owned(),
            ..Default::default()
        });
    }
    app.open_content(None, None);
    app.content.view.as_mut().unwrap().locals = locals;
    app.content.view.as_mut().unwrap().request_rebuild();
    assert!(in_ctx(&mut app, prepare));
    (root, app)
}

fn view(app: &App) -> &ContentView {
    app.content.view.as_ref().unwrap()
}

fn entry_for(app: &App, key: &str) -> Entry {
    let view = view(app);
    let slot = view
        .shown
        .iter()
        .find(|s| view.key_of(s).as_deref() == Some(key))
        .unwrap_or_else(|| panic!("{key} is shown"));
    view.entry(slot, &app.content, Instant::now()).unwrap()
}

fn show(app: &mut App, tab: Tab) {
    app.content.view.as_mut().unwrap().show_tab(tab);
    assert!(in_ctx(app, prepare));
}

fn keys(app: &App) -> Vec<String> {
    let view = view(app);
    view.shown.iter().filter_map(|s| view.key_of(s)).collect()
}

#[test]
fn content_packs_are_never_listed_and_only_addons_wear_the_sheep() {
    let mut addon = local("studio", "Studio", Tier::Addon);
    addon.record = Some(record("studio", Kind::Addon, 'a', Some(1)));
    let mut forge = local("forge", "Forge", Tier::ContentPack);
    forge.refusal = Some("missing wasm".to_owned());
    let (_root, mut app) = app_with(
        "tiers",
        vec![
            local("forge_ok", "Anvil", Tier::ContentPack),
            forge,
            addon,
            local("tweaks", "Tweaks", Tier::Mod),
        ],
        true,
    );
    assert_eq!(
        keys(&app),
        ["studio", "tweaks"],
        "Installed: no content pack"
    );
    assert!(entry_for(&app, "studio").is_addon);
    assert!(entry_for(&app, "studio").can_delete);
    assert!(!entry_for(&app, "tweaks").is_addon);
    assert!(entry_for(&app, "tweaks").can_delete);

    app.content.listing_arrived(Ok(vec![
        listing_row("forge_ok", "Anvil", Kind::Addon, 'a', 4096),
        listing_row("studio", "Studio", Kind::Addon, 'a', 4096),
    ]));
    show(&mut app, Tab::Browse);
    assert_eq!(keys(&app), ["studio"]);
}

#[test]
fn a_row_shows_its_job_before_its_pending_change_update_or_refusal() {
    let mut studio = local("studio", "Studio", Tier::Addon);
    studio.record = Some(record("studio", Kind::Addon, 'a', Some(1)));
    studio.refusal = Some("needs sky_core".to_owned());
    let (_root, mut app) = app_with("precedence", vec![studio], true);
    app.content.listing_arrived(Ok(vec![listing_row(
        "studio",
        "Studio",
        Kind::Addon,
        'b',
        4096,
    )]));

    let entry = entry_for(&app, "studio");
    assert_eq!(entry.action, Action::Update);
    assert_eq!(entry.detail, "Not loaded · update");

    app.content.pending.push(Pending {
        dir: "studio".to_owned(),
        change: PendingKind::Install {
            name: "Studio".to_owned(),
            version: "1.1.0".to_owned(),
            touches_world: false,
            dependencies: Vec::new(),
        },
    });
    let entry = entry_for(&app, "studio");
    assert_eq!(entry.action, Action::Undo);
    assert_eq!(entry.detail, "Ready to update");
    assert_eq!(entry.action_tip, "Stay on v1.0.0");

    app.content.pending.clear();
    let row = listing_row("studio", "Studio", Kind::Addon, 'b', 4096);
    app.content.get(&row).unwrap();
    assert!(matches!(entry_for(&app, "studio").action, Action::Busy(_)));
}

#[test]
fn apply_button_requests_a_content_switch_in_the_running_app() {
    let (_root, mut app) = app_with("apply-button", Vec::new(), false);
    app.content.pending.push(Pending {
        dir: "studio".into(),
        change: PendingKind::Remove,
    });
    in_ctx(&mut app, |ctx| click(ctx, "apply", None));
    assert!(app.take_content_apply_requested());
    assert!(!app.take_content_apply_requested());

    app.set_content_report(petramond::content::ApplyReport {
        deferred: true,
        ..Default::default()
    });
    let state = state_of(&mut app);
    assert!(
        matches!(state.get("apply_status"), Some(UiValue::Str(text)) if text.contains("Close other Petramond windows"))
    );
    assert_fits(GuiKind::Content, &state, None, "deferred apply");
}

#[test]
fn entry_text_width_and_action_edge_stay_fixed_across_get_delete_and_undo() {
    let (_root, mut app) = app_with(
        "stable-action-width",
        vec![local("studio", "Studio", Tier::Addon)],
        false,
    );
    let installed = state_of(&mut app);
    app.content.pending.push(Pending {
        dir: "studio".into(),
        change: PendingKind::Remove,
    });
    let pending = state_of(&mut app);
    let (_browse_root, mut browsing) = app_with("get-right", Vec::new(), true);
    browsing.content.listing_arrived(Ok(vec![listing_row(
        "studio",
        "Studio",
        Kind::Addon,
        'a',
        631_856,
    )]));
    show(&mut browsing, Tab::Browse);
    let available = state_of(&mut browsing);
    let geometry = |state: &UiState, action_id: &str, viewport| {
        let mut result = None;
        solve_kind_at(
            GuiKind::Content,
            state,
            None,
            viewport,
            |tree, solved, _| {
                let visible_rect = |id: &str| {
                    (0..tree.len())
                        .find(|&i| {
                            tree.get(i as u32).node.id.as_deref() == Some(id)
                                && solved.rects[i].w > 0
                        })
                        .map(|i| solved.rects[i])
                        .unwrap()
                };
                let detail = visible_rect("detail");
                let action = visible_rect(action_id);
                result = Some(((detail.x, detail.w), (action.x, action.w, action.h)));
            },
        );
        result.unwrap()
    };
    for viewport in [VIEWPORT, (640, 360)] {
        let delete = geometry(&installed, "delete", viewport);
        let undo = geometry(&pending, "undo", viewport);
        let get = geometry(&available, "get", viewport);
        assert_eq!(delete, undo, "{viewport:?}");
        assert_eq!(delete.0, get.0, "description width at {viewport:?}");
        let (delete_action, get_action) = (delete.1, get.1);
        assert_eq!(
            delete_action.0 + delete_action.1,
            get_action.0 + get_action.1,
            "action right edge at {viewport:?}"
        );
    }
}

#[test]
fn content_rows_keep_their_width_when_the_scrollbar_appears() {
    let (_root, mut app) = app_with(
        "stable-row-width",
        vec![
            local("studio", "Studio", Tier::Addon),
            local("tweaks", "Tweaks", Tier::Mod),
        ],
        false,
    );
    let row_width = |state: &UiState| {
        let mut width = 0;
        solve_state(state, None, |tree, solved, _| {
            width = (0..tree.len())
                .filter(|&i| tree.get(i as u32).node.style.as_deref() == Some("list.row"))
                .map(|i| solved.rects[i].w)
                .find(|&w| w > 0)
                .unwrap();
        });
        width
    };
    app.content.view.as_mut().unwrap().search = "Studio".into();
    let one = row_width(&state_of(&mut app));
    app.content.view.as_mut().unwrap().search.clear();
    let two = row_width(&state_of(&mut app));
    assert_eq!(one, two);
}

#[test]
fn content_panel_width_is_stable_across_list_and_confirmation_states() {
    let (_root, mut app) = app_with(
        "stable-page-width",
        vec![
            local("studio", "Studio", Tier::Addon),
            local("tweaks", "Tweaks", Tier::Mod),
        ],
        false,
    );
    for viewport in [VIEWPORT, (640, 360)] {
        let mut widths = Vec::new();
        for state in [
            state_of(&mut app),
            {
                show(&mut app, Tab::Browse);
                state_of(&mut app)
            },
            {
                app.content.view.as_mut().unwrap().confirm = Some(confirm::ConfirmPage {
                    question: "Delete this addon?".into(),
                    body: "This change cannot be undone.".into(),
                    variant: confirm::Variant::Destroy,
                    action_text: "Delete",
                    now_text: "",
                    confirmed: confirm::Confirmed::Delete {
                        dir: "studio".into(),
                    },
                });
                state_of(&mut app)
            },
        ] {
            solve_kind_at(
                GuiKind::Content,
                &state,
                None,
                viewport,
                |tree, solved, _| {
                    let panel = (0..tree.len())
                        .find(|&i| tree.get(i as u32).node.style.as_deref() == Some("panel.large"))
                        .map(|i| solved.rects[i])
                        .unwrap();
                    let scroll = (0..tree.len())
                        .find(|&i| {
                            let id = tree.get(i as u32).node.id.as_deref();
                            matches!(id, Some("content_scroll" | "confirm_scroll"))
                                && solved.rects[i].w > 0
                        })
                        .map(|i| solved.rects[i])
                        .unwrap();
                    widths.push(((panel.x, panel.w), (scroll.x, scroll.w)));
                },
            );
        }
        assert!(
            widths.windows(2).all(|pair| pair[0] == pair[1]),
            "{viewport:?}: {widths:?}"
        );
    }
}

#[test]
fn empty_installed_browse_fills_the_bottom_action_row() {
    let (_root, mut app) = app_with("empty-browse", Vec::new(), false);
    let state = state_of(&mut app);
    for viewport in [VIEWPORT, (640, 360)] {
        solve_kind_at(
            GuiKind::Content,
            &state,
            None,
            viewport,
            |tree, solved, _| {
                let rect = |id: &str| {
                    (0..tree.len())
                        .find(|&i| tree.get(i as u32).node.id.as_deref() == Some(id))
                        .map(|i| solved.rects[i])
                        .unwrap()
                };
                let search = rect("search");
                let scroll = rect("content_scroll");
                let browse = rect("browse_empty");
                let back = rect("back");
                let account = rect("account");
                assert_eq!((browse.x, browse.w), (search.x, search.w));
                assert_eq!((browse.x, browse.w), (scroll.x, scroll.w));
                assert!(
                    back.y - (browse.y + browse.h) <= 10,
                    "browse {browse:?} should sit directly above the footer {back:?}"
                );
                assert_eq!(account.h, back.h);
            },
        );
    }
    in_ctx(&mut app, |ctx| click(ctx, "browse_empty", None));
    assert_eq!(view(&app).tab, Tab::Browse);
}

#[test]
fn a_replace_over_a_folder_named_otherwise_shows_as_pending() {
    let mut hand = local("lanterns", "Lanterns", Tier::Mod);
    hand.dir = "lanterns-main".to_owned();
    let (_root, mut app) = app_with("replace-renamed", vec![hand], true);
    app.content.listing_arrived(Ok(vec![listing_row(
        "lanterns",
        "Lanterns",
        Kind::Mod,
        'b',
        4096,
    )]));
    assert_eq!(entry_for(&app, "lanterns").action, Action::Replace);

    app.content.pending.push(Pending {
        dir: "lanterns".to_owned(),
        change: PendingKind::Install {
            name: "Lanterns".to_owned(),
            version: "1.1.0".to_owned(),
            touches_world: true,
            dependencies: Vec::new(),
        },
    });
    let entry = entry_for(&app, "lanterns");
    assert_eq!(entry.action, Action::Undo);
    assert_eq!(entry.detail, "Ready to update");
}

#[test]
fn an_update_is_decided_by_the_archive_hash_never_the_version() {
    let mut same_version = local("studio", "Studio", Tier::Addon);
    same_version.record = Some(record("studio", Kind::Addon, 'a', Some(1)));
    let mut dev = local("dev", "Dev", Tier::Addon);
    dev.record = Some(record("dev", Kind::Addon, 'a', None));
    let loose = local("hand", "Hand", Tier::Mod);
    let (_root, mut app) = app_with("update", vec![same_version, dev, loose], true);
    assert_eq!(entry_for(&app, "studio").action, Action::None);
    assert!(entry_for(&app, "studio").detail.starts_with("Installed · "));

    let mut rows = vec![
        listing_row("studio", "Studio", Kind::Addon, 'b', 4096),
        listing_row("dev", "Dev", Kind::Addon, 'b', 4096),
        listing_row("hand", "Hand", Kind::Mod, 'b', 4096),
    ];
    for row in &mut rows {
        row.version = "1.0.0".to_owned();
    }
    app.content.listing_arrived(Ok(rows));
    assert_eq!(entry_for(&app, "studio").action, Action::Update);
    assert_eq!(
        entry_for(&app, "dev").action,
        Action::None,
        "a local install never updates"
    );
    assert_eq!(entry_for(&app, "dev").detail, "Installed locally");
    assert_eq!(entry_for(&app, "hand").action, Action::Replace);

    app.content.listing_arrived(Ok(Vec::new()));
    assert_eq!(entry_for(&app, "studio").detail, "No longer listed");
    app.content.listing = crate::app::content::ListingState::Failed("down".into());
    assert_ne!(
        entry_for(&app, "studio").detail,
        "No longer listed",
        "a failed listing says nothing about what the site has"
    );
}

#[test]
fn a_busy_site_is_a_wait_never_a_failure() {
    let (_root, mut app) = app_with("busy", Vec::new(), true);
    let row = listing_row("sky", "Sky Tools", Kind::Mod, 'c', 470 * 1024);
    app.content.listing_arrived(Ok(vec![row.clone()]));
    show(&mut app, Tab::Browse);
    app.content.get(&row).unwrap();
    let until = Instant::now() + Duration::from_secs(12);
    app.content
        .jobs
        .set_phase("sky", Phase::Waiting { until }, 100 * 1024);
    let entry = entry_for(&app, "sky");
    assert!(matches!(entry.action, Action::Busy(_)));
    assert_eq!(entry.detail_palette, rows::WARN);
    assert!(entry.detail.starts_with("Retrying in "), "{}", entry.detail);
    assert!(!app.content.failed.contains_key("sky"));
}

#[test]
fn merges_never_add_move_or_drop_a_row_until_a_refresh() {
    let (_root, mut app) = app_with(
        "merge",
        vec![local("forge", "Forge", Tier::ContentPack)],
        true,
    );
    let first = vec![
        listing_row("sky", "Sky Tools", Kind::Mod, 'c', 4096),
        listing_row("studio", "Studio", Kind::Addon, 'd', 4096),
    ];
    show(&mut app, Tab::Browse);
    app.content.listing_arrived(Ok(first));
    assert!(in_ctx(&mut app, prepare));
    assert_eq!(keys(&app), ["sky", "studio"]);

    app.content
        .listing_arrived(Ok(vec![listing_row("new", "New", Kind::Mod, 'e', 4096)]));
    assert!(in_ctx(&mut app, prepare));
    assert_eq!(keys(&app), ["sky", "studio"]);

    in_ctx(&mut app, refresh);
    app.content
        .listing_arrived(Ok(vec![listing_row("new", "New", Kind::Mod, 'e', 4096)]));
    assert!(in_ctx(&mut app, prepare));
    assert_eq!(keys(&app), ["new"]);
}

#[test]
fn keyboard_selection_walks_entries_only_and_a_double_click_never_flickers() {
    let (_root, mut app) = app_with(
        "keys",
        vec![
            local("tweaks", "Tweaks", Tier::Mod),
            local("zebra", "Zebra", Tier::Mod),
        ],
        false,
    );
    in_ctx(&mut app, |ctx| key(ctx, NavKey::Down, false));
    assert_eq!(view(&app).selected.as_deref(), Some("tweaks"));
    in_ctx(&mut app, |ctx| key(ctx, NavKey::Down, false));
    assert_eq!(view(&app).selected.as_deref(), Some("zebra"));
    in_ctx(&mut app, |ctx| key(ctx, NavKey::Down, false));
    assert_eq!(view(&app).selected.as_deref(), Some("zebra"));

    show(&mut app, Tab::Browse);
    in_ctx(&mut app, |ctx| {
        handle(
            ctx,
            UiEvent::ListSelect {
                id: LIST.into(),
                index: 0,
            },
        )
    });
    assert_eq!(view(&app).selected, None, "a message is not selectable");

    show(&mut app, Tab::Installed);
    let index = view(&app)
        .shown
        .iter()
        .position(|s| view(&app).key_of(s).as_deref() == Some("tweaks"))
        .unwrap() as u32;
    in_ctx(&mut app, |ctx| {
        handle(
            ctx,
            UiEvent::ListSelect {
                id: LIST.into(),
                index,
            },
        )
    });
    in_ctx(&mut app, |ctx| {
        handle(
            ctx,
            UiEvent::ListActivate {
                id: LIST.into(),
                index,
            },
        )
    });
    assert!(
        !view(&app).expanded.contains("tweaks"),
        "the double click's toggle was undone"
    );
    assert!(in_ctx(&mut app, prepare));
    in_ctx(&mut app, |ctx| {
        handle(
            ctx,
            UiEvent::ListSelect {
                id: LIST.into(),
                index,
            },
        )
    });
    assert!(view(&app).expanded.contains("tweaks"));
}

#[test]
fn with_installs_off_no_row_offers_to_install() {
    let (_root, mut app) = app_with("off", vec![local("hand", "Hand", Tier::Mod)], true);
    app.content.installs_enabled = false;
    app.content.listing_arrived(Ok(vec![
        listing_row("sky", "Sky Tools", Kind::Mod, 'c', 4096),
        listing_row("hand", "Hand", Kind::Mod, 'c', 4096),
    ]));
    show(&mut app, Tab::Browse);
    assert_eq!(entry_for(&app, "sky").action, Action::None);
    assert_eq!(entry_for(&app, "hand").action, Action::None);
    assert!(app
        .content
        .get(&listing_row("sky", "Sky", Kind::Mod, 'c', 4096))
        .is_err());
}

#[test]
fn quitting_with_downloads_running_asks_first_and_can_wait_for_them() {
    let (_root, mut app) = app_with("exit", Vec::new(), true);
    let row = listing_row("sky", "Sky Tools", Kind::Mod, 'c', 4096);
    app.content.get(&row).unwrap();
    app.request_exit(ExitKind::Quit);
    assert!(
        !app.take_quit_requested(),
        "a running download is never dropped unasked"
    );
    assert_eq!(app.screen, AppScreen::Content);
    assert!(view(&app).confirm.is_some());

    in_ctx(&mut app, |ctx| {
        handle(
            ctx,
            UiEvent::Click {
                id: "confirm_go".into(),
                item: None,
                button: petramond_ui::PointerButton::Primary,
            },
        )
    });
    assert!(!app.take_quit_requested());
    app.content.jobs.cancel("sky");
    app.poll_content();
    assert!(app.take_quit_requested());
}

const VIEWPORT: (i32, i32) = (320, 240);

fn solve_state(
    state: &UiState,
    hover: Option<&str>,
    f: impl FnMut(&InstTree<'_>, &petramond_ui::Solved, &ThemeEnv<'_>),
) {
    solve_kind(GuiKind::Content, state, hover, f)
}

fn solve_kind(
    kind: GuiKind,
    state: &UiState,
    hover: Option<&str>,
    f: impl FnMut(&InstTree<'_>, &petramond_ui::Solved, &ThemeEnv<'_>),
) {
    solve_kind_at(kind, state, hover, VIEWPORT, f)
}

fn solve_kind_at(
    kind: GuiKind,
    state: &UiState,
    hover: Option<&str>,
    viewport: (i32, i32),
    mut f: impl FnMut(&InstTree<'_>, &petramond_ui::Solved, &ThemeEnv<'_>),
) {
    let doc = petramond::gui::documents::doc_for(kind).expect("document loads");
    let theme = petramond::gui::doc_theme::theme();
    let hover = hover.map(|id| InstKey {
        id: id.to_owned(),
        item: None,
    });
    let tree = InstTree::expand_form_hover(&doc.doc, state, false, hover.as_ref());
    let env = ThemeEnv {
        theme: &theme,
        gui_scale: 1,
        image_size: &|_| None,
    };
    let solved = solve(&tree, &env, viewport, &|_| 0);
    f(&tree, &solved, &env);
}

fn state_of(app: &mut App) -> UiState {
    assert!(in_ctx(app, prepare));
    let mut state = UiState::new();
    in_ctx(app, |ctx| populate(ctx, &mut state));
    state
}

fn assert_fits(kind: GuiKind, state: &UiState, hover: Option<&str>, what: &str) {
    let mut overflowing = Vec::new();
    solve_kind(kind, state, hover, |tree, solved, env| {
        for i in 0..tree.len() {
            let inst = tree.get(i as u32);
            let Some(p) = inst.parent else { continue };
            let mut scrolled = false;
            let mut up = Some(p);
            while let Some(a) = up {
                scrolled |= matches!(tree.get(a).node.kind, petramond_ui::NodeKind::Scroll { .. });
                up = tree.get(a).parent;
            }
            let rect = solved.rects[i];
            if scrolled || solved.overlay[i] || inst.layout.abs.is_some() || rect.h == 0 {
                continue;
            }
            let parent = tree.get(p);
            let border = env.container_insets(parent.node);
            let pad = parent.layout.pad;
            let bx = solved.rects[p as usize].inset(std::array::from_fn(|k| pad[k] + border[k]));
            if rect.y < bx.y
                || rect.y + rect.h > bx.y + bx.h
                || rect.x < bx.x
                || rect.x + rect.w > bx.x + bx.w
            {
                overflowing.push(format!(
                    "{what}: {:?} {:?} outside {bx:?}",
                    inst.node.kind, rect
                ));
            }
        }
    });
    assert!(overflowing.is_empty(), "{}", overflowing.join("\n"));
}

fn shrunk_labels(state: &UiState, id: &str) -> Vec<String> {
    let theme = petramond::gui::doc_theme::theme();
    let mut shrunk = Vec::new();
    solve_state(state, None, |tree, solved, _| {
        for i in 0..tree.len() {
            let inst = tree.get(i as u32);
            if inst.node.id.as_deref() != Some(id) || solved.rects[i].w == 0 {
                continue;
            }
            let text = inst.text.as_deref().unwrap_or("");
            let ink = theme.ui_font().width(text);
            if ink > solved.rects[i].w {
                shrunk.push(format!("{text:?} needs {ink}px, has {}", solved.rects[i].w));
            }
        }
    });
    shrunk
}

const LONG_NAME: &str = "A craftable rideable wooden chair with directional iron chains, a light-giving chandelier, a slate cauldron and a very long name";

#[test]
fn the_fixed_detail_copy_never_ellipsizes_at_the_smallest_viewport() {
    const MAX: u64 = 20 * 1024 * 1024;
    let named = |id: &str, tier| {
        let mut l = local(id, LONG_NAME, tier);
        l.version = Some("1.0.0-beta.1".to_owned());
        l
    };
    let mut installed = named("installed", Tier::Addon);
    installed.record = Some(record("installed", Kind::Addon, 'a', Some(1)));
    let mut update = named("update", Tier::Addon);
    update.record = Some(record("update", Kind::Addon, 'a', Some(1)));
    update.refusal = Some("needs a pack that is not installed".to_owned());
    let mut local_dev = named("local_dev", Tier::Addon);
    local_dev.record = Some(record("local_dev", Kind::Addon, 'a', None));
    let (_root, mut app) = app_with(
        "detail",
        vec![
            installed,
            update,
            local_dev,
            named("hand", Tier::Mod),
            named("forge", Tier::ContentPack),
        ],
        true,
    );
    let mut rows: Vec<ListingRow> = ["queued", "down", "wait", "check", "avail", "update"]
        .iter()
        .map(|id| listing_row(id, LONG_NAME, Kind::Addon, 'f', MAX))
        .collect();
    rows.push(listing_row("installed", LONG_NAME, Kind::Addon, 'a', MAX));
    for row in &mut rows {
        row.version = "1.0.0-beta.1".to_owned();
    }
    app.content.listing_arrived(Ok(rows.clone()));
    for row in &rows[..4] {
        app.content.get(row).unwrap();
    }
    app.content.jobs.set_phase("down", Phase::Downloading, MAX);
    app.content.jobs.set_phase(
        "wait",
        Phase::Waiting {
            until: Instant::now() + Duration::from_secs(300),
        },
        MAX,
    );
    app.content.jobs.set_phase("check", Phase::Checking, MAX);
    let row_details = |state: &UiState| -> Vec<String> {
        match state.get("rows") {
            Some(UiValue::List(rows)) => rows
                .iter()
                .filter_map(|r| match r.get("detail") {
                    Some(UiValue::Str(s)) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    show(&mut app, Tab::Browse);
    let mut details = row_details(&state_of(&mut app));
    assert_fits(GuiKind::Content, &state_of(&mut app), None, "browse");
    show(&mut app, Tab::Installed);
    let mut state = state_of(&mut app);
    details.extend(row_details(&state));
    for expected in [
        "100% · 20.0/20.0 MB",
        "Retrying in 300 s",
        "20.0 MB",
        "Installed · 20.0 MB",
        "Not loaded · update",
        "Installed locally",
        "Installed by hand",
        "Queued",
        "Checking…",
    ] {
        assert!(
            details.iter().any(|d| d == expected),
            "{expected} is on a row: {details:?}"
        );
    }
    assert!(
        shrunk_labels(&state, "detail").is_empty(),
        "{:#?}",
        shrunk_labels(&state, "detail")
    );
    assert_fits(GuiKind::Content, &state, None, "list page");

    app.content.jobs.cancel_all();
    for (dir, change) in [
        (
            "avail",
            PendingKind::Install {
                name: LONG_NAME.into(),
                version: "1.0.0".into(),
                touches_world: true,
                dependencies: Vec::new(),
            },
        ),
        (
            "queued",
            PendingKind::Install {
                name: LONG_NAME.into(),
                version: "1.0.0".into(),
                touches_world: false,
                dependencies: Vec::new(),
            },
        ),
        ("hand", PendingKind::Remove),
    ] {
        app.content.pending.push(Pending {
            dir: dir.into(),
            change,
        });
    }
    for key in ["avail", "queued", "hand"] {
        app.content
            .view
            .as_mut()
            .unwrap()
            .expanded
            .insert(key.into());
    }
    state = state_of(&mut app);
    assert!(shrunk_labels(&state, "detail").is_empty());
    assert_fits(GuiKind::Content, &state, None, "staged installed");
    show(&mut app, Tab::Browse);
    state = state_of(&mut app);
    assert!(
        shrunk_labels(&state, "detail").is_empty(),
        "{:#?}",
        shrunk_labels(&state, "detail")
    );
    assert_fits(GuiKind::Content, &state, None, "staged");
    let notes: Vec<String> = match state.get("rows") {
        Some(UiValue::List(rows)) => rows
            .iter()
            .filter_map(|r| match r.get("world_note") {
                Some(UiValue::Str(s)) if !s.is_empty() => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    assert!(notes.contains(&rows::WORLD_NOTE_TOUCHES.to_owned()));
    assert!(notes.contains(&rows::WORLD_NOTE_PRESENTATION.to_owned()));

    for i in 0..20 {
        app.content.pending.push(Pending {
            dir: format!("more_{i}"),
            change: PendingKind::Install {
                name: LONG_NAME.into(),
                version: "1".into(),
                touches_world: true,
                dependencies: Vec::new(),
            },
        });
    }
    state = state_of(&mut app);
    solve_state(&state, Some("apply"), |tree, solved, _| {
        let tip = (0..tree.len())
            .find(|&i| tree.get(i as u32).node.id.as_deref() == Some("apply_tooltip"))
            .expect("the apply tooltip");
        assert!(
            solved.rects[tip].h > 0 && solved.rects[tip].h < VIEWPORT.1,
            "{:?}",
            solved.rects[tip]
        );
    });
}

#[test]
fn the_messages_and_confirm_pages_fit_the_smallest_viewport_with_their_real_copy() {
    use crate::app::content::ListingState as L;
    let (_root, mut app) = app_with(
        "messages",
        vec![local("forge", "Forge", Tier::ContentPack)],
        true,
    );
    app.content.installs_enabled = false;
    app.content.view.as_mut().unwrap().filter =
        Some((0..9).map(|i| format!("missing_pack_{i}")).collect());
    show(&mut app, Tab::Browse);
    for listing in [
        L::SignedOut { expired: false },
        L::SignedOut { expired: true },
        L::Loading,
        L::Failed("petramond.com did not respond".into()),
        L::Cancelled,
        L::Ready,
    ] {
        app.content.listing = listing.clone();
        app.content.rows_known = listing == L::Ready;
        let state = state_of(&mut app);
        assert_fits(GuiKind::Content, &state, None, &format!("{listing:?}"));
        assert_fits(
            GuiKind::Content,
            &state,
            Some("message_action"),
            &format!("{listing:?} tip"),
        );
    }

    let worlds: Vec<String> = (0..15).map(|i| format!("{} {i}", "w".repeat(40))).collect();
    let dependents: Vec<String> = (0..5).map(|_| LONG_NAME.to_owned()).collect();
    let body = confirm::delete_body(true, &worlds, &dependents);
    assert!(
        body.contains("Used by 15 worlds: ") && body.contains(" and 12 more"),
        "{body}"
    );
    let delete = confirm::ConfirmPage {
        question: format!("Delete {}?", rows::cap(LONG_NAME, rows::NAME_CAP)),
        body,
        variant: confirm::Variant::Destroy,
        action_text: "Delete",
        now_text: "",
        confirmed: confirm::Confirmed::Delete {
            dir: "forge".into(),
        },
    };
    let restart = crate::app::content::StartRoute::content(None);
    let (question, body, go, now) = confirm::exit_copy(
        &ExitKind::Restart {
            route: restart.clone(),
        },
        2,
    );
    let exit = confirm::ConfirmPage {
        question,
        body,
        variant: confirm::Variant::Exit,
        action_text: go,
        now_text: now,
        confirmed: confirm::Confirmed::Exit {
            kind: ExitKind::Restart { route: restart },
        },
    };
    for page in [delete, exit] {
        app.content.view.as_mut().unwrap().confirm = Some(page.clone());
        let state = state_of(&mut app);
        assert_fits(GuiKind::Content, &state, None, &page.question);
        let theme = petramond::gui::doc_theme::theme();
        solve_state(&state, None, |tree, solved, _| {
            for i in 0..tree.len() {
                let inst = tree.get(i as u32);
                let id = inst.node.id.as_deref().unwrap_or("");
                if !id.starts_with("confirm_") || solved.rects[i].w == 0 {
                    continue;
                }
                if let Some(text) = inst.text.as_deref() {
                    let ink = theme.ui_font().width(text) + 2 * theme.metrics.button_pad;
                    assert!(
                        ink <= solved.rects[i].w,
                        "{id} {text:?} needs {ink}px, has {}",
                        solved.rects[i].w
                    );
                }
            }
        });
    }
}

#[test]
fn the_title_notice_fits_under_the_menu_with_its_real_copy() {
    let (_root, mut app) = app_with("notice", Vec::new(), true);
    let mut lines = Vec::new();
    app.set_content_report(petramond::content::ApplyReport {
        failed: vec![("studio".into(), "a file is in use".into())],
        ..Default::default()
    });
    lines.push(crate::app::shell_docs::title::notice(&app.content_report, &app.content).0);
    app.set_content_report(petramond::content::ApplyReport {
        deferred: true,
        ..Default::default()
    });
    lines.push(crate::app::shell_docs::title::notice(&app.content_report, &app.content).0);
    app.set_content_report(Default::default());
    for i in 0..20 {
        app.content
            .get(&listing_row(&format!("p{i}"), "P", Kind::Mod, 'c', 4096))
            .unwrap();
    }
    lines.push(crate::app::shell_docs::title::notice(&app.content_report, &app.content).0);
    app.content.jobs.cancel_all();
    app.content.pending.push(Pending {
        dir: "x".into(),
        change: PendingKind::Remove,
    });
    lines.push(crate::app::shell_docs::title::notice(&app.content_report, &app.content).0);
    let theme = petramond::gui::doc_theme::theme();
    for line in lines {
        let mut state = UiState::new();
        state.set("has_notice", UiValue::Bool(true));
        state.set("notice", UiValue::Str(line.clone()));
        assert_fits(GuiKind::Title, &state, None, &line);
        solve_kind(GuiKind::Title, &state, None, |tree, solved, _| {
            let i = (0..tree.len())
                .find(|&i| tree.get(i as u32).node.id.as_deref() == Some("notice"))
                .unwrap();
            let ink = theme.ui_font().width(&line);
            assert!(
                ink <= solved.rects[i].w,
                "{line:?} needs {ink}px, has {}",
                solved.rects[i].w
            );
        });
    }
}

#[test]
fn the_list_rows_never_overlap() {
    use crate::app::content::ListingState as L;
    let (_root, mut app) = app_with(
        "rows-stack",
        vec![
            Local {
                summary: "Record your world and film it from any angle.".into(),
                ..local("studio", "Studio", Tier::Addon)
            },
            local("weather", "Weather", Tier::ContentPack),
        ],
        true,
    );
    app.content.listing = L::Failed("petramond.com refused the request".into());
    for tab in [Tab::Installed, Tab::Browse] {
        show(&mut app, tab);
        let state = state_of(&mut app);
        for (viewport, gui_scale) in [
            ((320, 240), 1),
            ((640, 360), 1),
            ((880, 520), 2),
            ((586, 346), 3),
            ((1280, 720), 1),
            ((853, 480), 3),
            ((1280, 720), 2),
            ((960, 540), 2),
            ((640, 360), 4),
            ((480, 270), 4),
        ] {
            let doc = petramond::gui::documents::doc_for(GuiKind::Content).expect("document loads");
            let theme = petramond::gui::doc_theme::theme();
            let compact = doc.doc.compact_active(viewport.0);
            let tree = InstTree::expand_form_hover(&doc.doc, &state, compact, None);
            let env = ThemeEnv {
                theme: &theme,
                gui_scale,
                image_size: &|_| None,
            };
            let solved = solve(&tree, &env, viewport, &|_| 0);
            let list = (0..tree.len() as u32)
                .find(|&i| tree.get(i).key.as_ref().is_some_and(|k| k.id == "content"))
                .expect("the list");
            fn bottom(tree: &InstTree<'_>, solved: &petramond_ui::Solved, i: u32) -> i32 {
                let r = solved.rects[i as usize];
                tree.get(i)
                    .children
                    .iter()
                    .filter(|&&c| solved.rects[c as usize].h > 0)
                    .map(|&c| bottom(tree, solved, c))
                    .fold(r.y + r.h, i32::max)
            }
            for &c in &tree.get(list).children {
                let r = solved.rects[c as usize];
                if r.h > 0 {
                    assert_eq!(
                        bottom(&tree, &solved, c),
                        r.y + r.h,
                        "{viewport:?} x{gui_scale}: a row's content spills out of {r:?}"
                    );
                }
            }
            let rows: Vec<_> = tree
                .get(list)
                .children
                .iter()
                .map(|&c| solved.rects[c as usize])
                .filter(|r| r.h > 0)
                .collect();
            assert!(!rows.is_empty(), "{tab:?} {viewport:?}: {rows:?}");
            for pair in rows.windows(2) {
                assert!(
                    pair[1].y >= pair[0].y + pair[0].h,
                    "{viewport:?}: {:?} overlaps {:?}",
                    pair[1],
                    pair[0]
                );
            }
        }
    }
}
