use crate::block::Block;
use crate::item::Tool;
use crate::mathh::IVec3;
use crate::world::data::WorldData;

pub const SECONDS_PER_HARDNESS_HAND: f32 = 2.5;
pub const FRUITLESS_BREAK_PENALTY: f32 = 4.0;
pub const BREAK_STAGES: u8 = 10;

#[derive(Clone, Debug, Default)]
pub struct MiningState {
    target: Option<IVec3>,
    block: Option<Block>,
    tool: Option<Tool>,
    elapsed: f32,
}

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

    pub fn update(
        &mut self,
        dt: f32,
        look: Option<IVec3>,
        mining_held: bool,
        barred: bool,
        world: &WorldData,
        tool: Option<Tool>,
    ) -> Option<BreakEvent> {
        self.update_core(dt, look, mining_held, barred, tool, &|p| {
            Block::from_id(world.chunk_block(p.x, p.y, p.z))
        })
    }

    fn update_core(
        &mut self,
        dt: f32,
        look: Option<IVec3>,
        mining_held: bool,
        barred: bool,
        tool: Option<Tool>,
        block_at: &impl Fn(IVec3) -> Block,
    ) -> Option<BreakEvent> {
        let pos = match (mining_held, barred, look) {
            (true, false, Some(cell)) => cell,
            _ => {
                self.reset();
                return None;
            }
        };

        let block = block_at(pos);

        if block.hardness() < 0.0 {
            self.reset();
            return None;
        }

        self.advance(dt, pos, block, tool)
    }

    pub fn advance(
        &mut self,
        dt: f32,
        pos: IVec3,
        block: Block,
        tool: Option<Tool>,
    ) -> Option<BreakEvent> {
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
                harvested: harvests(block, tool),
            };
            self.reset();
            return Some(event);
        }

        None
    }

    pub fn overlay(&self) -> Option<(IVec3, u8)> {
        let target = self.target?;
        let block = self.block?;
        let break_time = break_time(block, self.tool);
        if break_time <= 0.0 || self.elapsed <= 0.0 {
            return None;
        }
        let stage = overlay_stage(self.elapsed, break_time);
        Some((target, stage))
    }

    #[cfg(any(test, feature = "test-support"))]
    #[inline]
    pub fn is_mining(&self) -> bool {
        self.target.is_some() && self.elapsed > 0.0
    }

    #[cfg(any(test, feature = "test-support"))]
    #[inline]
    pub fn target(&self) -> Option<IVec3> {
        self.target
    }

    #[inline]
    pub fn progress(&self) -> Option<(IVec3, f32)> {
        Some((self.target?, self.elapsed))
    }

    #[inline]
    pub fn reset(&mut self) {
        self.target = None;
        self.block = None;
        self.tool = None;
        self.elapsed = 0.0;
    }
}

#[inline]
fn tool_power(block: Block, tool: Option<Tool>) -> u8 {
    match tool {
        Some(t) if block.preferred_tool() == Some(t.kind) => t.tier,
        _ => 0,
    }
}

#[inline]
pub fn harvests(block: Block, tool: Option<Tool>) -> bool {
    tool_power(block, tool) >= block.harvest_tier()
}

#[inline]
pub fn break_time(block: Block, tool: Option<Tool>) -> f32 {
    let h = block.hardness();
    if h <= 0.0 {
        return 0.0;
    }
    let base = h * SECONDS_PER_HARDNESS_HAND;
    let power = tool_power(block, tool);
    if power >= block.harvest_tier().max(1) {
        if block.cut_by_preferred_tool() {
            return 0.0;
        }
        let efficiency = tool.map_or(1.0, |t| t.kind.mining_efficiency());
        let speed = tool.map_or(1.0, |t| t.speed);
        base / (speed * efficiency).max(1.0)
    } else if !harvests(block, tool) {
        base * FRUITLESS_BREAK_PENALTY
    } else {
        base
    }
}

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

    fn pick(tier: u8) -> Option<Tool> {
        Some(Tool::new(ToolKind::Pickaxe, tier))
    }

    fn axe(tier: u8) -> Option<Tool> {
        Some(Tool::new(ToolKind::Axe, tier))
    }

    fn hit_at(pos: IVec3) -> IVec3 {
        pos
    }

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
    fn only_a_break_that_yields_nothing_pays_the_penalty() {
        let penalised = |b: Block, t: Option<Tool>| {
            let base = b.hardness() * SECONDS_PER_HARDNESS_HAND;
            (break_time(b, t) - base * FRUITLESS_BREAK_PENALTY).abs() < 1e-4
        };
        assert!(penalised(Block::Stone, None));
        assert!(penalised(Block::OakLog, None));
        assert!(penalised(Block::Stone, axe(4)));
        assert!(penalised(Block::OakLog, pick(4)));
        assert!(penalised(Block::DiamondOre, pick(2)));
        for b in [Block::Dirt, Block::Sand, Block::Gravel, Block::OakPlanks] {
            assert!(!penalised(b, None), "{b:?}");
            assert_eq!(
                break_time(b, None),
                b.hardness() * SECONDS_PER_HARDNESS_HAND,
                "{b:?}"
            );
        }
    }

    #[test]
    fn a_log_breaks_when_its_break_time_elapses() {
        let mut state = MiningState::new();
        let pos = IVec3::new(1, 2, 3);
        let hit = hit_at(pos);

        let total = break_time(Block::OakLog, None);
        for _ in 0..((total / 0.1) as usize - 10) {
            assert!(step(&mut state, 0.1, Some(hit), true, false, Block::OakLog).is_none());
        }
        assert!(state.is_mining());

        let dt = 0.1;
        let mut elapsed = total - 1.0;
        let mut ev = None;
        for _ in 0..40 {
            elapsed += dt;
            if let Some(e) = step(&mut state, dt, Some(hit), true, false, Block::OakLog) {
                ev = Some(e);
                break;
            }
        }
        let ev = ev.expect("wood should break at its break time");
        assert!(
            (elapsed - total).abs() <= dt + 1e-3,
            "wood broke at {elapsed} s, expected ~{total} s"
        );
        assert_eq!(ev.pos, pos);
        assert_eq!(ev.block, Block::OakLog);
        assert_eq!(ev.harvested, harvests(Block::OakLog, None));

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
        assert_eq!(state.overlay(), None);
    }

    #[test]
    fn overlay_stage_climbs_zero_to_nine() {
        let total = break_time(Block::Stone, None);
        assert_eq!(overlay_stage(0.0001, total), 0);
        assert_eq!(overlay_stage(total - 0.0001, total), 9);
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

        for _ in 0..(break_time(Block::Stone, None) / 0.5) as usize {
            step(&mut state, 0.1, Some(a), true, false, Block::Stone);
        }
        assert!(state.is_mining());
        let (_, before) = state.overlay().unwrap();
        assert!(before > 0);

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
        assert!(step(&mut state, 0.1, None, true, false, Block::Stone).is_none());
        assert!(!state.is_mining());
    }

    #[test]
    fn unbreakable_block_is_never_a_target() {
        let mut state = MiningState::new();
        let hit = hit_at(IVec3::new(0, 0, 0));
        assert!(step(&mut state, 1.0, Some(hit), true, false, Block::Water).is_none());
        assert!(!state.is_mining());
        assert_eq!(state.target(), None);
    }

    #[test]
    fn break_event_harvest_flag_follows_tool() {
        let hit = hit_at(IVec3::new(1, 1, 1));
        let dt = 0.05;
        let mine = |tool: Option<Tool>, block: Block| {
            let mut state = MiningState::new();
            let total = break_time(block, tool);
            for _ in 0..((total / dt) as usize + 2) {
                if let Some(e) = step_with_tool(&mut state, dt, Some(hit), true, false, tool, block)
                {
                    return e;
                }
            }
            panic!("{block:?} should break with tool {tool:?}");
        };
        for (tool, block) in [
            (pick(1), Block::Stone),
            (None, Block::Stone),
            (pick(1), Block::IronOre),
            (pick(2), Block::IronOre),
            (pick(2), Block::DiamondOre),
            (pick(3), Block::DiamondOre),
            (axe(1), Block::OakLog),
            (None, Block::OakLog),
            (pick(4), Block::OakLog),
        ] {
            assert_eq!(mine(tool, block).harvested, harvests(block, tool));
        }
    }

    #[test]
    fn switching_tools_resets_progress() {
        let mut state = MiningState::new();
        let hit = hit_at(IVec3::new(2, 2, 2));
        for _ in 0..(break_time(Block::Stone, None) / 0.5) as usize {
            step(&mut state, 0.1, Some(hit), true, false, Block::Stone);
        }
        let (_, before) = state.overlay().unwrap();
        assert!(before > 0);
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
