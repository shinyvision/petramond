use super::*;

#[test]
fn graphics_readouts_fit_their_columns() {
    use petramond_ui::{solve, InstTree, ThemeEnv};
    let doc =
        petramond::gui::documents::doc_for(petramond_world::gui_state::GuiKind::OptionsGraphics)
            .expect("graphics document loads");
    let theme = petramond::gui::doc_theme::theme();
    for mode in AntiAliasing::ALL {
        let mut state = UiState::new();
        state.set("vd_label", UiValue::Str("48 chunks".into()));
        state.set("aa_label", UiValue::Str(anti_aliasing_label(mode).into()));
        let tree = InstTree::expand(&doc.doc, &state);
        for gui_scale in 1..=4 {
            let env = ThemeEnv {
                theme: &theme,
                gui_scale,
                image_size: &|_| None,
            };
            let solved = solve(&tree, &env, (320, 240), &|_| 0);
            for key in ["vd_label", "aa_label"] {
                let i = (0..tree.len() as u32)
                    .find(|i| tree.get(*i).node.bind.text.as_deref() == Some(key))
                    .expect("readout in document");
                let text = tree.get(i).text.as_deref().unwrap_or("");
                assert!(
                    theme.ui_font().width(text) <= solved.rects[i as usize].w,
                    "{text:?} overflows its column at GUI scale {gui_scale}"
                );
            }
        }
    }
}
