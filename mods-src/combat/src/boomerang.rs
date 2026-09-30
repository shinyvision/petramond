mod flight;
pub(crate) mod motion;
mod route;
mod rows;
#[cfg(test)]
mod tests;

use crate::body::BodyClocks;
use crate::claims::{Body, Claims, Rule};
use crate::strike;
use mod_sdk::*;
use motion::{Clock, Pose};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use flight::Flight;
use rows::Row;

pub struct Boomerangs {
    rows: Vec<Row>,
    flights: RefCell<BTreeMap<u64, Flight>>,
}

impl Boomerangs {
    pub fn load() -> Option<Self> {
        let rows = rows::load();
        (!rows.is_empty()).then(|| Self {
            rows,
            flights: RefCell::default(),
        })
    }

    fn held(&self, state: &PlayerSnapshot) -> Option<&Row> {
        self.rows
            .iter()
            .find(|r| Some(r.id) == state.held)
            .filter(|_| !state.spectator && state.health > 0)
    }

    fn launch(&self, row: &Row, player: PlayerId, state: &PlayerSnapshot, ticks: u32) {
        let Some(held) = player_held(player).filter(|s| s.item == row.name) else {
            return;
        };
        let data: Vec<(&str, &[u8])> = held
            .data
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect();
        if !consume_held_by(player, row.id, 1) {
            return;
        }
        let (from, direction) = crate::projectile_aim::from_hand(
            state,
            crate::projectile_aim::crosshair_reach(player, state),
        );
        let home = [
            state.pos[0],
            state.pos[1] + f64::from(state.height * 0.7),
            state.pos[2],
        ];
        let flight = Flight::new(player, row.clone(), direction, home, ticks);
        let vel = flight.outbound_velocity();
        let id = launch_item(
            &held.item,
            from,
            vel,
            Some(EntityRef::Player(player)),
            &data,
        );
        if id == 0 {
            give_item_to(player, &held.item, 1, &data);
            return;
        }
        steer_item(id, Some(vel));
        self.flights.borrow_mut().insert(id, flight);
    }

    pub fn step(&self, roster: &[PlayerListEntry]) {
        self.flights.borrow_mut().retain(|&id, flight| {
            let Some(item) = item_entity(id).filter(|it| it.motion == ItemMotion::Flight) else {
                return false;
            };
            let Some(_) = roster
                .iter()
                .find(|p| p.id == flight.owner)
                .filter(|p| !p.state.spectator && p.state.health > 0)
            else {
                steer_item(id, None);
                return false;
            };
            let Some(vel) = flight.step(item.pos) else {
                steer_item(id, None);
                return false;
            };
            steer_item(id, Some(vel))
        });
    }

    pub fn on_hit(
        &self,
        id: u64,
        target: &ProjectileTarget,
        pos: [f64; 3],
        vel: [f32; 3],
        fate: &mut ProjectileFate,
        mut blocks: impl FnMut(PlayerId, [f64; 3]) -> bool,
    ) -> bool {
        let mut flights = self.flights.borrow_mut();
        let Some(flight) = flights.get_mut(&id) else {
            return false;
        };
        match target {
            ProjectileTarget::Block { .. } => {
                *fate = ProjectileFate::Drop;
                flights.remove(&id);
            }
            ProjectileTarget::Player(player) if *player == flight.owner => {
                if let Some(stack) = take_item_entity(id) {
                    let data: Vec<(&str, &[u8])> = stack
                        .data
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.as_slice()))
                        .collect();
                    give_item_to(*player, &stack.item, stack.count, &data);
                }
                *fate = ProjectileFate::Consume;
                flights.remove(&id);
            }
            target => {
                let body = match target {
                    ProjectileTarget::Mob(id) => EntityRef::Mob(*id),
                    ProjectileTarget::Player(id) => EntityRef::Player(*id),
                    ProjectileTarget::Block { .. } => unreachable!(),
                };
                let origin = std::array::from_fn(|axis| pos[axis] - f64::from(vel[axis]));
                let fresh = flight.strike(body);
                let blocked = match body {
                    EntityRef::Player(player) if fresh => blocks(player, origin),
                    EntityRef::Player(_) => false,
                    EntityRef::Mob(_) => false,
                };
                if fresh && !blocked {
                    let damage = strike::roll(flight.damage(), rng_u64("boomerang"));
                    let attacker = Some(EntityRef::Player(flight.owner));
                    match body {
                        EntityRef::Mob(mob) => {
                            damage_mob(mob, damage, Some(pos), attacker);
                        }
                        EntityRef::Player(player) => {
                            damage_player(player, damage.round() as i32, Some(pos), attacker);
                        }
                    }
                }
                *fate = ProjectileFate::Deflect {
                    vel: flight.reverse(vel),
                };
            }
        }
        true
    }
}

pub struct ThrowRule {
    boomerangs: Rc<Boomerangs>,
}

impl ThrowRule {
    pub fn new(boomerangs: Rc<Boomerangs>) -> Self {
        Self { boomerangs }
    }
}

impl Rule for ThrowRule {
    fn takes_press(&self, state: &PlayerSnapshot) -> bool {
        self.boomerangs.held(state).is_some()
    }

    fn step(
        &self,
        clocks: &mut BodyClocks,
        player: PlayerId,
        state: &PlayerSnapshot,
        press: bool,
        dt_ticks: f32,
        authority: bool,
    ) {
        let row = clocks
            .throwing
            .item()
            .and_then(|id| self.boomerangs.rows.iter().find(|r| r.id == id))
            .or_else(|| self.boomerangs.held(state));
        if let (Some(ticks), Some(row), true) = (
            clocks.throwing.step(
                row,
                state.held,
                !state.spectator && state.health > 0,
                press,
                dt_ticks,
            ),
            row,
            authority,
        ) {
            self.boomerangs.launch(row, player, state, ticks);
        }
    }

    fn claims(&self, body: &Body) -> Claims {
        let row = body
            .clocks
            .throwing
            .item()
            .and_then(|id| self.boomerangs.rows.iter().find(|r| r.id == id));
        throw_claims(row, &body.clocks.throwing)
    }
}

fn throw_claims(row: Option<&Row>, clock: &Clock) -> Claims {
    let (Some(row), Some(pose)) = (row, clock.pose()) else {
        return Claims::default();
    };
    let mut params = Vec::new();
    let times = match pose {
        Pose::Draw(ticks) => [Some((ticks / row.draw.full_ticks as f32).clamp(0.0, 1.0)); 2],
        Pose::Throw(elapsed) => row.motion.throw_times(elapsed),
    };
    if matches!(pose, Pose::Throw(elapsed) if elapsed >= row.motion.release_time()) {
        params.extend(
            [rig::PLAYER_FIRST_PERSON, rig::PLAYER_BODY]
                .into_iter()
                .map(|rig| AnimatorParam {
                    rig: rig.into(),
                    param: "main.item_visible".into(),
                    value: AnimatorValue::Number(0.0),
                }),
        );
    }
    let plays = [rig::PLAYER_FIRST_PERSON, rig::PLAYER_BODY]
        .into_iter()
        .zip([&row.motion.first_person, &row.motion.body])
        .zip(times)
        .filter_map(|((rig, clips), progress)| {
            let progress = progress?;
            for param in ["main.swing_claim", "main.jab_claim"] {
                params.push(AnimatorParam {
                    rig: rig.into(),
                    param: param.into(),
                    value: AnimatorValue::Number(1.0),
                });
            }
            if rig == rig::PLAYER_FIRST_PERSON {
                params.push(AnimatorParam {
                    rig: rig.into(),
                    param: "combat.boomerang_claim".into(),
                    value: AnimatorValue::Number(1.0),
                });
            }
            let clip = &clips[usize::from(matches!(pose, Pose::Throw(_)))];
            Some(AnimatorPlay::scrubbed(rig, "main_claim", clip, progress))
        })
        .collect();
    Claims {
        holds_press: true,
        speed: if matches!(pose, Pose::Draw(_)) {
            row.draw.speed_scale
        } else {
            1.0
        },
        denied: vec![BodyAction::Attack, BodyAction::Mine],
        params,
        plays,
        ..Default::default()
    }
}
