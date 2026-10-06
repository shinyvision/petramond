//! The fight, once a tick for every skeleton near a player: who it sees, then what its hands do
//! about it. [`melee`] winds a swing up and lands it at its impact tick; [`archery`] draws, holds
//! and looses an aimed arrow; the shield's own clock is the guard's. Combat follows the brain's
//! lock, and archers retreat from nearby foes without dropping their aim.

mod archery;
mod melee;
mod movement;

use mod_sdk::*;

use super::geometry::{distance, turn_toward, yaw_toward, Upright};
use super::guard::GuardClock;
use super::kit::{Hand, Kit};
use super::presence::{self, Play};
use super::{Body, Skeletons};

pub use melee::on_player_damage;

/// Skeletons this close to a player are run.
const ENGAGE_RADIUS: f32 = 40.0;
/// An attack only starts on a foe seen this recently.
const FRESH_TICKS: u64 = 8;
/// Radians a standing skeleton turns per tick toward its foe.
const FACE_STEP: f32 = 0.35;
/// A shield comes up only against a foe this close.
const GUARD_RANGE: f64 = 10.0;
const EYE: f32 = 0.9;

#[derive(Default)]
pub struct Fight {
    target: Option<EntityRef>,
    retreat: movement::Retreat,
    seen_at: u64,
    pub(super) act: Act,
    /// The first tick a new swing or draw may start.
    ready_at: u64,
    pub guard: GuardClock,
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub(super) enum Act {
    #[default]
    Idle,
    Swing {
        impact: u64,
        until: u64,
        landed: bool,
    },
    Draw {
        release: u64,
        give_up: u64,
    },
    Loose {
        until: u64,
        clip_live: bool,
    },
}

/// The layers a body's state asks for: each hand's stance unless an action or the guard is using
/// that hand, the raised guard, the drawn bow.
pub fn wanted<'k>(kit: &'k Kit, fight: &Fight, moving: bool) -> Vec<&'k str> {
    let mut busy = [false; 2];
    match fight.act {
        Act::Swing { .. } => match kit.swing.as_ref().and_then(|s| s.hand) {
            Some(hand) => busy[hand.index()] = true,
            None => busy = [true; 2],
        },
        Act::Draw { .. }
        | Act::Loose {
            clip_live: true, ..
        } => {
            if let Some(bow) = &kit.bow {
                busy[bow.hand.index()] = true;
            }
        }
        _ => {}
    }
    let shield = kit.shield.as_ref().filter(|_| fight.guard.raised);
    if let Some(shield) = shield {
        busy[shield.hand.index()] = true;
    }
    let mut layers: Vec<&str> = Hand::BOTH
        .iter()
        .filter(|h| !busy[h.index()])
        .filter_map(|h| {
            moving
                .then(|| kit.walk_stances[h.index()].as_deref())
                .flatten()
                .or(kit.stances[h.index()].as_deref())
        })
        .collect();
    // The item's attachment stays seated while arm poses crossfade.
    layers.extend(kit.grips.iter().filter_map(|g| g.as_deref()));
    if let Some(shield) = shield {
        layers.push(&shield.guard.clip);
    }
    if let (Act::Draw { .. }, Some(bow)) = (fight.act, &kit.bow) {
        layers.push(&bow.ranged.clip_draw);
    }
    layers
}

pub fn held_display<'k>(kit: &'k Kit, fight: &Fight, now: u64) -> [Option<&'k str>; 2] {
    let mut held = kit.held.each_ref().map(|v| v.as_deref());
    if let (Act::Draw { release, .. }, Some(bow)) = (fight.act, &kit.bow) {
        let elapsed = now.saturating_sub(release.saturating_sub(u64::from(bow.ranged.draw)));
        if let Some(frame) = bow.ranged.display(elapsed) {
            held[bow.hand.index()] = Some(frame);
        }
    }
    held
}

fn upright(m: &MobSnapshot) -> Upright {
    Upright {
        feet: m.pos,
        half_width: m.half_width,
        height: m.height,
    }
}

#[derive(Clone, Copy)]
struct Foe {
    id: EntityRef,
    body: Upright,
    eye: [f64; 3],
    vel: [f32; 3],
}

impl Foe {
    fn resolve(id: EntityRef, roster: &[PlayerListEntry]) -> Option<Self> {
        match id {
            EntityRef::Player(id) => {
                let p = &roster.iter().find(|p| p.id == id)?.state;
                Some(Self {
                    id: EntityRef::Player(id),
                    body: Upright {
                        feet: p.pos,
                        half_width: p.half_width,
                        height: p.height,
                    },
                    eye: [p.pos[0], p.pos[1] + f64::from(p.eye_height), p.pos[2]],
                    vel: p.vel,
                })
            }
            EntityRef::Mob(id) => {
                let m = mob_info(id).filter(|m| m.health > 0.0)?;
                let body = upright(&m);
                Some(Self {
                    id: EntityRef::Mob(id),
                    body,
                    eye: body.at(EYE),
                    vel: m.vel,
                })
            }
        }
    }
}

/// Whether nothing collidable stands between two points.
pub fn clear_line(from: [f64; 3], to: [f64; 3]) -> bool {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let len = distance(from, to);
    if len < 1.0e-3 {
        return true;
    }
    let dir = d.map(|c| (c / len) as f32);
    let max = (len as f32).min(RAYCAST_MAX_DISTANCE);
    raycast(from, dir, max, RayFilter::Collidable).is_none_or(|hit| hit.distance >= max - 0.05)
}

/// This tick's fight: the players skeletons fight, and the skeletons near enough to them to run.
pub struct Engagement {
    roster: Vec<PlayerListEntry>,
    near: Vec<MobSnapshot>,
}

impl Engagement {
    pub fn now(kind: MobId) -> Engagement {
        let roster: Vec<PlayerListEntry> = players()
            .into_iter()
            .filter(|p| !p.state.spectator && p.state.health > 0)
            .collect();
        let anchors: Vec<[f64; 3]> = roster.iter().map(|p| p.state.pos).collect();
        let near = if anchors.is_empty() {
            Vec::new()
        } else {
            mobs_near_any_of(&anchors, ENGAGE_RADIUS, &[kind])
        };
        Engagement { roster, near }
    }

    /// The engaged skeletons missing from `bodies`.
    pub fn strangers(&self, bodies: &FxHashMap<u64, Body>) -> Vec<u64> {
        self.near
            .iter()
            .map(|m| m.id)
            .filter(|id| !bodies.contains_key(id))
            .collect()
    }
}

/// Runs every engaged skeleton this session knows; settles the rest at rest.
pub fn tick(sk: &mut Skeletons, engagement: &Engagement) {
    let now = current_tick();
    let Engagement { roster, near } = engagement;
    let nearby_ids: FxHashSet<u64> = near.iter().map(|m| m.id).collect();
    let mut drives = Vec::new();
    let mut anims = Vec::new();
    let Skeletons {
        kits,
        bodies,
        shoves,
        ..
    } = sk;
    for me in near {
        let Some(body) = bodies.get_mut(&me.id) else {
            continue;
        };
        let kit = kits.kit(body.loadout);
        let mut turn = Turn {
            now,
            me,
            kit,
            hold: false,
            velocity: None,
            retreating: false,
            foe: None,
            plays: Vec::new(),
        };
        turn.run(body, roster, shoves);
        if turn.hold {
            let yaw = turn
                .foe
                .and_then(|f| yaw_toward(me.pos, f.body.feet))
                .map(|want| turn_toward(me.yaw, want, FACE_STEP));
            let mut drive = MobDriveData::horizontal(me.id, turn.velocity.unwrap_or([0.0; 2]), yaw);
            drive.gait = turn.velocity.is_some();
            drives.push(drive);
        }
        let plays = std::mem::take(&mut turn.plays);
        if let Some([main, off]) = body.presence.display(held_display(kit, &body.fight, now)) {
            mob_held_display(me.id, main, off);
        }
        body.presence.frame(
            me.id,
            &wanted(kit, &body.fight, me.moving),
            &plays,
            &mut anims,
        );
    }
    for (id, body) in bodies.iter_mut() {
        if !nearby_ids.contains(id) {
            body.fight = Fight::default();
            let kit = kits.kit(body.loadout);
            if let Some([main, off]) = body.presence.display(held_display(kit, &body.fight, now)) {
                mob_held_display(*id, main, off);
            }
            // Distant guards carry loosely, leaving their return walk free to move the arms.
            body.presence
                .frame(*id, &wanted(kit, &body.fight, true), &[], &mut anims);
        }
    }
    paged(drives, mob_drive_many);
    presence::send(anims);
}

/// One skeleton's tick.
struct Turn<'a> {
    now: u64,
    me: &'a MobSnapshot,
    kit: &'a Kit,
    hold: bool,
    velocity: Option<[f32; 2]>,
    retreating: bool,
    foe: Option<Foe>,
    plays: Vec<Play<'a>>,
}

impl Turn<'_> {
    fn run(
        &mut self,
        body: &mut Body,
        roster: &[PlayerListEntry],
        shoves: &mut Vec<(PlayerId, [f32; 3])>,
    ) {
        let foe = self.look(&mut body.fight, roster);
        self.foe = foe;
        if let Some(foe) = foe.filter(|_| self.kit.bow.is_some()) {
            self.velocity = self.retreat(&mut body.fight.retreat, &foe);
            self.retreating = self.velocity.is_some();
            // Hold the chase shut when no safe retreat exists.
            if distance(self.me.pos, foe.body.feet) <= movement::KEEP_DISTANCE {
                self.hold = true;
            }
            if !self.retreating
                && self.in_melee_reach(&foe)
                && matches!(body.fight.act, Act::Draw { .. } | Act::Loose { .. })
            {
                self.cancel_act(&mut body.fight);
            }
        }
        self.guard(&mut body.fight, foe.as_ref());
        match body.fight.act {
            Act::Idle => {
                if let Some(foe) = foe {
                    self.start(&mut body.fight, &foe);
                }
            }
            Act::Swing { .. } => self.swinging(&mut body.fight, foe.as_ref(), shoves),
            Act::Draw { .. } => self.drawing(&mut body.fight, foe.as_ref()),
            Act::Loose { .. } => self.resting(&mut body.fight),
        }
        let archer_in_band = foe.is_some_and(|f| self.within_band(&f))
            && matches!(body.fight.act, Act::Idle | Act::Loose { .. });
        if archer_in_band || (body.watch && foe.is_some()) {
            self.hold = true;
        }
    }

    fn eye(&self) -> [f64; 3] {
        upright(self.me).at(EYE)
    }

    fn cancel_act(&self, fight: &mut Fight) {
        match fight.act {
            Act::Swing { .. } => {
                if let Some(swing) = &self.kit.swing {
                    self.stop(&swing.melee.clip);
                }
            }
            Act::Draw { .. } | Act::Loose { .. } => {
                if let Some(bow) = &self.kit.bow {
                    self.stop(&bow.ranged.clip_draw);
                    self.stop(&bow.ranged.clip_loose);
                }
            }
            Act::Idle => {}
        }
        fight.act = Act::Idle;
    }

    fn look(&self, fight: &mut Fight, roster: &[PlayerListEntry]) -> Option<Foe> {
        if fight.target != self.me.target {
            self.cancel_act(fight);
            fight.target = self.me.target;
            fight.seen_at = self.now.saturating_sub(FRESH_TICKS + 1);
            fight.retreat = movement::Retreat::default();
        }
        let foe = Foe::resolve(self.me.target?, roster)?;
        if clear_line(self.eye(), foe.eye) {
            fight.seen_at = self.now;
        }
        Some(foe)
    }

    fn guard(&self, fight: &mut Fight, foe: Option<&Foe>) {
        let Some(shield) = &self.kit.shield else {
            return;
        };
        let engaged = foe.is_some_and(|f| distance(self.me.pos, f.body.feet) <= GUARD_RANGE);
        let swinging = matches!(fight.act, Act::Swing { .. });
        let id = self.me.id;
        let roll = || splitmix64_mix(rng_u64("skeleton_guard") ^ id);
        fight
            .guard
            .cycle(self.now, &shield.guard, engaged, swinging, roll);
        if fight
            .guard
            .recoil_until
            .is_some_and(|until| self.now >= until)
        {
            fight.guard.recoil_until = None;
            self.stop(&shield.guard.impact_clip);
        }
    }

    fn start(&mut self, fight: &mut Fight, foe: &Foe) {
        if self.now.saturating_sub(fight.seen_at) > FRESH_TICKS || self.now < fight.ready_at {
            return;
        }
        if self.retreating || !self.start_swing(fight, foe) {
            self.start_draw(fight, foe);
        }
    }

    fn playing(&self, clip: &str) -> bool {
        mob_anim_state(self.me.id, clip).is_some()
    }

    fn stop(&self, clip: &str) {
        mob_anim_set(self.me.id, clip, false);
    }
}

#[cfg(test)]
mod tests;
