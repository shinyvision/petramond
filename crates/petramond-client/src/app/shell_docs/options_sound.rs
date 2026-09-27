use super::{ScreenCtx, ShellCommand};
use petramond_ui::{UiEvent, UiState, UiValue};

fn bind_volume(state: &mut UiState, key: &str, pct_key: &str, value: f32) {
    let pct = (value * 100.0).round();
    state.set(key.to_string(), UiValue::F32(pct));
    state.set(pct_key.to_string(), UiValue::Str(format!("{pct:.0}%")));
}

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    super::populate_options_chrome(ctx, state);
    let settings = &ctx.options.settings;
    bind_volume(state, "master_vol", "master_pct", settings.master_volume);
    bind_volume(state, "sound_vol", "sound_pct", settings.sound_volume);
    bind_volume(state, "music_vol", "music_pct", settings.music_volume);
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    if super::options_category_back(ctx, &ev) {
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
        let settings = &mut ctx.options.settings;
        match id.as_str() {
            "master_vol" => settings.master_volume = volume,
            "sound_vol" => settings.sound_volume = volume,
            "music_vol" => settings.music_volume = volume,
            _ => return,
        }
        ctx.request(ShellCommand::ApplyVolumes);
        if committed {
            ctx.options.persist();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
