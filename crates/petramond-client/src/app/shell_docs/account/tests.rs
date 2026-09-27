use super::*;
use petramond_ui::{solve, InstTree, ThemeEnv};

/// Both account lines are engine copy in a FIXED, non-wrapping box, and the
/// shipped-document guard seeds its own short sample strings — so nothing else
/// measures what a player actually reads. Overrunning the box does not wrap, it
/// ellipsizes mid-sentence: "Sign in to play on servers that requ..." is how
/// this screen shipped its first draft.
#[test]
fn the_account_lines_fit_their_inset() {
    let doc = petramond::gui::documents::doc_for(petramond_world::gui_state::GuiKind::Account)
        .expect("account document loads");
    let theme = petramond::gui::doc_theme::theme();
    // The longest name the website can issue (its username rule is 3..=24).
    let widest = SavedSignIn {
        username: "w".repeat(24),
        ..SavedSignIn::default()
    };
    for signed_in in [None, Some(&widest)] {
        let mut state = UiState::new();
        state.set("account_name", UiValue::Str(name_line(signed_in)));
        state.set("account_detail", UiValue::Str(detail_line(signed_in)));
        state.set("is_signed_in", UiValue::Bool(signed_in.is_some()));
        state.set("signed_out", UiValue::Bool(signed_in.is_none()));
        let tree = InstTree::expand(&doc.doc, &state);
        let env = ThemeEnv {
            theme: &theme,
            gui_scale: 3,
            image_size: &|_| None,
        };
        // The smallest viewport the game scales to, as the document guard uses.
        let solved = solve(&tree, &env, (320, 240), &|_| 0);
        for key in ["account_name", "account_detail"] {
            let i = (0..tree.len() as u32)
                .find(|i| tree.get(*i).node.bind.text.as_deref() == Some(key))
                .unwrap_or_else(|| panic!("{key} is in the document"));
            let text = tree.get(i).text.as_deref().unwrap_or("");
            let ink = theme.ui_font().width(text);
            let box_w = solved.rects[i as usize].w;
            assert!(
                ink <= box_w,
                "{key} {text:?} needs {ink}px, its box is {box_w}px — it will ellipsize"
            );
        }
    }
}

/// The same blind spot one screen over. The shipped-document guard proves every
/// screen FITS, but it seeds its own sample strings — so a panel that only
/// overflows once the controller's real (longer) copy wraps to a second line
/// passes it. Both account-flow screens carry engine copy the guard never sees,
/// so solve them with the real thing.
///
/// Each row is checked against ITS PARENT's content box, never the viewport:
/// the root frame is `grow`, so it IS the viewport by construction and an
/// assertion against it can never fail. The first draft of this test made
/// exactly that mistake and sat green while four extra rows ran off the panel.
#[test]
fn the_account_flow_panels_fit_the_smallest_viewport_with_their_real_copy() {
    use petramond_ui::LayoutEnv;
    let theme = petramond::gui::doc_theme::theme();
    let viewport = (320, 240);
    let mut overflowing = Vec::new();
    for (kind, state) in [
        (
            petramond_world::gui_state::GuiKind::Account,
            real_account_state(),
        ),
        (
            petramond_world::gui_state::GuiKind::ConnectServer,
            real_connect_state(),
        ),
        (
            petramond_world::gui_state::GuiKind::AccountSignIn,
            real_sign_in_state(),
        ),
    ] {
        let doc = petramond::gui::documents::doc_for(kind).expect("document loads");
        // Every gui_scale bottoms out at the same logical box, so one scale
        // solves the layout the others do.
        let tree = InstTree::expand(&doc.doc, &state);
        let env = ThemeEnv {
            theme: &theme,
            gui_scale: 3,
            image_size: &|_| None,
        };
        let solved = solve(&tree, &env, viewport, &|_| 0);
        // Overflowing a `scroll` is what it is for.
        let in_scroll = |mut i: u32| {
            while let Some(p) = tree.get(i).parent {
                if matches!(tree.get(p).node.kind, petramond_ui::NodeKind::Scroll { .. }) {
                    return true;
                }
                i = p;
            }
            false
        };
        for i in 0..tree.len() {
            let inst = tree.get(i as u32);
            let Some(p) = inst.parent else { continue };
            // `abs` children are deliberately out of flow (the backdrop image).
            if in_scroll(i as u32) || inst.layout.abs.is_some() || solved.rects[i].h == 0 {
                continue;
            }
            let parent = tree.get(p);
            let pad = parent.layout.pad;
            let border = env.container_insets(parent.node);
            let box_ = solved.rects[p as usize].inset(std::array::from_fn(|k| pad[k] + border[k]));
            let rect = solved.rects[i];
            if rect.y < box_.y || rect.y + rect.h > box_.y + box_.h {
                overflowing.push(format!(
                    "{kind:?}: {:?} at y {}..{} outside its parent's {}..{}",
                    inst.node.kind,
                    rect.y,
                    rect.y + rect.h,
                    box_.y,
                    box_.y + box_.h,
                ));
            }
        }
    }
    assert!(
        overflowing.is_empty(),
        "the real copy does not fit — the bottom rows are what fall off:\n{}",
        overflowing.join("\n"),
    );
}

/// The sign-in form mid-request, with a refusal showing — every row the screen
/// can put up at once.
fn real_sign_in_state() -> UiState {
    let mut state = UiState::new();
    state.set("account_id", UiValue::Str("explorer".to_owned()));
    state.set("account_password", UiValue::Str("hunter2".to_owned()));
    state.set(
        "service_line",
        UiValue::Str(crate::app::shell_docs::account_sign_in::service_line()),
    );
    state.set("working", UiValue::Bool(true));
    state.set("working_text", UiValue::Str("Signing in…".to_owned()));
    state.set("has_status", UiValue::Bool(true));
    state.set(
        "status_text",
        UiValue::Str("That username or password is not correct".to_owned()),
    );
    state.set("can_submit", UiValue::Bool(false));
    state
}

/// The Account screen as a signed-OUT player sees it, with every transient row
/// showing at once (the worst case the screen can reach).
fn real_account_state() -> UiState {
    let mut state = UiState::new();
    state.set("account_name", UiValue::Str(name_line(None)));
    state.set("account_detail", UiValue::Str(detail_line(None)));
    state.set("signed_out", UiValue::Bool(true));
    state.set("is_signed_in", UiValue::Bool(false));
    state.set("working", UiValue::Bool(true));
    state.set(
        "working_text",
        UiValue::Str("Checking your sign-in…".to_owned()),
    );
    state.set("has_status", UiValue::Bool(true));
    state.set(
        "status_text",
        UiValue::Str("Your Petramond sign-in has expired — sign in again".to_owned()),
    );
    state
}

/// The Connect screen mid-attempt, signed out — the longest identity line the
/// controller can bind, plus the progress row and an inline failure.
fn real_connect_state() -> UiState {
    let mut state = UiState::new();
    state.set("server_addr", UiValue::Str("play.petramond.com".to_owned()));
    state.set("signed_out", UiValue::Bool(true));
    state.set("is_signed_in", UiValue::Bool(false));
    state.set(
        "signed_in_as",
        UiValue::Str(crate::app::shell_docs::connect_server::identity_line(None)),
    );
    state.set("connecting", UiValue::Bool(true));
    state.set("connect_phase", UiValue::Str("Joining world…".to_owned()));
    state.set("has_status", UiValue::Bool(true));
    state.set(
        "status_text",
        UiValue::Str("This server requires a Petramond account".to_owned()),
    );
    state.set("can_connect", UiValue::Bool(false));
    state
}
