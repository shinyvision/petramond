use super::rows::{BowRow, Rows};
use crate::projectile_aim::{crosshair_reach, from_hand};
use mod_sdk::*;

pub fn loose(rows: &Rows, bow: &BowRow, me: PlayerId, state: &PlayerSnapshot, ticks: u32) {
    let Some((arrow, stack)) = rows
        .arrows
        .iter()
        .find_map(|arrow| take_item(me, &arrow.name, 1, None).map(|stack| (arrow, stack)))
    else {
        return;
    };
    let (from, dir) = from_hand(state, crosshair_reach(me, state));
    let speed = bow.draw.launch_speed(ticks);
    let vel = [
        dir[0] * speed + state.vel[0],
        dir[1] * speed + state.vel[1],
        dir[2] * speed + state.vel[2],
    ];
    let data: Vec<(&str, &[u8])> = stack
        .data
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    launch_item(&arrow.name, from, vel, Some(EntityRef::Player(me)), &data);
}
