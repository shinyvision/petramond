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

#[cfg(test)]
mod tests {
    use super::*;

    /// Same class as the volume readout: engine copy in a fixed column that no
    /// document guard can see. The widest is the view-distance ceiling.
    #[test]
    fn the_widest_view_distance_readout_fits_its_column() {
        use petramond_ui::{solve, InstTree, ThemeEnv};
        let doc = petramond::gui::documents::doc_for(petramond_world::gui_state::GuiKind::OptionsGraphics)
            .expect("graphics document loads");
        let theme = petramond::gui::doc_theme::theme();
        let mut state = UiState::new();
        state.set("vd_label", UiValue::Str(format!("{} chunks", 48)));
        let tree = InstTree::expand(&doc.doc, &state);
        let env = ThemeEnv {
            theme: &theme,
            gui_scale: 3,
            image_size: &|_| None,
        };
        let solved = solve(&tree, &env, (320, 240), &|_| 0);
        let i = (0..tree.len() as u32)
            .find(|i| tree.get(*i).node.bind.text.as_deref() == Some("vd_label"))
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
