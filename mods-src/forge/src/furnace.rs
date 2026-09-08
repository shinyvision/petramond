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
use crate::liquid::{Basin, Liquid, EJECT_AT};

pub mod fittings;
mod panel;
mod state;
mod storage;

use state::{Phase, State};

const STATE_KEY: &str = "forge:state";

/// The class cast when the basin holds no mould: metal poured into bare stone
/// sets as a plate.
const CLASS_PLATE: &str = "forge:cast_plate";

/// The tile the poured metal is drawn with. It must be NEAR WHITE: the tint is
/// a straight multiply, so a mid-grey stone face comes out mustard and the
/// molten metal ends up duller than the coals three pixels away. Liquid metal
/// leaving a crucible is the brightest thing on the machine.
const MELT_TILE: &str = "calcite";

const SLOT_METAL: usize = 0;
const SLOT_FUEL: usize = 1;
/// The mould waiting in the basin. It is a SLOT rather than a held-item
/// gesture, and the document's `accepts` is what keeps everything else out —
/// the filter is content, not a branch in here. It names the `forge:mould`
/// DATA key, which is the same statement `Casting` enumerates, so a pack
/// shipping a mould joins both with one row and no tag to remember.
const SLOT_MOULD: usize = 2;
const SLOTS: usize = 3;

/// The lever's widget id in `forging_furnace.gui.json`. Clicking it is the
/// whole pour control; its DRAWN frame is `forge:lever_frame`, published from
/// the machine state rather than its own latch, so the graphic can only ever
/// show what the machine is actually doing.
pub const WIDGET_LEVER: &str = "lever";

/// The four moments the machine is AUDIBLE, and they are the four moments the
/// player is waiting for: the fire catching, the lever thrown, the metal
/// arriving in the basin, and the cast breaking free. Each is a TRANSITION —
/// a machine that hums every tick is a machine you stop hearing, and every one
/// of these costs a host call.
///
/// The rows are `sounds.json` keys, so what they actually play is content: the
/// shipped rows point at engine clips at their own base pitch, and dropping a
/// clip of this pack's own beside them is a one-line row change with nothing
/// to alter here.
const SOUND_FIRE: &str = "forge:fire_catch";
const SOUND_LEVER: &str = "forge:lever";
const SOUND_LAND: &str = "forge:pour_land";
const SOUND_CAST: &str = "forge:cast_free";

/// The anchor cell's centre. Every sound this machine makes is positional and
/// none of them needs to be more precise than the block: the attenuation
/// distances in `sounds.json` are all an order of magnitude wider than the
/// footprint.
fn at(pos: [i32; 3]) -> [f32; 3] {
    [
        pos[0] as f32 + 0.5,
        pos[1] as f32 + 0.5,
        pos[2] as f32 + 0.5,
    ]
}

/// Crucible capacity, in melts.
pub const CRUCIBLE_MAX: u8 = 8;
/// The pour, and the set that follows it.
const POUR_TICKS: u32 = 80;
const SET_TICKS: u32 = 120;

/// The lever's sprite sheet is 8 frames laid over 12 game ticks (the strip is
/// 8x70 ms ≈ 560 ms at authoring time); after that it just stays down.
const LEVER_FRAME_TICKS: u32 = 12;
const LEVER_LAST_FRAME: u32 = 7;

/// Which frame of the lever strip the panel should draw, from the machine
/// state alone. Idle snaps back to the top; an early pour walks 0..7 over
/// [`LEVER_FRAME_TICKS`]; the rest of the pour and the whole set hold the
/// lever down.
fn lever_frame(state: &State) -> u32 {
    match state.phase {
        Phase::Idle => 0,
        Phase::Pouring => {
            (state.phase_ticks * LEVER_LAST_FRAME / LEVER_FRAME_TICKS).min(LEVER_LAST_FRAME)
        }
        Phase::Setting => LEVER_LAST_FRAME,
    }
}

/// Whether the pour is still VISIBLE: the tap is open, or metal is still in
/// the air. The pour row's light and droplet stream hang off this — keyed on
/// the phase instead, they run through the whole set, long after the drawn
/// stream has subsided.
fn pouring_visually(state: &State) -> bool {
    state.phase == Phase::Pouring || state.liquid.head > state.liquid.tail
}

const PARTS: [&str; 1] = ["coals"];
const PART_COALS: u32 = 1 << 0;

/// [`MachineSpec::VARIANT_KEYS`] indices.
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
    const KIND_KEY: &'static str = "forge:forging_furnace";
    const BLOCK_KEY: &'static str = "forge:forging_furnace";
    /// Two variants, and both earn their row: a lit furnace has its own
    /// emission and hearth embers, and a POURING one has its own light and a
    /// particle stream of falling droplets. Everything else this furnace shows
    /// is a PART of the same model.
    const VARIANT_KEYS: &'static [&'static str] =
        &["forge:forging_furnace_lit", "forge:forging_furnace_pour"];
    const ANCHORS_KEY: &'static str = "forge:furnaces";
    const STATE_KEY: &'static str = STATE_KEY;
    const AUX_KEYS: &'static [&'static str] = &[fittings::KEY];
    const PANEL_KEYS: &'static [&'static str] = &[fittings::PAGE];

    fn init(&mut self) {
        self.casting = Some(Casting::resolve());
        self.fittings = fittings::Fittings::resolve();
        self.storage = storage::Storage::resolve();
    }

    /// The furnace is gone. Spill what was in the crucible and drop the blob.
    ///
    /// Both halves are one rule each:
    ///
    /// - **The crucible SPILLS.** The engine scatters a broken container's
    ///   slots, so the coal and the mould come back — and the metal you melted,
    ///   which cost the longest, silently did not. It comes back as the raw
    ///   metal it was fed as, which is what the crucible remembers; the pack
    ///   already refuses to spend a unit that produces nothing (`eject`), and
    ///   this is the same rule at the other end of the machine's life.
    /// - **The blob GOES.** Without this it outlives the block, and the next
    ///   forge built on that cell wakes up with the old one's crucible — full
    ///   of metal, or half-way through a pour it never started.
    fn forget(&mut self, pos: [i32; 3]) {
        let state = State::decode(&section_kv_get(pos, STATE_KEY).unwrap_or_default());
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
        let mut state = State::decode(stored);
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
        // Manual pours use the live mould; automatic pours retain the mould
        // they started with so a slot change cannot turn the cast into a plate.
        let mould = slots[SLOT_MOULD].as_ref().map(|s| s.item.clone());

        if state.burn_remaining > 0 {
            state.burn_remaining -= 1;
        }
        // The crucible only sets while the fire is OUT; relighting melts it
        // back down, so a forgotten crucible is a delay, never a loss.
        if state.burn_remaining > 0 {
            state.idle_ticks = 0;
        } else {
            state.idle_ticks = state.idle_ticks.saturating_add(1);
        }

        if self.melt(&mut state, &mut slots, casting) {
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
        // The tap is open exactly while the pour phase runs; the metal keeps
        // moving for as long as it has somewhere to go, which is why the
        // stream finishes falling after the phase ends.
        let tapped = state.phase == Phase::Pouring;
        state
            .liquid
            .step(tapped, 1.0 / (POUR_TICKS as f32 * 0.6).max(1.0));
        // The stream ARRIVING is the moment the pour stops being a promise, and
        // it is a transition rather than a state: the metal goes on landing for
        // the rest of the pour, and a sound per tick of that is a drone.
        if state.liquid.landed() && !before.liquid.landed() {
            emit_sound(SOUND_LAND, Some(at(ctx.pos)));
        }

        write_changed_slots(ctx.pos, &before_slots, &slots);
        if state != before {
            *stored = state.encode();
        }

        // The row (lit or not) and the parts mask are both compared against
        // what the world CURRENTLY shows rather than tracked as transitions,
        // so a furnace whose visual ever gets out of step heals on the next
        // tick instead of staying wrong forever.
        let want_row = if pouring_visually(&state) {
            ctx.variant_or_base(ROW_POUR)
        } else if state.burn_remaining > 0 {
            ctx.variant_or_base(ROW_LIT)
        } else {
            ctx.block
        };
        if ctx.current != want_row {
            swap_block(ctx.pos, want_row);
        }
        // Submitted UNCONDITIONALLY, like the draw set below and for the same
        // reason: comparing against the mod's own previous state is a
        // TRANSITION check, and a machine that has never transitioned has
        // never been drawn. A freshly placed furnace's mask equals its own
        // previous mask, so it was never submitted and the model came up with
        // every optional part hidden — no lever, no coals — until the first
        // time something changed. The engine drops an unchanged submission
        // before it touches a cell, so the gate belongs there, not here.
        out.parts(ctx.pos, self.parts_mask(&state), None);
        // The presentation is submitted UNCONDITIONALLY, and that is deliberate.
        //
        // A draw set is a runtime record, not saved state: a section unload
        // drops it, a slot change happens between ticks, and a reload starts
        // with nothing declared. Every mod-side memo of "I already drew this"
        // is wrong in at least one of those cases, and wrong there means a
        // machine that draws nothing for the rest of the session. So the mod
        // keeps no memo; the ENGINE owns the change gate (an unchanged
        // submission is compared and dropped before any name resolve,
        // replication or allocation). One gate, where the truth is.
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
            // The melt bar's denominator is the INPUT's melt time, because that
            // is what `melt` counts against — the crucible's metal is empty
            // for the whole first melt, and a bar reading its own arithmetic
            // instead of the timer's snaps or stalls at the end of every one.
            let melting = slots[SLOT_METAL].as_ref().map(|s| s.item.as_str());
            self.publish_gauges(ctx, &state, casting, pourable, melting);
        }
    }
}

/// Whether the furnace wants its fire lit: something to melt, or metal in the
/// crucible to keep liquid. An idle forge never eats coal; a loaded one does,
/// because letting it go cold is what sets the crucible.
fn wants_heat(state: &State, can_melt: bool) -> bool {
    can_melt || state.units > 0
}

/// The mould sitting in a placed furnace's basin, read from its container.
/// The click path has no slot snapshot of its own — the tick's is a tick old
/// and belongs to a different call.
fn mould_in(anchor: [i32; 3]) -> Option<String> {
    container_get(anchor)?
        .get(SLOT_MOULD)?
        .as_ref()
        .map(|s| s.item.clone())
}

/// What the metal in the basin is casting: the mould's class, or a bare plate
/// when the basin is empty.
fn cast_class<'a>(c: &'a Casting, mould: Option<&str>) -> &'a str {
    mould.and_then(|m| c.mould_class(m)).unwrap_or(CLASS_PLATE)
}

/// What metal already running becomes: the mould's own route, else a plate.
/// The ONE place that decision is made, so the shape drawn while it sets and
/// the shape that pops out cannot disagree.
///
/// The plate FALLBACK is what makes this different from the lever's gate
/// ([`ForgingFurnaceSpec::pourable`], which asks about the mould's class and
/// nothing else). The gate refuses a pour that would produce nothing; this
/// answers a pour already committed, whose mould may have been swapped since
/// for one this metal cannot fill. Metal that ran becomes SOMETHING.
fn cast_result(
    c: &Casting,
    caches: &mut Caches,
    metal: &str,
    mould: Option<&str>,
) -> Option<ItemStackData> {
    caches
        .recipe_for(cast_class(c, mould), metal)
        .or_else(|| caches.recipe_for(CLASS_PLATE, metal))
}

impl ForgingFurnaceSpec {
    /// Melt one raw metal into the crucible. Fuel is only spent when there is
    /// something to melt AND room for it (the furnace contract — an idle forge
    /// never eats coal), and the crucible never mixes metals.
    /// Returns whether the fire CAUGHT this tick — the sound belongs to the
    /// caller, so this stays a pure state transition and can be tested without
    /// a host.
    fn melt(
        &self,
        state: &mut State,
        slots: &mut [Option<ItemStackData>],
        casting: &Casting,
    ) -> bool {
        let input = slots[SLOT_METAL]
            .as_ref()
            .filter(|s| s.count > 0)
            .map(|s| s.item.clone());
        let can_melt = input.as_deref().is_some_and(|item| {
            casting.is_metal(item)
                && state.units < CRUCIBLE_MAX
                // `state.metal` is the CANONICAL form, so the incoming item has
                // to be canonicalised before they are compared — otherwise a
                // second plate never matches the raw metal the first one became
                // and the machine silently ignores the whole stack.
                && (state.metal.is_empty() || state.metal == casting.molten_form(item))
        });
        // The fire is wanted whenever there is something to melt OR metal to
        // keep liquid. Gating relight on `can_melt` alone BRICKS a full
        // crucible: at 8 units nothing can melt, so nothing can relight, so it
        // hardens ten seconds later and stays hardened forever with the metal
        // trapped inside it.
        let mut caught = false;
        if wants_heat(state, can_melt) && state.burn_remaining == 0 {
            self.relight(state, slots);
            caught = state.burn_remaining > 0;
        }
        if !can_melt || state.burn_remaining == 0 {
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

    fn relight(&self, state: &mut State, slots: &mut [Option<ItemStackData>]) {
        let Some(fuel) = slots[SLOT_FUEL].clone() else {
            return;
        };
        // Fuel burn ticks come off the item row; `Caches` would need threading
        // through, and this runs at most once per fuel item.
        let burn = item_info(&fuel.item)
            .map(|i| i.fuel_burn_ticks)
            .unwrap_or(0);
        if burn > 0 {
            state.burn_remaining = burn;
            state.burn_max = burn;
            consume_one(&mut slots[SLOT_FUEL]);
        }
    }

    /// Advance a pour that the lever started. Heat is NOT required: the metal
    /// is already liquid, and a pour that stalled half-way down the groove
    /// because the coal ran out would be a bug, not a mechanic.
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

    /// The cast is finished: work out what it became and pop it out of the
    /// basin as an item entity. The mould's class is read HERE, so pulling the
    /// mould out mid-pour yields a plate — the metal was already committed,
    /// the shape was not.
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
            // Over the basin, in the model's OWN pixels — the engine turns the
            // point by the placed facing. A world offset off the anchor is
            // right at one facing and puts the cast inside the masonry at the
            // other three.
            let Some(spot) =
                block_local_to_world(ctx.pos, vec![EJECT_AT]).and_then(|p| p.into_iter().next())
            else {
                // The cell went unreadable between the container read and now.
                // Leave the phase alone and try again next tick: a cast the
                // machine already spent a crucible unit on must not evaporate
                // because a section was mid-stream for one frame.
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
            // At the BASIN rather than the anchor: this is the one sound whose
            // exact source the player is looking straight at.
            emit_sound(SOUND_CAST, Some(spot));
        }
        state.phase = Phase::Idle;
        state.phase_ticks = 0;
        state.pour_mould.clear();
        state.liquid = Liquid::default();
        // The crucible pays only for metal that BECAME something. With no
        // route at all — not even a plate, which needs a pack shipping a metal
        // with no `forge:cast_plate` row — the pour ends and the unit stays
        // put. Metal is never destroyed for nothing: a machine that eats your
        // ore and hands back silence is the worst thing this loop could do,
        // and the lever is already dead in that state (`pourable` asks the
        // same question), so the metal is not stranded either.
        if result.is_some() {
            state.units = state.units.saturating_sub(1);
            if state.units == 0 {
                state.metal.clear();
            }
        }
    }

    /// What is sitting in the basin: the mould, and the product the metal is
    /// becoming. Both are drawn as their own ITEMS, so the cast is always the
    /// real thing rather than an authored stand-in — which is also why the
    /// mod does not need a model cube per mould or per product.
    ///
    /// The cast appears the moment metal LANDS, not when the pour ends: while
    /// it is still arriving the basin draws the product at the pour's own
    /// fill, so the metal accumulates in the shape it is becoming. Only with
    /// a mould — metal poured onto the bare crucible has no shape to grow
    /// into and stays the square pool (`liquid.rs`).
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
            // The SAME answer `eject` will give, plate fallback and all — the
            // metal in the basin is drawn as the thing that is about to pop
            // out of it, glowing pour-hot and growing with the fill.
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

    /// Everything the model STAGES, from the state alone.
    ///
    /// Colour is deliberately absent: the cast and the metal are both DRAWN
    /// (`set_block_draw`), where a colour costs nothing per tick, so nothing
    /// the parts mask stages needs tinting.
    ///
    /// The pour is animated in STAGES rather than by moving geometry, and that
    /// is a cost decision as much as a simplicity one: a parts change re-meshes
    /// the section, so smooth per-tick motion would re-mesh it twenty times a
    /// second. Stages carry the shape of the action (the stream reaches down,
    /// the metal accumulates, the tap drains) and the row's particle emitter
    /// carries the motion, at frame rate, for free.
    fn parts_mask(&self, state: &State) -> u32 {
        let mask = if state.burn_remaining > 0 {
            PART_COALS
        } else {
            0
        };
        // A bit past the declared list names no cube and simply draws nothing,
        // so a stray one is invisible rather than wrong.
        debug_assert_eq!(mask >> PARTS.len(), 0, "a part bit the row never declared");
        mask
    }

    /// Whether pulling the lever right now would produce something.
    ///
    /// A pour that matches no `(class, metal)` route would run the whole
    /// animation and hand back nothing — which the player sees as the machine
    /// breaking. The lever is dead in that case, and it is dead for a reason
    /// it can SHOW: the basin holds a mould this metal cannot fill.
    ///
    /// Deliberately WITHOUT [`cast_result`]'s plate fallback: this is the
    /// gate, and it must answer for the mould you actually put in — falling
    /// back here would light the lever for a shears mould full of copper and
    /// hand back a plate instead. The fallback belongs to metal already
    /// running, whose mould may have changed since.
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

// ---------------------------------------------------------------------------
// The hand: what a click on a placed furnace does
// ---------------------------------------------------------------------------

impl ForgingFurnaceSpec {
    /// The lever was pulled. Starting the pour is the only thing a click can
    /// do, so this is the whole control surface — and it lives in the panel,
    /// where a control can say what it is and refuse when it cannot be used.
    pub fn pull_lever(&self, anchor: [i32; 3], caches: &mut Caches) {
        let stored = section_kv_get(anchor, STATE_KEY).unwrap_or_default();
        let mut state = State::decode(&stored);
        let mould = mould_in(anchor);
        if !self.pourable(&state, caches, mould.as_deref()) {
            return;
        }
        state.phase = Phase::Pouring;
        state.phase_ticks = 0;
        section_kv_set(anchor, STATE_KEY, state.encode());

        // AFTER the gate, never before it: the lever is the one control on the
        // machine, and a click that made the noise but not the pour would be
        // the most confusing feedback the panel could give.
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
