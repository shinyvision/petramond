//! Mining controller: turns a held left-mouse-button + a raycast target into a
//! timed break, producing a [`BreakEvent`] the frame a block finishes breaking.
//!
//! The model is deliberately small and pure: progress is a single `elapsed`
//! accumulator measured against [`break_time`]. Changing the target, releasing
//! the button, opening the inventory, switching tools, or losing the raycast all
//! reset progress. Instant blocks (`hardness == 0`) break the first qualifying
//! frame and never display a break overlay.
//!
//! Tools: a held tool whose KIND matches the block's
//! [`Block::preferred_tool`] (a pickaxe on stone/ore, an axe on wood, a shovel on
//! dirt/sand) mines it faster by its tier (×2/×4/×6/×8 for wooden/stone/iron/
//! diamond), scaled by the kind's efficiency — the shovel digs at 0.5625× so it
//! is uniformly slower than a pickaxe/axe of equal tier. For a
//! tool-gated block (stone/ore) a pickaxe must also meet the block's
//! [`Block::harvest_tier`] to unlock the drop; a wrong-kind, insufficient, or
//! absent tool mines at the bare-hand rate and — for those blocks — yields nothing.

use crate::block::Block;
use crate::item::Tool;
use crate::mathh::IVec3;
use crate::world::World;

/// Seconds of mining per unit of hardness, bare-handed. Anchors wood
/// (`hardness 2.0`) to a 5.0 s break, matching the survival goal.
pub const SECONDS_PER_HARDNESS_HAND: f32 = 2.5;
/// Number of distinct break-overlay stages (`0..BREAK_STAGES`).
pub const BREAK_STAGES: u8 = 10;

/// The per-tick mining progress for the block currently under the crosshair.
///
/// Holds the active target cell, the block being mined (cached so a `set_block`
/// elsewhere can't desync the timer mid-break), and the accumulated mining time.
#[derive(Clone, Debug, Default)]
pub struct MiningState {
    target: Option<IVec3>,
    block: Option<Block>,
    /// Tool in use on this target (`None` = bare hand). Cached so a tool switch
    /// mid-break restarts progress and the overlay reads the right break time.
    tool: Option<Tool>,
    elapsed: f32,
}

/// Emitted by [`MiningState::update`] the frame a block finishes breaking.
///
/// `harvested == false` means the block broke but yields no drop (Stone/Ore by
/// hand in 0.1); the caller still clears the cell, it just rolls no drops.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BreakEvent {
    pub pos: IVec3,
    pub block: Block,
    pub harvested: bool,
}

impl MiningState {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance mining by `dt`. Call every tick.
    ///
    /// - `look`: the targeted cell of the current raycast, or `None` if nothing
    ///   is targeted.
    /// - `mining_held`: left mouse button currently held down (not edge).
    /// - `inventory_open`: gates mining off entirely while the inventory is open.
    /// - `world`: looked up to resolve the targeted block.
    /// - `tool`: the held mining tool (`None` = bare hand). Drives break speed +
    ///   whether the block is harvested.
    ///
    /// Returns `Some(BreakEvent)` exactly on the frame the block breaks; resets
    /// progress whenever the target changes, the tool changes, or the button
    /// releases.
    pub fn update(
        &mut self,
        dt: f32,
        look: Option<IVec3>,
        mining_held: bool,
        inventory_open: bool,
        world: &World,
        tool: Option<Tool>,
    ) -> Option<BreakEvent> {
        self.update_core(dt, look, mining_held, inventory_open, tool, &|p| {
            Block::from_id(world.chunk_block(p.x, p.y, p.z))
        })
    }

    /// Pure state machine behind [`update`](Self::update): `block_at` resolves the
    /// block at a cell. Split out so tests can drive the full controller without
    /// standing up a real `World` (which spins a worker thread pool).
    fn update_core(
        &mut self,
        dt: f32,
        look: Option<IVec3>,
        mining_held: bool,
        inventory_open: bool,
        tool: Option<Tool>,
        block_at: &impl Fn(IVec3) -> Block,
    ) -> Option<BreakEvent> {
        // Not mining, inventory open, or nothing targeted -> reset and bail.
        let pos = match (mining_held, inventory_open, look) {
            (true, false, Some(cell)) => cell,
            _ => {
                self.reset();
                return None;
            }
        };

        let block = block_at(pos);

        // Unbreakable cells (Air/Water/hardness < 0) are never mining targets.
        if block.hardness() < 0.0 {
            self.reset();
            return None;
        }

        // New target, OR a tool switch on the same cell: restart the timer (the
        // break time depends on the tool, so switching mid-break starts over).
        if self.target != Some(pos) || self.tool != tool {
            self.target = Some(pos);
            self.block = Some(block);
            self.tool = tool;
            self.elapsed = 0.0;
        }

        self.elapsed += dt;

        let break_time = break_time(block, tool);
        if self.elapsed >= break_time {
            let event = BreakEvent {
                pos,
                block,
                // Harvested only when the held tool meets this block's harvest
                // requirement; below that (wrong kind, too low a tier, or bare
                // hand) it breaks but drops nothing (redstone/diamond by hand).
                harvested: harvests(block, tool),
            };
            self.reset();
            return Some(event);
        }

        None
    }

    /// Current break-overlay target + stage `0..BREAK_STAGES`, or `None` when not
    /// actively mining a breakable, non-instant block.
    pub fn overlay(&self) -> Option<(IVec3, u8)> {
        let target = self.target?;
        let block = self.block?;
        // Instant blocks never show an overlay.
        let break_time = break_time(block, self.tool);
        if break_time <= 0.0 || self.elapsed <= 0.0 {
            return None;
        }
        let stage = overlay_stage(self.elapsed, break_time);
        Some((target, stage))
    }

    /// True while a block is actively being mined (a target with accrued time).
    /// The client-side hand/dust now key on the REPLICATED `overlay()` state
    /// (multiplayer C2c-i), so this is a test-only readout of the raw latch.
    #[cfg(test)]
    #[inline]
    pub fn is_mining(&self) -> bool {
        self.target.is_some() && self.elapsed > 0.0
    }

    /// The cell currently being mined, if any. Test-only readout of the mined target.
    #[cfg(test)]
    #[inline]
    pub fn target(&self) -> Option<IVec3> {
        self.target
    }

    #[inline]
    fn reset(&mut self) {
        self.target = None;
        self.block = None;
        self.tool = None;
        self.elapsed = 0.0;
    }
}

/// The effective mining tier of `tool` against `block`: the tool's `tier` when it
/// is the block's [`preferred_tool`](Block::preferred_tool) kind (a pickaxe on
/// stone/ore, an axe on wood), else `0` (the bare-hand tier). Both the harvest
/// gate and the speed multiplier key off this, so a wrong-kind tool (an axe on
/// stone, a pickaxe on a log) mines exactly like a bare hand.
#[inline]
fn tool_power(block: Block, tool: Option<Tool>) -> u8 {
    match tool {
        Some(t) if block.preferred_tool() == Some(t.kind) => t.tier,
        _ => 0,
    }
}

/// Whether `tool` harvests `block` (i.e. the break yields its drop). True when the
/// effective [`tool_power`] meets the block's [`harvest_tier`](Block::harvest_tier):
/// hand-harvestable blocks (tier `0` — dirt, wood, plants) always drop, while
/// stone/ore need a pickaxe of sufficient tier and never drop to an axe or a hand.
#[inline]
pub fn harvests(block: Block, tool: Option<Tool>) -> bool {
    tool_power(block, tool) >= block.harvest_tier()
}

/// Total seconds to break `block` with `tool` (`None` = bare hand). `0.0` for
/// instant blocks; callers must not pass unbreakable blocks (`hardness < 0`). A
/// tool of the block's [`preferred_tool`](Block::preferred_tool) kind that also
/// meets its [`harvest_tier`](Block::harvest_tier) divides the hand time by
/// [`tool_speed`] scaled by the kind's
/// [`mining_efficiency`](crate::item::ToolKind::mining_efficiency) (so a shovel
/// digs slower than a pickaxe/axe of equal tier); a wrong-kind, insufficient, or
/// absent tool mines at the bare-hand rate.
#[inline]
pub fn break_time(block: Block, tool: Option<Tool>) -> f32 {
    let h = block.hardness();
    if h <= 0.0 {
        return 0.0;
    }
    let base = h * SECONDS_PER_HARDNESS_HAND;
    let power = tool_power(block, tool);
    // The speed-up needs a real tool (tier >= 1) of the right kind that also meets
    // the harvest tier — so an under-tier pickaxe (wood on iron ore) stays at hand
    // speed, matching the no-drop gate. `max(1)` covers wood, whose harvest tier is
    // 0: any axe (tier >= 1) speeds it, a bare hand never does.
    if power >= block.harvest_tier().max(1) {
        // `power >= 1` means the tool's kind matched the block, so `tool` is Some.
        // Scale the shared tier ladder by the kind's efficiency so a clumsier kind —
        // the shovel — is uniformly slower than a pickaxe/axe of the same tier.
        let efficiency = tool.map_or(1.0, |t| t.kind.mining_efficiency());
        base / (tool_speed(power) * efficiency)
    } else {
        base
    }
}

/// Mining-speed multiplier of a tool tier over the bare hand (Minecraft's
/// wooden ×2, stone ×4, iron ×6, diamond ×8). Only applied once the tool's kind
/// matches the block and it can actually harvest it (see [`break_time`]).
#[inline]
fn tool_speed(tier: u8) -> f32 {
    match tier {
        0 => 1.0,
        1 => 2.0,
        2 => 4.0,
        3 => 6.0,
        _ => 8.0,
    }
}

/// Map mining progress to a break-overlay stage in `0..BREAK_STAGES`.
///
/// `((elapsed / break_time) * 10).floor().clamp(0, 9)`. Caller guarantees
/// `break_time > 0.0`.
#[inline]
pub fn overlay_stage(elapsed: f32, break_time: f32) -> u8 {
    let frac = (elapsed / break_time) * BREAK_STAGES as f32;
    frac.floor().clamp(0.0, (BREAK_STAGES - 1) as f32) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::item::ToolKind;
    use crate::mathh::IVec3;

    /// A pickaxe of `tier` in hand.
    fn pick(tier: u8) -> Option<Tool> {
        Some(Tool {
            kind: ToolKind::Pickaxe,
            tier,
        })
    }

    /// An axe of `tier` in hand.
    fn axe(tier: u8) -> Option<Tool> {
        Some(Tool {
            kind: ToolKind::Axe,
            tier,
        })
    }

    /// A shovel of `tier` in hand.
    fn shovel(tier: u8) -> Option<Tool> {
        Some(Tool {
            kind: ToolKind::Shovel,
            tier,
        })
    }

    /// The targeted cell, as the controller consumes it.
    fn hit_at(pos: IVec3) -> IVec3 {
        pos
    }

    /// Drive `update_core` bare-handed with a constant single-block world.
    fn step(
        state: &mut MiningState,
        dt: f32,
        look: Option<IVec3>,
        held: bool,
        inv_open: bool,
        block: Block,
    ) -> Option<BreakEvent> {
        state.update_core(dt, look, held, inv_open, None, &|_| block)
    }

    /// Like [`step`] but with `tool` in hand.
    fn step_with_tool(
        state: &mut MiningState,
        dt: f32,
        look: Option<IVec3>,
        held: bool,
        inv_open: bool,
        tool: Option<Tool>,
        block: Block,
    ) -> Option<BreakEvent> {
        state.update_core(dt, look, held, inv_open, tool, &|_| block)
    }

    #[test]
    fn break_time_anchors_match_contract() {
        // Wood: hardness 2.0 -> 5.0 s by hand.
        assert_eq!(break_time(Block::OakLog, None), 5.0);
        // Instant plants: 0.0 s.
        assert_eq!(break_time(Block::Poppy, None), 0.0);
        assert_eq!(break_time(Block::ShortGrass, None), 0.0);
    }

    #[test]
    fn wood_breaks_at_five_seconds() {
        let mut state = MiningState::new();
        let pos = IVec3::new(1, 2, 3);
        let hit = hit_at(pos);

        // Well before 5.0 s (4.0 s) we are still mining with no break event.
        for _ in 0..40 {
            assert!(step(&mut state, 0.1, Some(hit), true, false, Block::OakLog).is_none());
        }
        assert!(state.is_mining());

        // Mine until it breaks, tracking accumulated time. It must break right
        // around the 5.0 s anchor (within one tick of float-summed dt).
        let dt = 0.1;
        let mut elapsed = 4.0;
        let mut ev = None;
        for _ in 0..20 {
            elapsed += dt;
            if let Some(e) = step(&mut state, dt, Some(hit), true, false, Block::OakLog) {
                ev = Some(e);
                break;
            }
        }
        let ev = ev.expect("wood should break around 5.0 s");
        assert!(
            (elapsed - 5.0).abs() <= dt + 1e-3,
            "wood broke at {elapsed} s, expected ~5.0 s"
        );
        assert_eq!(ev.pos, pos);
        assert_eq!(ev.block, Block::OakLog);
        assert!(ev.harvested, "wood is hand-harvestable");

        // Breaking resets progress.
        assert!(!state.is_mining());
        assert_eq!(state.overlay(), None);
    }

    #[test]
    fn instant_plant_breaks_in_one_update() {
        let mut state = MiningState::new();
        let pos = IVec3::new(0, 64, 0);
        let hit = hit_at(pos);
        let ev = step(&mut state, 0.016, Some(hit), true, false, Block::Poppy)
            .expect("instant block breaks on the first qualifying frame");
        assert_eq!(ev.block, Block::Poppy);
        assert!(ev.harvested);
        // Instant blocks never show an overlay even mid-break.
        assert_eq!(state.overlay(), None);
    }

    #[test]
    fn stone_breaks_but_is_not_harvested() {
        let mut state = MiningState::new();
        let pos = IVec3::new(5, 5, 5);
        let hit = hit_at(pos);
        let total = break_time(Block::Stone, None); // 1.5 * 2.5 = 3.75 s
        let dt = 0.05;
        let mut ev = None;
        for _ in 0..((total / dt) as usize + 2) {
            if let Some(e) = step(&mut state, dt, Some(hit), true, false, Block::Stone) {
                ev = Some(e);
                break;
            }
        }
        let ev = ev.expect("stone eventually breaks");
        assert_eq!(ev.block, Block::Stone);
        assert!(!ev.harvested, "stone yields nothing by hand");
    }

    #[test]
    fn dirt_is_harvested() {
        let mut state = MiningState::new();
        let pos = IVec3::new(2, 2, 2);
        let hit = hit_at(pos);
        let total = break_time(Block::Dirt, None); // 0.5 * 2.5 = 1.25 s
        let dt = 0.05;
        let mut ev = None;
        for _ in 0..((total / dt) as usize + 2) {
            if let Some(e) = step(&mut state, dt, Some(hit), true, false, Block::Dirt) {
                ev = Some(e);
                break;
            }
        }
        let ev = ev.expect("dirt eventually breaks");
        assert_eq!(ev.block, Block::Dirt);
        assert!(ev.harvested, "dirt is hand-harvestable");
    }

    #[test]
    fn overlay_stage_climbs_zero_to_nine() {
        let total = break_time(Block::Stone, None);
        // Just-started progress is stage 0.
        assert_eq!(overlay_stage(0.0001, total), 0);
        // Just-before-break is stage 9.
        assert_eq!(overlay_stage(total - 0.0001, total), 9);
        // Spot-check the climb is monotone and spans the full 0..=9 range.
        let mut seen = [false; BREAK_STAGES as usize];
        let mut last = 0u8;
        let steps = 200;
        for i in 0..steps {
            let elapsed = total * (i as f32 / steps as f32);
            let s = overlay_stage(elapsed, total);
            assert!(s >= last, "stage must not decrease");
            last = s;
            seen[s as usize] = true;
        }
        assert_eq!(last, 9);
        assert!(seen.iter().all(|&b| b), "every stage 0..9 should appear");
    }

    #[test]
    fn overlay_reports_target_and_stage_while_mining() {
        let mut state = MiningState::new();
        let pos = IVec3::new(7, 8, 9);
        let hit = hit_at(pos);
        // Mine ~half of stone's break time.
        let total = break_time(Block::Stone, None);
        let half = total / 2.0;
        let dt = 0.05;
        let mut t = 0.0;
        while t + dt < half {
            step(&mut state, dt, Some(hit), true, false, Block::Stone);
            t += dt;
        }
        let (otarget, stage) = state.overlay().expect("overlay while mining stone");
        assert_eq!(otarget, pos);
        assert!(
            (4..=5).contains(&stage),
            "halfway should be ~stage 4-5, got {stage}"
        );
    }

    #[test]
    fn changing_target_resets_progress() {
        let mut state = MiningState::new();
        let a = hit_at(IVec3::new(0, 0, 0));
        let b = hit_at(IVec3::new(0, 0, 1));

        for _ in 0..10 {
            step(&mut state, 0.1, Some(a), true, false, Block::Stone);
        }
        assert!(state.is_mining());
        let (_, before) = state.overlay().unwrap();
        assert!(before > 0);

        // Switching the target restarts the timer for this frame.
        step(&mut state, 0.1, Some(b), true, false, Block::Stone);
        let (target, stage) = state.overlay().unwrap();
        assert_eq!(target, IVec3::new(0, 0, 1));
        assert_eq!(stage, 0, "target switch resets elapsed to one frame of dt");
    }

    #[test]
    fn releasing_the_button_resets() {
        let mut state = MiningState::new();
        let hit = hit_at(IVec3::new(3, 3, 3));
        for _ in 0..10 {
            step(&mut state, 0.1, Some(hit), true, false, Block::Stone);
        }
        assert!(state.is_mining());

        // Button up: progress clears.
        assert!(step(&mut state, 0.1, Some(hit), false, false, Block::Stone).is_none());
        assert!(!state.is_mining());
        assert_eq!(state.overlay(), None);
    }

    #[test]
    fn inventory_open_gates_mining_off() {
        let mut state = MiningState::new();
        let hit = hit_at(IVec3::new(1, 1, 1));
        for _ in 0..5 {
            step(&mut state, 0.1, Some(hit), true, false, Block::Stone);
        }
        assert!(state.is_mining());
        // Opening the inventory resets even with the button held.
        assert!(step(&mut state, 0.1, Some(hit), true, true, Block::Stone).is_none());
        assert!(!state.is_mining());
    }

    #[test]
    fn no_target_resets() {
        let mut state = MiningState::new();
        let hit = hit_at(IVec3::new(1, 1, 1));
        for _ in 0..5 {
            step(&mut state, 0.1, Some(hit), true, false, Block::Stone);
        }
        assert!(state.is_mining());
        // Losing the raycast (look = None) clears progress.
        assert!(step(&mut state, 0.1, None, true, false, Block::Stone).is_none());
        assert!(!state.is_mining());
    }

    #[test]
    fn unbreakable_block_is_never_a_target() {
        let mut state = MiningState::new();
        let hit = hit_at(IVec3::new(0, 0, 0));
        // Water has hardness < 0: never mined.
        assert!(step(&mut state, 1.0, Some(hit), true, false, Block::Water).is_none());
        assert!(!state.is_mining());
        assert_eq!(state.target(), None);
    }

    #[test]
    fn pickaxe_speeds_and_harvest_gate_by_tier() {
        // Wooden pickaxe halves stone's hand time; a stone pickaxe quarters it.
        assert_eq!(break_time(Block::Stone, None), 3.75);
        assert_eq!(break_time(Block::Stone, pick(1)), 3.75 / 2.0);
        assert_eq!(break_time(Block::Stone, pick(2)), 3.75 / 4.0);
        // Iron needs a stone pickaxe: a wooden one mines it at the hand rate.
        assert_eq!(
            break_time(Block::IronOre, pick(1)),
            break_time(Block::IronOre, None)
        );
        assert_eq!(break_time(Block::IronOre, pick(2)), 7.5 / 4.0);
        // Diamond ore needs an iron pickaxe: a stone one is still hand speed; iron
        // is ×6 and diamond ×8.
        assert_eq!(
            break_time(Block::DiamondOre, pick(2)),
            break_time(Block::DiamondOre, None)
        );
        assert_eq!(break_time(Block::DiamondOre, pick(3)), 7.5 / 6.0);
        assert_eq!(break_time(Block::DiamondOre, pick(4)), 7.5 / 8.0);
    }

    #[test]
    fn axes_speed_wood_and_pickaxes_do_not() {
        // Oak log is wood (hardness 2.0 -> 5.0 s by hand). Each axe tier mines it
        // faster than the last: ×2/×4/×6/×8 for wooden/stone/iron/diamond.
        assert_eq!(break_time(Block::OakLog, None), 5.0);
        assert_eq!(break_time(Block::OakLog, axe(1)), 5.0 / 2.0);
        assert_eq!(break_time(Block::OakLog, axe(2)), 5.0 / 4.0);
        assert_eq!(break_time(Block::OakLog, axe(3)), 5.0 / 6.0);
        assert_eq!(break_time(Block::OakLog, axe(4)), 5.0 / 8.0);
        // A pickaxe is the wrong kind for wood: hand speed, no faster than a stick.
        assert_eq!(break_time(Block::OakLog, pick(4)), 5.0);
        // The crafting table and chest are wood too, so axes speed them as well.
        for wood in [Block::CraftingTable, Block::Chest] {
            assert!(
                break_time(wood, axe(1)) < break_time(wood, None),
                "{wood:?} should mine faster with an axe"
            );
            assert_eq!(
                break_time(wood, pick(4)),
                break_time(wood, None),
                "{wood:?}"
            );
        }
        // Conversely, an axe is the wrong kind for stone/ore: no speed-up.
        assert_eq!(
            break_time(Block::Stone, axe(4)),
            break_time(Block::Stone, None)
        );
    }

    #[test]
    fn shovels_speed_dirt_and_sand_but_less_than_an_equal_tier_pickaxe_axe() {
        use crate::item::ToolKind;
        // Dirt is shovel-material (hardness 0.5 -> 1.25 s by hand).
        let hand = break_time(Block::Dirt, None);
        assert_eq!(hand, 1.25);

        // A shovel is, by design, a less efficient digger than the baseline kinds.
        let eff = ToolKind::Shovel.mining_efficiency();
        assert!(
            eff < 1.0,
            "shovel must be less efficient than a pickaxe/axe"
        );

        for tier in 1..=4u8 {
            let with_shovel = break_time(Block::Dirt, shovel(tier));
            // Still a real speed-up over the bare hand at every tier...
            assert!(
                with_shovel < hand,
                "shovel tier {tier} should beat the hand"
            );
            // ...yet slower than a full-efficiency (pickaxe/axe-grade) tool of the
            // same tier would be — the kind penalty applies across the board.
            let full_speed = hand / tool_speed(tier);
            assert!(
                with_shovel > full_speed,
                "shovel tier {tier} should be slower than a full-efficiency tool"
            );
            // Exact: the shared tier ladder scaled by the kind's efficiency.
            assert_eq!(with_shovel, hand / (tool_speed(tier) * eff));
        }

        // The whole dirt/sand family is shovel-sped (grass, gravel, clay, …).
        for b in [Block::Grass, Block::Sand, Block::Gravel, Block::Clay] {
            assert!(
                break_time(b, shovel(1)) < break_time(b, None),
                "{b:?} should mine faster with a shovel"
            );
        }
        // A pickaxe/axe is the wrong kind for dirt: hand speed, no faster than bare.
        assert_eq!(break_time(Block::Dirt, pick(4)), hand);
        assert_eq!(break_time(Block::Dirt, axe(4)), hand);
        // And a shovel is the wrong kind for stone/wood: no speed-up there.
        assert_eq!(
            break_time(Block::Stone, shovel(4)),
            break_time(Block::Stone, None)
        );
        assert_eq!(
            break_time(Block::OakLog, shovel(4)),
            break_time(Block::OakLog, None)
        );
    }

    #[test]
    fn iron_pickaxe_harvests_every_ore() {
        // The iron pickaxe (tier 3) unlocks the drop on every ore — including the
        // tier-3 gold/lapis/diamond ores a stone pickaxe can't crack.
        for ore in [
            Block::CoalOre,
            Block::IronOre,
            Block::CopperOre,
            Block::GoldOre,
            Block::LapisOre,
            Block::DiamondOre,
            Block::RedstoneOre,
        ] {
            assert!(
                harvests(ore, pick(3)),
                "iron pickaxe should harvest {ore:?}"
            );
            assert!(
                harvests(ore, pick(4)),
                "diamond pickaxe should harvest {ore:?}"
            );
        }
        // A stone pickaxe still can't harvest the tier-3 ores.
        assert!(!harvests(Block::GoldOre, pick(2)));
        assert!(!harvests(Block::DiamondOre, pick(2)));
        // An axe — even diamond — never harvests ore (wrong tool kind).
        assert!(!harvests(Block::GoldOre, axe(4)));
    }

    #[test]
    fn break_event_harvest_flag_follows_tool() {
        let hit = hit_at(IVec3::new(1, 1, 1));
        let dt = 0.05;
        let mine = |tool: Option<Tool>, block: Block| {
            let mut state = MiningState::new();
            let total = break_time(block, tool);
            for _ in 0..((total / dt) as usize + 2) {
                if let Some(e) =
                    step_with_tool(&mut state, dt, Some(hit), true, false, tool, block)
                {
                    return e;
                }
            }
            panic!("{block:?} should break with tool {tool:?}");
        };
        // Wooden pickaxe harvests stone; a bare hand breaks it for nothing.
        assert!(mine(pick(1), Block::Stone).harvested);
        assert!(!mine(None, Block::Stone).harvested);
        // Iron ore needs a stone pickaxe — a wooden one yields nothing.
        assert!(!mine(pick(1), Block::IronOre).harvested);
        assert!(mine(pick(2), Block::IronOre).harvested);
        // Diamond ore yields nothing to a stone pickaxe, but the iron pickaxe cracks
        // it — and a diamond gem actually drops.
        assert!(!mine(pick(2), Block::DiamondOre).harvested);
        assert!(mine(pick(3), Block::DiamondOre).harvested);
        // Wood is hand-harvestable: an axe drops it, and so does a bare hand.
        assert!(mine(axe(1), Block::OakLog).harvested);
        assert!(mine(None, Block::OakLog).harvested);
    }

    #[test]
    fn switching_tools_resets_progress() {
        let mut state = MiningState::new();
        let hit = hit_at(IVec3::new(2, 2, 2));
        // Mine stone bare-handed for a while.
        for _ in 0..10 {
            step(&mut state, 0.1, Some(hit), true, false, Block::Stone);
        }
        let (_, before) = state.overlay().unwrap();
        assert!(before > 0);
        // Pull out a pickaxe on the same cell: progress restarts this frame.
        step_with_tool(
            &mut state,
            0.1,
            Some(hit),
            true,
            false,
            pick(1),
            Block::Stone,
        );
        let (_, stage) = state.overlay().unwrap();
        assert_eq!(stage, 0, "a tool switch resets elapsed to one frame of dt");
    }
}
