use super::*;

/// The hint box is deliberately fixed-height (a reflow between a queued
/// press and its resolution misroutes the click), so it is the copy that
/// has to fit — at every gui scale, including 1, where a `small` run has
/// no smaller step to take. Longer wording needs a taller box, and only
/// this file knows what the wording is.
#[test]
fn both_remap_hints_fit_the_fixed_hint_box() {
    use petramond_ui::{solve, InstTree, ThemeEnv, UiState};
    let doc =
        petramond::gui::documents::doc_for(petramond_world::gui_state::GuiKind::OptionsControls)
            .expect("controls document loads");
    let theme = petramond::gui::doc_theme::theme();
    let mut state = UiState::new();
    state.set("remap_hint", UiValue::Str(HINT_ARMED.into()));
    let tree = InstTree::expand(&doc.doc, &state);
    for scale in [1, 2, 3, 4] {
        let env = ThemeEnv {
            theme: &theme,
            gui_scale: scale,
            image_size: &|_| None,
        };
        let solved = solve(&tree, &env, (320, 240), &|_| 0);
        let hint = (0..tree.len() as u32)
            .find(|i| tree.get(*i).node.bind.text.as_deref() == Some("remap_hint"))
            .map(|i| solved.rects[i as usize])
            .expect("the hint label is in the document");
        for text in [HINT_ARMED, HINT_IDLE] {
            let (_, h) = theme.ui_font().measure(text, Some(hint.w));
            assert!(
                h <= hint.h,
                "@scale {scale}: {text:?} needs {h}px in a {}×{} box",
                hint.w,
                hint.h
            );
        }
    }
}

/// Every ENGINE action label is authored copy that reaches the row through
/// a bind — so the shrink rule treats it as data and ellipsizes it, and no
/// document guard can see it. The binding button's column is fixed, so the
/// label column is whatever is left; a label that outgrows it reads as
/// "Previous Ho...". Widen the panel or shorten the wording.
#[test]
fn every_engine_action_label_fits_the_controls_row() {
    use petramond_ui::{solve, InstTree, ThemeEnv, UiState};
    use petramond_world::controls::BindableAction;
    let doc =
        petramond::gui::documents::doc_for(petramond_world::gui_state::GuiKind::OptionsControls)
            .expect("controls document loads");
    let theme = petramond::gui::doc_theme::theme();
    let mut state = UiState::new();
    let rows: Vec<UiMap> = BindableAction::ALL
        .iter()
        .map(|a| {
            let mut m = UiMap::new();
            m.insert("label".into(), UiValue::Str(a.label().into()));
            m.insert("binding".into(), UiValue::Str("SCROLL DOWN".into()));
            m.insert("is_header".into(), UiValue::Bool(false));
            m.insert("is_action".into(), UiValue::Bool(true));
            m
        })
        .collect();
    state.set("rows", UiValue::List(Arc::new(rows)));
    let tree = InstTree::expand(&doc.doc, &state);
    let env = ThemeEnv {
        theme: &theme,
        gui_scale: 3,
        image_size: &|_| None,
    };
    let solved = solve(&tree, &env, (320, 240), &|_| 0);
    let mut clipped = Vec::new();
    for i in 0..tree.len() as u32 {
        let inst = tree.get(i);
        if inst.node.bind.text.as_deref() != Some("label") {
            continue;
        }
        let text = inst.text.as_deref().unwrap_or("");
        let ink = theme.ui_font().width(text);
        let box_w = solved.rects[i as usize].w;
        if ink > box_w {
            clipped.push(format!("{text:?} needs {ink}px, column is {box_w}px"));
        }
    }
    assert!(clipped.is_empty(), "clipped action labels: {clipped:#?}");
}
