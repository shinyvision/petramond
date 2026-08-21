//! Options → Sound controller: master / sound / music volume sliders.
//! Slider drags apply LIVE (the mixer re-reads volumes every frame); the
//! release commit persists `client.json`.

use crate::app::App;
use petramond_ui::{UiEvent, UiState, UiValue};

fn bind_volume(state: &mut UiState, key: &str, pct_key: &str, value: f32) {
    let pct = (value * 100.0).round();
    state.set(key.to_string(), UiValue::F32(pct));
    state.set(pct_key.to_string(), UiValue::Str(format!("{pct:.0}%")));
}

pub(super) fn populate(app: &App, state: &mut UiState) {
    super::populate_options_chrome(app, state);
    bind_volume(
        state,
        "master_vol",
        "master_pct",
        app.settings.master_volume,
    );
    bind_volume(state, "sound_vol", "sound_pct", app.settings.sound_volume);
    bind_volume(state, "music_vol", "music_pct", app.settings.music_volume);
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    if super::options_category_back(app, &ev) {
        return;
    }
    if let UiEvent::SliderChange {
        id,
        value,
        committed,
        ..
    } = ev
    {
        let volume = (value / 100.0).clamp(0.0, 1.0);
        match id.as_str() {
            "master_vol" => app.settings.master_volume = volume,
            "sound_vol" => app.settings.sound_volume = volume,
            "music_vol" => app.settings.music_volume = volume,
            _ => return,
        }
        app.apply_volumes();
        if committed {
            app.persist_settings();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The percent readout is engine copy in a fixed column, so no document
    /// guard sees it — and a box too tight even for an ellipsis draws NOTHING,
    /// which is how "100%" once rendered as blank. 100 is the widest value.
    #[test]
    fn the_widest_volume_readout_fits_its_column() {
        use petramond_ui::{solve, InstTree, ThemeEnv};
        let doc =
            petramond::gui::documents::doc_for(petramond_world::gui_state::GuiKind::OptionsSound)
                .expect("sound document loads");
        let theme = petramond::gui::doc_theme::theme();
        let mut state = UiState::new();
        bind_volume(&mut state, "master_vol", "master_pct", 1.0);
        let tree = InstTree::expand(&doc.doc, &state);
        let env = ThemeEnv {
            theme: &theme,
            gui_scale: 3,
            image_size: &|_| None,
        };
        let solved = solve(&tree, &env, (320, 240), &|_| 0);
        let i = (0..tree.len() as u32)
            .find(|i| tree.get(*i).node.bind.text.as_deref() == Some("master_pct"))
            .expect("the readout is in the document");
        let text = tree.get(i).text.as_deref().unwrap_or("");
        let ink = theme.ui_font().width(text);
        assert!(
            ink <= solved.rects[i as usize].w,
            "{text:?} needs {ink}px, column is {}px",
            solved.rects[i as usize].w
        );
    }
}
