//! The forging furnace: a machine you OPERATE, not a container you fill.
//!
//! Coal heats it and raw metal melts into its crucible. You drop a mould into
//! its own slot, pull the lever, and watch the metal run down the groove and
//! set in the shape you gave it. Every control is in the panel; the block
//! itself is the thing you WATCH.
//!
//! WHAT DECIDES THE CAST IS DATA. The mould's row names a recipe CLASS and the
//! crucible remembers which metal it holds, so `(class, metal)` in the ordinary
//! machine-recipe table IS the result. No metal, mould or product is named
//! here. An empty basin casts `forge:cast_plate` — which is also what you get
//! for pulling the mould out mid-pour, because the class is read when the metal
//! finishes running, not when it starts. Automatic pours retain their mould
//! choice so removing it cannot accidentally produce plates.
//!
//! WHAT THE PLAYER SEES IS ONE MODEL. The fire in the hood is a PER-INSTANCE
//! PART of a single `.bbmodel`; everything with a level or a
//! shape — the metal, the mould, the cast — is simulated and DRAWN. Enumerating
//! all of it as block rows would be dozens; the pack ships three, and they
//! differ only in what the block IS — cold, lit, or pouring — because each of
//! those has its own emission and its own particle emitter.

use machine_core::{
    consume_one, write_changed_slots, Caches, Machine, MachineSpec, Presentation, StepCtx,
};
use mod_sdk::*;

use crate::content::Casting;
use crate::keys;
use crate::liquid::{Basin, Liquid, EJECT_AT};

pub mod fittings;
mod panel;
mod state;
mod storage;

use state::{Phase, State};

const STATE_KEY: &str = "forge:state";

const MELT_TILE: &str = "calcite";

const SLOT_METAL: usize = 0;
const SLOT_FUEL: usize = 1;
const SLOT_MOULD: usize = 2;
const SLOTS: usize = 3;

use crate::keys::{SOUND_CAST, SOUND_FIRE, SOUND_LAND, SOUND_LEVER};

fn at(pos: [i32; 3]) -> [f64; 3] {
    [
        pos[0] as f64 + 0.5,
        pos[1] as f64 + 0.5,
        pos[2] as f64 + 0.5,
    ]
}

pub const CRUCIBLE_MAX: u8 = 8;
const POUR_TICKS: u32 = 80;
const SET_TICKS: u32 = 120;

const LEVER_FRAME_TICKS: u32 = 12;
const LEVER_LAST_FRAME: u32 = 7;

fn lever_frame(state: &State) -> u32 {
    match state.phase {
        Phase::Idle => 0,
        Phase::Pouring => {
            (state.phase_ticks * LEVER_LAST_FRAME / LEVER_FRAME_TICKS).min(LEVER_LAST_FRAME)
        }
        Phase::Setting => LEVER_LAST_FRAME,
    }
}

fn pouring_visually(state: &State) -> bool {
    state.phase == Phase::Pouring || state.liquid.head > state.liquid.tail
}

const PARTS: [&str; 1] = ["coals"];
const PART_COALS: u32 = 1 << 0;

const ROW_LIT: usize = 0;
const ROW_POUR: usize = 1;

pub type ForgingFurnace = Machine<ForgingFurnaceSpec>;

#[derive(Default)]
pub struct ForgingFurnaceSpec {
    casting: Option<Casting>,
    pub fittings: fittings::Fittings,
    storage: storage::Storage,
}

impl MachineSpec for ForgingFurnaceSpec {
    const KIND_KEY: &'static str = keys::FURNACE_GUI;
    const BLOCK_KEY: &'static str = keys::FORGING_FURNACE;
    const VARIANT_KEYS: &'static [&'static str] =
        &[keys::FORGING_FURNACE_LIT, keys::FORGING_FURNACE_POUR];
    const ANCHORS_KEY: &'static str = "forge:furnaces";
    const STATE_KEY: &'static str = STATE_KEY;
    const AUX_KEYS: &'static [&'static str] = &[fittings::KEY];
    const PANEL_KEYS: &'static [&'static str] = &[keys::FITTINGS_GUI];

    fn init(&mut self) {
        self.casting = Some(Casting::resolve());
        self.fittings = fittings::Fittings::resolve();
        self.storage = storage::Storage::resolve();
    }

    fn forget(&mut self, pos: [i32; 3]) {
        let state = State::load(&section_kv_get(pos, STATE_KEY).unwrap_or_default());
        if state.units > 0 && !state.metal.is_empty() {
            spawn_item(&state.metal, state.units, at(pos));
        }
        section_kv_delete(pos, STATE_KEY);
        section_kv_delete(pos, fittings::KEY);
        section_kv_delete(pos, fittings::INFO_KEY);
    }

    fn step(
        &mut self,
        ctx: &StepCtx<'_>,
        caches: &mut Caches,
        slots: Option<Vec<Option<ItemStackData>>>,
        stored: &mut Vec<u8>,
        out: &mut Presentation,
    ) {
        let Some(casting) = &self.casting else {
            return;
        };
        let mut slots = slots.unwrap_or_default();
        slots.resize(SLOTS, None);
        let mut state = State::load(stored);
        let before = state.clone();
        let before_slots = slots.clone();
        let bits = fittings::record(&ctx.aux[0]);
        if let Some(stoker) = self.fittings.active(bits, 3) {
            state.feed_ticks = state.feed_ticks.saturating_add(1);
            if state.feed_ticks >= stoker.feed_every {
                state.feed_ticks = 0;
                self.storage.feed(
                    ctx.pos,
                    &mut slots,
                    &state,
                    casting,
                    caches,
                    stoker.feed_keep,
                );
            }
        }
        let mould = slots[SLOT_MOULD].as_ref().map(|s| s.item.clone());

        state.fire.tick();
        if state.fire.lit() {
            state.idle_ticks = 0;
        } else {
            state.idle_ticks = state.idle_ticks.saturating_add(1);
        }

        if self.melt(&mut state, &mut slots, casting, caches) {
            emit_sound(SOUND_FIRE, Some(at(ctx.pos)));
        }
        if let Some(auto) = self.fittings.active(bits, 1) {
            let ready = mould
                .as_deref()
                .is_some_and(|m| casting.mould_class(m).is_some())
                && self.pourable(&state, caches, mould.as_deref());
            if auto_ready(&mut state, ready, mould.as_deref(), auto.dwell_ticks) {
                state.phase = Phase::Pouring;
                state.phase_ticks = 0;
                state.pour_mould = mould.clone().unwrap_or_default();
                emit_sound(SOUND_LEVER, Some(at(ctx.pos)));
            }
        }
        let cast_mould = if state.pour_mould.is_empty() {
            mould.clone()
        } else {
            Some(state.pour_mould.clone())
        };
        self.run_pour(ctx, &mut state, caches, casting, cast_mould.as_deref());
        let tapped = state.phase == Phase::Pouring;
        state
            .liquid
            .step(tapped, 1.0 / (POUR_TICKS as f32 * 0.6).max(1.0));
        if state.liquid.landed() && !before.liquid.landed() {
            emit_sound(SOUND_LAND, Some(at(ctx.pos)));
        }

        write_changed_slots(ctx.pos, &before_slots, &slots);
        if state != before {
            *stored = state.to_bytes();
        }

        let want_row = if pouring_visually(&state) {
            ctx.variant_or_base(ROW_POUR)
        } else if state.fire.lit() {
            ctx.variant_or_base(ROW_LIT)
        } else {
            ctx.block
        };
        if ctx.current != want_row {
            swap_block(ctx.pos, want_row);
        }
        out.parts(ctx.pos, self.parts_mask(&state), None);
        let set_ticks = self
            .fittings
            .active(bits, 0)
            .map_or(SET_TICKS, |r| r.set_ticks);
        let basin = self.basin(&state, casting, caches, cast_mould.as_deref(), set_ticks);
        let rgb = casting.metal(&state.metal).molten;
        out.draw(ctx.pos, state.liquid.prims(MELT_TILE, rgb, &basin));

        if ctx.gui_open() {
            self.fittings.publish(ctx, bits);
            let pourable = self.pourable(&state, caches, mould.as_deref());
            let melting = slots[SLOT_METAL].as_ref().map(|s| s.item.as_str());
            self.publish_gauges(ctx, &state, casting, pourable, melting);
        }
    }
}

fn wants_heat(state: &State, can_melt: bool) -> bool {
    can_melt || state.units > 0
}

fn mould_in(anchor: [i32; 3]) -> Option<String> {
    container_get(anchor.into())?
        .get(SLOT_MOULD)?
        .as_ref()
        .map(|s| s.item.clone())
}

fn cast_class<'a>(c: &'a Casting, mould: Option<&str>) -> &'a str {
    mould
        .and_then(|m| c.mould_class(m))
        .unwrap_or(keys::CAST_PLATE_CLASS)
}

fn cast_result(
    c: &Casting,
    caches: &mut Caches,
    metal: &str,
    mould: Option<&str>,
) -> Option<ItemStackData> {
    caches
        .recipe_for(cast_class(c, mould), metal)
        .or_else(|| caches.recipe_for(keys::CAST_PLATE_CLASS, metal))
}

impl ForgingFurnaceSpec {
    fn melt(
        &self,
        state: &mut State,
        slots: &mut [Option<ItemStackData>],
        casting: &Casting,
        caches: &mut Caches,
    ) -> bool {
        let input = slots[SLOT_METAL]
            .as_ref()
            .filter(|s| s.count > 0)
            .map(|s| s.item.clone());
        let can_melt = input.as_deref().is_some_and(|item| {
            casting.is_metal(item)
                && state.units < CRUCIBLE_MAX
                && (state.metal.is_empty() || state.metal == casting.molten_form(item))
        });
        let wants = wants_heat(state, can_melt);
        let caught = state.fire.relight(wants, &mut slots[SLOT_FUEL], caches);
        if !can_melt || !state.fire.lit() {
            state.melt_progress = 0;
            return caught;
        }
        let item = input.expect("can_melt implies an input");
        state.melt_progress += 1;
        if state.melt_progress >= casting.metal(&item).melt_ticks {
            state.melt_progress = 0;
            state.units += 1;
            state.metal = casting.molten_form(&item);
            consume_one(&mut slots[SLOT_METAL]);
        }
        caught
    }

    fn run_pour(
        &self,
        ctx: &StepCtx<'_>,
        state: &mut State,
        caches: &mut Caches,
        c: &Casting,
        mould: Option<&str>,
    ) {
        match state.phase {
            Phase::Idle => {}
            Phase::Pouring => {
                state.phase_ticks += 1;
                if state.phase_ticks >= POUR_TICKS {
                    state.phase = Phase::Setting;
                    state.phase_ticks = 0;
                }
            }
            Phase::Setting => {
                state.phase_ticks += 1;
                if state.phase_ticks
                    >= self
                        .fittings
                        .active(fittings::record(&ctx.aux[0]), 0)
                        .map_or(SET_TICKS, |r| r.set_ticks)
                {
                    self.eject(ctx, state, caches, c, mould);
                }
            }
        }
    }

    fn eject(
        &self,
        ctx: &StepCtx<'_>,
        state: &mut State,
        caches: &mut Caches,
        c: &Casting,
        mould: Option<&str>,
    ) {
        let result = cast_result(c, caches, &state.metal, mould);
        if let Some(result) = &result {
            let Some(spot) =
                block_local_to_world(ctx.pos, vec![EJECT_AT]).and_then(|p| p.into_iter().next())
            else {
                return;
            };
            let remaining = if self
                .fittings
                .active(fittings::record(&ctx.aux[0]), 2)
                .is_some()
            {
                self.storage.deliver(ctx.pos, result.clone())
            } else {
                Some(result.clone())
            };
            if let Some(left) = remaining {
                let data: Vec<_> = left
                    .data
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_slice()))
                    .collect();
                spawn_item_data(&left.item, left.count, spot, &data);
            }
            emit_sound(SOUND_CAST, Some(spot));
        }
        state.phase = Phase::Idle;
        state.phase_ticks = 0;
        state.pour_mould.clear();
        state.liquid = Liquid::default();
        if result.is_some() {
            state.units = state.units.saturating_sub(1);
            if state.units == 0 {
                state.metal.clear();
            }
        }
    }

    /// The mould and the product the metal is becoming, both drawn as their own items so the cast
    /// is always the real thing. That's also why there's no model cube per mould or product.
    ///
    /// The cast appears as soon as metal lands, drawn at the pour's own fill while it's still
    /// arriving. Only with a mould: metal on the bare crucible stays the square pool (`liquid.rs`).
    fn basin(
        &self,
        state: &State,
        c: &Casting,
        caches: &mut Caches,
        mould: Option<&str>,
        set_ticks: u32,
    ) -> Basin {
        let metal = c.metal(&state.metal);
        let cast = match state.phase {
            Phase::Pouring if mould.is_some() && state.liquid.level > 0.0 => {
                cast_result(c, caches, &state.metal, mould)
                    .map(|r| (r.item, metal.molten, state.liquid.level))
            }
            Phase::Setting => cast_result(c, caches, &state.metal, mould).map(|r| {
                let t = (state.phase_ticks as f32 / set_ticks as f32).clamp(0.0, 1.0);
                (r.item, mix(metal.molten, metal.solid, t), 1.0)
            }),
            _ => None,
        };
        Basin {
            mould: mould.map(str::to_owned),
            cast,
        }
    }

    fn parts_mask(&self, state: &State) -> u32 {
        let mask = if state.fire.lit() { PART_COALS } else { 0 };
        debug_assert_eq!(mask >> PARTS.len(), 0, "a part bit the row never declared");
        mask
    }

    fn pourable(&self, state: &State, caches: &mut Caches, mould: Option<&str>) -> bool {
        let Some(casting) = &self.casting else {
            return false;
        };
        state.ready_to_pour()
            && caches
                .recipe_for(cast_class(casting, mould), &state.metal)
                .is_some()
    }
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8)
}

impl ForgingFurnaceSpec {
    pub fn pull_lever(&self, anchor: [i32; 3], caches: &mut Caches) {
        let stored = section_kv_get(anchor, STATE_KEY).unwrap_or_default();
        let mut state = State::load(&stored);
        let mould = mould_in(anchor);
        if !self.pourable(&state, caches, mould.as_deref()) {
            return;
        }
        state.phase = Phase::Pouring;
        state.phase_ticks = 0;
        section_kv_set(anchor, STATE_KEY, state.to_bytes());

        emit_sound(SOUND_LEVER, Some(at(anchor)));
    }
}

fn auto_ready(state: &mut State, ready: bool, mould: Option<&str>, dwell: u32) -> bool {
    let mould = mould.unwrap_or_default();
    if !ready || mould.is_empty() || state.ready_mould != mould {
        state.ready_ticks = 0;
        state.ready_mould = mould.to_owned();
        return false;
    }
    state.ready_ticks = state.ready_ticks.saturating_add(1);
    if state.ready_ticks < dwell {
        return false;
    }
    state.ready_ticks = 0;
    true
}

#[cfg(test)]
mod tests;
