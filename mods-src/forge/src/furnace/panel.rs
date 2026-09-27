use super::*;

impl ForgingFurnaceSpec {
    pub(super) fn publish_gauges(
        &self,
        ctx: &StepCtx<'_>,
        state: &State,
        c: &Casting,
        pourable: bool,
        melting: Option<&str>,
    ) {
        ctx.publish(keys::BURN01, GuiValue::F32(state.fire.gauge01()));
        let melt = c.metal(melting.unwrap_or(&state.metal)).melt_ticks.max(1);
        ctx.publish(
            keys::MELT01,
            GuiValue::F32((state.melt_progress as f32 / melt as f32).clamp(0.0, 1.0)),
        );
        ctx.publish(
            keys::CRUCIBLE01,
            GuiValue::F32(state.units as f32 / CRUCIBLE_MAX as f32),
        );
        let metal = c.metal(&state.metal);
        let rgb = if state.hardened() {
            metal.solid
        } else {
            metal.molten
        };
        let packed = ((rgb[0] as i32) << 16) | ((rgb[1] as i32) << 8) | rgb[2] as i32;
        ctx.publish(keys::CRUCIBLE_COLOR, GuiValue::I32(packed));
        let tip = if state.units > 0 {
            metal.name.clone().unwrap_or_default()
        } else {
            String::new()
        };
        ctx.publish(keys::CRUCIBLE_TIP, GuiValue::Str(tip));
        let pouring = state.phase != Phase::Idle;
        ctx.publish(keys::LEVER_FRAME, GuiValue::I32(lever_frame(state) as i32));
        ctx.publish(
            keys::LEVER_ENABLED,
            GuiValue::I32((pourable || pouring) as i32),
        );
    }
}
