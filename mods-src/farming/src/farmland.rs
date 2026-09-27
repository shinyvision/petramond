use mod_sdk::*;
use weather_core::FieldParams;

use crate::content::Content;
use crate::kv_counter::kv_counter_bump;

pub const HYDRATION_RADIUS: i32 = 4;

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Hydration {
    Hydrated,
    Dry,
    Unknown,
}

pub fn probe(content: &Content, sky: Option<&FieldParams>, pos: [i32; 3]) -> Hydration {
    let mut cells = Vec::with_capacity((HYDRATION_RADIUS as usize * 2 + 1).pow(2) - 1);
    for dz in -HYDRATION_RADIUS..=HYDRATION_RADIUS {
        for dx in -HYDRATION_RADIUS..=HYDRATION_RADIUS {
            if dx == 0 && dz == 0 {
                continue;
            }
            cells.push([pos[0] + dx, pos[1], pos[2] + dz]);
        }
    }
    let mut any_unknown = false;
    for got in get_blocks(cells) {
        match got {
            Some(b) if b == content.water => return Hydration::Hydrated,
            Some(_) => {}
            None => any_unknown = true,
        }
    }
    if rained_on(sky, pos) {
        return Hydration::Hydrated;
    }
    if any_unknown {
        Hydration::Unknown
    } else {
        Hydration::Dry
    }
}

fn rained_on(sky: Option<&FieldParams>, pos: [i32; 3]) -> bool {
    let Some(params) = sky else {
        return false;
    };
    if weather_core::rain(pos[0] as f64 + 0.5, pos[2] as f64 + 0.5, params) <= 0.0 {
        return false;
    }
    let above = [pos[0], pos[1] + 1, pos[2]];
    light_at(above).is_some_and(|l| l.sky >= weather_core::DIRECT_SKY_MIN)
}

pub fn on_block_placed_above(content: &Content, pos: [i32; 3], block: BlockId) {
    if content.crop_stage(block).is_some() {
        return;
    }
    let below = [pos[0], pos[1] - 1, pos[2]];
    match get_block(below) {
        Some(b) if content.is_farmland(b) => {
            set_block(below, content.dirt);
        }
        _ => {}
    }
}

const IDLE_KEY: &str = "farming:idle";
const IDLE_REVERT_TICKS: u8 = 3;

pub fn on_hook(content: &Content, sky: Option<&FieldParams>, kind: BlockHookKind, pos: [i32; 3]) {
    match kind {
        BlockHookKind::RandomTick => random_tick(content, sky, pos),
        BlockHookKind::NeighborUpdate | BlockHookKind::ScheduledTick => {}
    }
}

fn random_tick(content: &Content, sky: Option<&FieldParams>, pos: [i32; 3]) {
    let Some(current) = get_block(pos) else {
        return;
    };
    if !content.is_farmland(current) {
        return;
    }
    let mut carry_idle = None;
    match get_block([pos[0], pos[1] + 1, pos[2]]) {
        None => {}
        Some(above) if content.crop_stage(above).is_some() => {
            section_kv_delete(pos, IDLE_KEY);
        }
        Some(_) => {
            let idle = kv_counter_bump(pos, IDLE_KEY);
            if idle >= IDLE_REVERT_TICKS {
                set_block(pos, content.dirt);
                return;
            }
            carry_idle = Some(idle);
        }
    }
    let (dry_skin, wet_skin) = content
        .farmland_skins(current)
        .unwrap_or((content.farmland_dry, content.farmland_wet));
    let want = match probe(content, sky, pos) {
        Hydration::Hydrated => wet_skin,
        Hydration::Dry => dry_skin,
        Hydration::Unknown => current,
    };
    if current != want {
        set_block(pos, want);
    }
    if let Some(idle) = carry_idle {
        section_kv_set(pos, IDLE_KEY, vec![idle]);
    }
}
