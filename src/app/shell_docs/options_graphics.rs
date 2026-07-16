//! Options → Graphics controller: the view-distance slider (4..=48 chunks,
//! applied live on release — replica, streaming request, and fog together)
//! and the particles cycle button (Full → Reduced → Off).

use crate::app::App;
use petramond_ui::{UiEvent, UiState, UiValue};

pub(super) fn populate(app: &App, state: &mut UiState) {
    super::populate_options_chrome(app, state);
    let vd = app.settings.render_dist;
    state.set("view_distance", UiValue::F32(vd as f32));
    state.set("vd_label", UiValue::Str(format!("{vd} chunks")));
    state.set(
        "particles_label",
        UiValue::Str(format!("Particles: {}", app.settings.particles.label())),
    );
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    if super::options_category_back(app, &ev) {
        return;
    }
    match ev {
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
            let chunks = (value.round() as i32).clamp(4, 48);
            // Drags only preview the label; the release applies the new
            // radius (streaming/meshing re-shape once, not per drag step).
            app.settings.render_dist = chunks;
            if committed {
                app.apply_view_distance(chunks);
                app.persist_settings();
            }
        }
        _ => {}
    }
}
