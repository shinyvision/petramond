//! What the golem wears over its head when work is not going ahead, and what
//! it answers a player who asks: a thinking bubble while the planner is
//! finding another way (it nearly always does), a warning sign when only the
//! player can help — and the reason, in words.

use mod_sdk::*;

use super::tuning::{
    mark::{MARK_BOB, MARK_CLEAR, MARK_SIZE, MARK_STEP},
    patience::{STUCK_AFTER, THINK_AFTER},
};
use super::{Body, Crew, Ctx, Step};
use crate::project::{Hold, Phase, Project};

const WARNING_TILE: &str = "builder:golem_warning";
const THINKING_TILE: &str = "builder:golem_thinking";

const GOLEM_HEIGHT: f64 = 1.5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trouble {
    /// The planner and the golem get out of this on their own.
    Thinking,
    /// Someone has to help.
    Stuck,
}

/// The mark as last submitted: which one, and where from the feet.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Mark {
    trouble: Trouble,
    offset: [f32; 3],
}

pub fn of(project: &Project, crew: &Crew, now: u64) -> Option<Trouble> {
    if !matches!(project.phase(), Phase::Working | Phase::Returning) {
        return None;
    }
    match project.hold() {
        Some(Hold::Player) => return None,
        Some(_) => return Some(Trouble::Stuck),
        None => {}
    }
    if now > crew.pace.progress_at + STUCK_AFTER {
        return Some(Trouble::Stuck);
    }
    if let Some(asked_about) = crew.presence.asked_about {
        return asked_about;
    }
    let getting_out = crew.rescue.stuck.is_some()
        || matches!(
            crew.step,
            Step::Hop { .. } | Step::Rise { .. } | Step::Relocate { .. }
        );
    (getting_out || now > crew.pace.busy_at + THINK_AFTER).then_some(Trouble::Thinking)
}

/// Why, in the owner's words; what it is doing when nothing is wrong.
pub fn reason(project: &Project, crew: &Crew, trouble: Option<Trouble>) -> String {
    let told = [&project.note, &crew.note]
        .into_iter()
        .find(|note| !note.is_empty())
        .cloned()
        .or_else(|| crew.why.told().or(crew.why.pondering()).map(str::to_owned));
    match (trouble, told) {
        (Some(_), Some(told)) => told,
        (None, Some(told)) if project.hold().is_some() => told,
        (Some(Trouble::Stuck), None) => "The golem has found nothing it can do".into(),
        (Some(Trouble::Thinking), None) => "Working out what to do next".into(),
        (None, _) => match (project.hold(), project.phase()) {
            (Some(_), _) => "Paused".into(),
            (None, Phase::Returning) => "Returning materials".into(),
            (None, Phase::Emerging) => "Digging itself out".into(),
            (None, Phase::Burrowing) => "Burrowing home".into(),
            (None, _) => "Building".into(),
        },
    }
}

/// Keep the mark over the golem's head in step with its trouble.
pub fn show(ctx: &mut Ctx, crew: &mut Crew, body: &Body, trouble: Option<Trouble>) {
    let mark = trouble.map(|trouble| Mark {
        trouble,
        offset: hang(ctx, body),
    });
    let moved = |was: Mark, now: Mark| {
        was.trouble != now.trouble
            || (0..3).any(|a| (was.offset[a] - now.offset[a]).abs() > MARK_STEP)
    };
    let changed = match (crew.presence.mark, mark) {
        (None, None) => false,
        (Some(was), Some(now)) => moved(was, now),
        _ => true,
    };
    if !changed {
        return;
    }
    let prims = mark
        .map(|mark| DrawPrim::Sprite {
            at: mark.offset,
            scale: MARK_SIZE,
            yaw: 0.0,
            pitch: 0.0,
            spin: 0.0,
            bob: MARK_BOB,
            faces_viewer: true,
            tile: match mark.trouble {
                Trouble::Thinking => THINKING_TILE,
                Trouble::Stuck => WARNING_TILE,
            }
            .into(),
            tint: [255; 3],
            emissive: false,
        })
        .into_iter()
        .collect();
    if set_mob_draw(body.id, DrawFrame::World, prims) {
        if super::TRACE && crew.presence.mark.map(|m| m.trouble) != trouble {
            log(&format!(
                "TRACE mark t={} {:?} ({}; {})",
                ctx.now,
                trouble,
                crew.note,
                crew.why.label()
            ));
        }
        crew.presence.mark = mark;
    }
}

/// Where the mark hangs, from the feet: centred over the head, or in the
/// nearest free cell when a block is there.
fn hang(ctx: &mut Ctx, body: &Body) -> [f32; 3] {
    let over = [0.0, (GOLEM_HEIGHT + MARK_CLEAR) as f32, 0.0];
    let want = [
        body.pos[0],
        body.pos[1] + GOLEM_HEIGHT + MARK_CLEAR,
        body.pos[2],
    ];
    let home = want.map(|c| c.floor() as i32);
    let mut cells = vec![home];
    for dy in -1..=2 {
        for dx in -1..=1 {
            for dz in -1..=1 {
                // Straight down is the golem's own head.
                if (dx, dz) != (0, 0) || dy > 0 {
                    cells.push([home[0] + dx, home[1] + dy, home[2] + dz]);
                }
            }
        }
    }
    let centre = |cell: [i32; 3]| cell.map(|c| f64::from(c) + 0.5);
    let far = |cell: [i32; 3]| -> f64 {
        let c = centre(cell);
        (0..3).map(|a| (c[a] - want[a]).powi(2)).sum()
    };
    cells[1..].sort_by(|a, b| far(*a).total_cmp(&far(*b)));
    let blocks = get_blocks(cells.clone());
    let free = cells.iter().zip(blocks).position(|(_, block)| {
        block
            .and_then(|block| ctx.caches.block(block))
            .is_some_and(|info| {
                info.replaceable && info.collision.is_empty() && info.fluid.is_none()
            })
    });
    match free {
        // Over the head, or nowhere better to be found.
        Some(0) | None => over,
        Some(i) => {
            let c = centre(cells[i]);
            std::array::from_fn(|a| (c[a] - body.pos[a]) as f32)
        }
    }
}
