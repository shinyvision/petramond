//! Options → Graphics controller: the view-distance slider (4..=48 chunks,
//! applied live on release — replica, streaming request, and fog together)
//! plus particles, anti-aliasing and screen shake. Both sliders preview their
//! readout while dragged and apply on release.

use crate::app::App;
use petramond::save::client::{AntiAliasing, ParticlesMode};
use petramond_ui::{UiEvent, UiState, UiValue};

/// The view-distance slider's range in chunks.
const VIEW_DISTANCE_RANGE: std::ops::RangeInclusive<i32> = 4..=48;

/// The readout for an anti-aliasing mode.
pub(super) fn anti_aliasing_label(mode: AntiAliasing) -> &'static str {
    match mode {
        AntiAliasing::Off => "Off",
        AntiAliasing::Msaa4x => "MSAA 4x",
        AntiAliasing::Msaa8x => "MSAA 8x",
        AntiAliasing::Ssaa4x => "SSAA 4x",
        AntiAliasing::Ssaa16x => "SSAA 16x",
    }
}

fn particles_label(mode: ParticlesMode) -> &'static str {
    match mode {
        ParticlesMode::Off => "Off",
        ParticlesMode::Reduced => "Reduced",
        ParticlesMode::Full => "Full",
    }
}

pub(super) fn populate(app: &App, state: &mut UiState) {
    super::populate_options_chrome(app, state);
    let vd = app
        .view_distance_preview
        .unwrap_or(app.settings.render_dist);
    state.set("view_distance", UiValue::F32(vd as f32));
    state.set("vd_label", UiValue::Str(format!("{vd} chunks")));
    state.set(
        "particles_label",
        UiValue::Str(format!(
            "Particles: {}",
            particles_label(app.settings.particles)
        )),
    );
    state.set("screen_shake", UiValue::Bool(app.settings.screen_shake));
    let aa = app
        .anti_aliasing_preview
        .unwrap_or(app.settings.anti_aliasing);
    state.set("anti_aliasing", UiValue::F32(aa.index() as f32));
    state.set("aa_label", UiValue::Str(anti_aliasing_label(aa).into()));
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    if super::options_category_back(app, &ev) {
        return;
    }
    match ev {
        UiEvent::SliderChange {
            id,
            value,
            committed,
            ..
        } if id == "anti_aliasing" => {
            let mode = AntiAliasing::from_index(value.round().max(0.0) as usize);
            if committed {
                app.apply_anti_aliasing(mode);
                app.persist_settings();
            } else {
                app.anti_aliasing_preview = Some(mode);
            }
        }
        UiEvent::Toggle { id, .. } if id == "screen_shake" => {
            app.apply_screen_shake(!app.settings.screen_shake);
            app.persist_settings();
        }
        UiEvent::Click { id, .. } if id == "particles" => {
            let next = app.settings.particles.next();
            app.settings.particles = next;
            app.apply_particles();
            app.persist_settings();
        }
        UiEvent::SliderChange {
            id,
            value,
            committed,
            ..
        } if id == "view_distance" => {
            let chunks = (value.round() as i32)
                .clamp(*VIEW_DISTANCE_RANGE.start(), *VIEW_DISTANCE_RANGE.end());
            if committed {
                app.apply_view_distance(chunks);
                app.persist_settings();
            } else {
                app.view_distance_preview = Some(chunks);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
