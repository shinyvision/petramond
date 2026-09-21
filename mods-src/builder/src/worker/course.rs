//! A pillar's course: everything the golem walks to from the top and back
//! again (a roof course, a wall top, a porch roof). A perched golem found
//! off it has fallen or been knocked down, and its pillar is no way anywhere.

use super::{route, Ctx};

/// The footholds walked to from `top` and back. `None` = no route budget
/// this tick; `Some(None)` = no flood answers it.
pub fn around(ctx: &mut Ctx, top: [i32; 3]) -> Option<Option<Vec<[i32; 3]>>> {
    let Some(out) = route::region(ctx, top, false, &[])? else {
        return Some(None);
    };
    let out: Vec<[i32; 3]> = out.cells().collect();
    let Some(back) = route::region(ctx, top, true, &[])? else {
        return Some(None);
    };
    Some(Some(
        out.into_iter().filter(|c| back.contains(*c)).collect(),
    ))
}
