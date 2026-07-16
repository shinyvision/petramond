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
