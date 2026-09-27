use super::{ScreenCtx, ShellCommand};
use petramond::save::client::{AntiAliasing, ParticlesMode};
use petramond_ui::{UiEvent, UiState, UiValue};

const VIEW_DISTANCE_RANGE: std::ops::RangeInclusive<i32> = 4..=48;

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

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    super::populate_options_chrome(ctx, state);
    let options = &*ctx.options;
    let vd = options
        .view_distance_preview
        .unwrap_or(options.settings.render_dist);
    state.set("view_distance", UiValue::F32(vd as f32));
    state.set("vd_label", UiValue::Str(format!("{vd} chunks")));
    state.set(
        "particles_label",
        UiValue::Str(format!(
            "Particles: {}",
            particles_label(options.settings.particles)
        )),
    );
    state.set("screen_shake", UiValue::Bool(options.settings.screen_shake));
    let aa = options
        .anti_aliasing_preview
        .unwrap_or(options.settings.anti_aliasing);
    state.set("anti_aliasing", UiValue::F32(aa.index() as f32));
    state.set("aa_label", UiValue::Str(anti_aliasing_label(aa).into()));
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    if super::options_category_back(ctx, &ev) {
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
                ctx.options.set_anti_aliasing(mode);
                ctx.options.persist();
            } else {
                ctx.options.anti_aliasing_preview = Some(mode);
            }
        }
        UiEvent::Toggle { id, .. } if id == "screen_shake" => {
            let on = !ctx.options.settings.screen_shake;
            ctx.options.set_screen_shake(on);
            ctx.options.persist();
        }
        UiEvent::Click { id, .. } if id == "particles" => {
            let next = ctx.options.settings.particles.next();
            ctx.options.settings.particles = next;
            ctx.request(ShellCommand::ApplyParticles);
            ctx.options.persist();
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
                ctx.request(ShellCommand::ApplyViewDistance(chunks));
            } else {
                ctx.options.view_distance_preview = Some(chunks);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
