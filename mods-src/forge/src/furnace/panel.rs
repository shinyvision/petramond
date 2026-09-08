use super::*;

impl ForgingFurnaceSpec {
    /// `melting` is the item in the input slot — the one whose melt time the
    /// progress is counted against. Published to the sessions watching THIS
    /// furnace: two people at two forges read their own metal.
    pub(super) fn publish_gauges(
        &self,
        ctx: &StepCtx<'_>,
        state: &State,
        c: &Casting,
        pourable: bool,
        melting: Option<&str>,
    ) {
        let burn01 = if state.burn_max == 0 {
            0.0
        } else {
            state.burn_remaining as f32 / state.burn_max as f32
        };
        ctx.publish("forge:burn01", GuiValue::F32(burn01));
        let melt = c.metal(melting.unwrap_or(&state.metal)).melt_ticks.max(1);
        ctx.publish(
            "forge:melt01",
            GuiValue::F32((state.melt_progress as f32 / melt as f32).clamp(0.0, 1.0)),
        );
        ctx.publish(
            "forge:crucible01",
            GuiValue::F32(state.units as f32 / CRUCIBLE_MAX as f32),
        );
        // Hardened metal is the same hue gone dull — the crucible tells you it
        // has set by its COLOUR, with no extra widget.
        let metal = c.metal(&state.metal);
        let rgb = if state.hardened() {
            metal.solid
        } else {
            metal.molten
        };
        let packed = ((rgb[0] as i32) << 16) | ((rgb[1] as i32) << 8) | rgb[2] as i32;
        ctx.publish("forge:crucible_color", GuiValue::I32(packed));
        // The crucible NAMES its metal on hover: one value is both the tip's
        // text and its condition (a non-empty string is `true` for bindings),
        // so an empty or unnamed crucible simply has no tooltip.
        let tip = if state.units > 0 {
            metal.name.clone().unwrap_or_default()
        } else {
            String::new()
        };
        ctx.publish("forge:crucible_tip", GuiValue::Str(tip));
        // The lever DRAWS from the machine, not from its own latch: the frame
        // is a function of the phase and its tick count, so the handle swings
        // down as the pour starts and snaps back up the instant the machine
        // returns to idle. A control that reports the state it caused cannot
        // show a pour that is not happening.
        let pouring = state.phase != Phase::Idle;
        ctx.publish("forge:pouring", GuiValue::I32(pouring as i32));
        ctx.publish(
            "forge:lever_frame",
            GuiValue::I32(lever_frame(state) as i32),
        );
        // A running pour is not STARTABLE, but the control must stay live or the
        // button paints its disabled face over the frame it should show.
        ctx.publish(
            "forge:lever_enabled",
            GuiValue::I32((pourable || pouring) as i32),
        );
        ctx.publish("forge:can_pour", GuiValue::I32(pourable as i32));
    }
}
