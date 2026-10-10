//! The painting table's server half: a client asks to paint the flag in the table it has open,
//! and the table repaints the stack in its slot.

use mod_sdk::{container_get, container_set, gui_viewers, PlayerId};

use crate::keys;
use crate::painting::design::Design;
use crate::painting::paintable::Plans;

/// The table's one slot.
const FLAG_SLOT: u32 = 0;

/// Paints the flag in the table `player` has open with `paint`. The request is the client's
/// word alone, so anything but a design for exactly that flag's canvas is ignored.
pub(crate) fn paint(plans: &Plans, player: PlayerId, paint: &[u8]) {
    let viewing = gui_viewers()
        .into_iter()
        .find(|viewer| viewer.player_id == player && viewer.kind == keys::TABLE_GUI);
    let Some(at) = viewing.and_then(|viewer| viewer.anchor) else {
        return;
    };
    let Some(stack) =
        container_get(at).and_then(|slots| slots.into_iter().nth(FLAG_SLOT as usize)?)
    else {
        return;
    };
    let Some(plan) = plans.of(&stack.item) else {
        return;
    };
    let Some(design) = Design::from_paint(plan.canvas, paint) else {
        return;
    };
    let painted = plan.painted(&stack, &design);
    if painted != stack {
        container_set(at, vec![(FLAG_SLOT, Some(painted))]);
    }
}
