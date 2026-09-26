//! Which of a tick window's world-anchored events each recipient perceives.
//!
//! Every [`WorldEventMsg`] declares its [`Reach`]: a cell edit's presentation
//! (break burst, door swing, chest lid) reaches the recipients that hold the
//! cell's section — exactly the block-delta filter — a sound reaches the
//! listeners within its row's `attenuation_distance` (where the audio engine
//! fades it to silence), a burst the recipients whose view reaches it, and
//! only the non-positional events (a UI sound, a handle's retune or stop)
//! reach everybody. Event traffic therefore grows with what each player can
//! perceive, not with the whole server's activity.
//!
//! Looping spatial sounds carry STATE, so their delivery is per recipient
//! too: each connection remembers the loops it currently hears
//! ([`EntityInterest::loops`](super::interest::EntityInterest)); a loop coming
//! within earshot is (re)started for it, one moving out of earshot (past a
//! hysteresis band) is stopped for it alone. A joining session starts with an
//! empty set, so its first batch replays exactly the loops it can hear.

use std::sync::OnceLock;

use rustc_hash::FxHashSet;

use crate::net::protocol::{SpatialSoundMsg, WorldEventMsg};
use petramond_math::math::IVec3;
use petramond_math::world_pos::WorldPos;

use super::spatial_loops::LiveSpatialLoops;

/// Slack on every hearing range: the listener's ears sit above the feet
/// position a recipient is measured from.
pub const HEARING_MARGIN_BLOCKS: f64 = 2.0;

/// How far past its hearing range a loop a recipient already hears keeps
/// playing for it, so a listener pacing the edge does not restart it.
pub const LOOP_HYSTERESIS_BLOCKS: f64 = 8.0;

/// Who can perceive one world event.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum Reach {
    /// Non-positional: every recipient.
    Everyone,
    /// A cell's presentation: recipients holding the cell's section.
    Cell(IVec3),
    /// A sound at `at`, silent beyond `range` blocks.
    Heard { at: WorldPos, range: f64 },
    /// A visual effect at `at`: recipients whose view reaches it.
    Seen { at: WorldPos },
    /// The play of a looping row — delivered by the per-recipient loop sync
    /// ([`sync_loops`]), never from the window's stream.
    Loop,
}

/// The travel distance of sound row `sound_id`.
fn sound_range(sound_id: u8) -> f64 {
    f64::from(
        petramond_world::sound_registry::Sound(sound_id)
            .def()
            .attenuation_distance,
    )
}

/// The farthest any loaded sound row travels — the range of events whose
/// row the CLIENT picks (a species' hurt call, the pickup pop).
fn loudest_sound_range() -> f64 {
    static LOUDEST: OnceLock<f64> = OnceLock::new();
    *LOUDEST.get_or_init(|| {
        petramond_world::sound_registry::defs()
            .iter()
            .map(|d| f64::from(d.attenuation_distance))
            .fold(
                f64::from(petramond_world::sound_registry::DEFAULT_ATTENUATION_DISTANCE),
                f64::max,
            )
    })
}

fn is_looped(sound_id: u8) -> bool {
    petramond_world::sound_registry::Sound(sound_id)
        .def()
        .looped
}

/// Who can perceive `ev`. Exhaustive: a new event kind states its reach here.
pub(super) fn reach(ev: &WorldEventMsg) -> Reach {
    match *ev {
        WorldEventMsg::BlockBroken { pos, .. } | WorldEventMsg::BlockPlaced { pos, .. } => {
            Reach::Cell(pos)
        }
        WorldEventMsg::PanelToggled { anchor, .. } => Reach::Cell(anchor),
        WorldEventMsg::ChestOpened { pos } | WorldEventMsg::ChestClosed { pos } => {
            Reach::Cell(pos)
        }
        WorldEventMsg::ItemPickedUp { pos, .. } | WorldEventMsg::MobSound { pos, .. } => {
            Reach::Heard {
                at: pos,
                range: loudest_sound_range(),
            }
        }
        WorldEventMsg::Sound { sound_id, pos } => match pos {
            Some(at) => Reach::Heard {
                at,
                range: sound_range(sound_id),
            },
            None => Reach::Everyone,
        },
        WorldEventMsg::EmitterBurst { pos, .. } => Reach::Seen { at: pos },
        WorldEventMsg::SpatialSound(cmd) => match cmd {
            SpatialSoundMsg::PlayAt { sound_id, .. } | SpatialSoundMsg::PlayOnMob { sound_id, .. }
                if is_looped(sound_id) =>
            {
                Reach::Loop
            }
            SpatialSoundMsg::PlayAt { sound_id, pos, .. } => Reach::Heard {
                at: pos,
                range: sound_range(sound_id),
            },
            SpatialSoundMsg::PlayOnMob {
                sound_id, last_pos, ..
            } => Reach::Heard {
                at: last_pos,
                range: sound_range(sound_id),
            },
            // Handle-addressed commands are tiny and a stop for a handle the
            // recipient never heard is a no-op: they reach everyone, so no
            // recipient can miss the end of something it heard start.
            SpatialSoundMsg::Set { .. } | SpatialSoundMsg::Stop { .. } => Reach::Everyone,
        },
    }
}

fn distance_sq(a: WorldPos, b: WorldPos) -> f64 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)
}

/// One recipient, as the event filter sees it.
pub(super) struct Viewer<'a> {
    /// Where the recipient's player stands.
    pub pos: WorldPos,
    /// How far (horizontally) its view reaches, in blocks.
    pub view_blocks: f64,
    /// Whether it holds the section of a cell.
    pub holds_cell: &'a dyn Fn(IVec3) -> bool,
}

impl Viewer<'_> {
    fn hears(&self, at: WorldPos, range: f64) -> bool {
        distance_sq(self.pos, at) <= (range + HEARING_MARGIN_BLOCKS).powi(2)
    }

    /// Whether this recipient perceives an event of reach `reach`.
    pub(super) fn perceives(&self, reach: Reach) -> bool {
        match reach {
            Reach::Everyone => true,
            Reach::Cell(pos) => (self.holds_cell)(pos),
            Reach::Heard { at, range } => self.hears(at, range),
            Reach::Seen { at } => {
                (at.x - self.pos.x).powi(2) + (at.z - self.pos.z).powi(2)
                    <= self.view_blocks.powi(2)
            }
            Reach::Loop => false,
        }
    }
}

/// One loop still playing this window, placed where it sounds from.
pub(super) struct LiveLoop {
    pub handle: u64,
    /// The command that (re)starts it, retunes folded in.
    pub cmd: SpatialSoundMsg,
    pub at: WorldPos,
    pub range: f64,
}

/// Place every live loop for this window: a pinned loop sounds from its mob's
/// current position (`mob_pos`), or where it was started if the mob is gone.
pub(super) fn live_loops(
    live: &LiveSpatialLoops,
    mob_pos: impl Fn(u64) -> Option<WorldPos>,
) -> Vec<LiveLoop> {
    live.iter()
        .filter_map(|(&handle, &cmd)| {
            let (at, sound_id) = match cmd {
                SpatialSoundMsg::PlayAt { sound_id, pos, .. } => (pos, sound_id),
                SpatialSoundMsg::PlayOnMob {
                    sound_id,
                    mob_id,
                    last_pos,
                    ..
                } => (mob_pos(mob_id).unwrap_or(last_pos), sound_id),
                SpatialSoundMsg::Set { .. } | SpatialSoundMsg::Stop { .. } => return None,
            };
            // A loop can enter a listener's range long after it first
            // started. Its replay needs the mob's current fallback position
            // in case this listener has no replica row for that mob.
            let mut replay = cmd;
            if let SpatialSoundMsg::PlayOnMob { last_pos, .. } = &mut replay {
                *last_pos = at;
            }
            Some(LiveLoop {
                handle,
                cmd: replay,
                at,
                range: sound_range(sound_id),
            })
        })
        .collect()
}

/// Bring one recipient's heard-loop set up to this window: start (as their
/// restart command) the loops that came within earshot, stop the ones that
/// left it past the hysteresis band, and forget handles no longer live (their
/// stop rides the window to everyone). Returns `(starts, stops)`.
pub(super) fn sync_loops(
    viewer: &Viewer,
    heard: &mut FxHashSet<u64>,
    loops: &[LiveLoop],
) -> (Vec<WorldEventMsg>, Vec<WorldEventMsg>) {
    let (mut starts, mut stops) = (Vec::new(), Vec::new());
    for l in loops {
        let hearing = heard.contains(&l.handle);
        let slack = if hearing { LOOP_HYSTERESIS_BLOCKS } else { 0.0 };
        let audible = viewer.hears(l.at, l.range + slack);
        if audible && !hearing {
            heard.insert(l.handle);
            starts.push(WorldEventMsg::SpatialSound(l.cmd));
        } else if !audible && hearing {
            heard.remove(&l.handle);
            stops.push(WorldEventMsg::SpatialSound(SpatialSoundMsg::Stop {
                handle: l.handle,
            }));
        }
    }
    heard.retain(|h| loops.iter().any(|l| l.handle == *h));
    (starts, stops)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer<'a>(x: f64, holds_cell: &'a dyn Fn(IVec3) -> bool) -> Viewer<'a> {
        Viewer {
            pos: WorldPos::new(x, 64.0, 0.0),
            view_blocks: 128.0,
            holds_cell,
        }
    }

    fn range_of_row_zero() -> f64 {
        sound_range(0)
    }

    #[test]
    fn positional_events_reach_only_recipients_that_can_perceive_them() {
        let near_cell = |p: IVec3| p.x < 100;
        let no_cells = |_: IVec3| false;
        let near = viewer(0.0, &near_cell);
        let far = viewer(5000.0, &no_cells);

        let broke = WorldEventMsg::BlockBroken {
            pos: IVec3::new(1, 64, 1),
            block_id: 1,
            normal: None,
            tint: None,
        };
        assert!(near.perceives(reach(&broke)), "the holder of the cell sees the burst");
        assert!(!far.perceives(reach(&broke)), "a recipient without the section does not");

        let range = range_of_row_zero();
        let sound = |x: f64| WorldEventMsg::Sound {
            sound_id: 0,
            pos: Some(WorldPos::new(x, 64.0, 0.0)),
        };
        assert!(near.perceives(reach(&sound(range))), "at the edge of its range");
        assert!(
            !near.perceives(reach(&sound(range + HEARING_MARGIN_BLOCKS + 1.0))),
            "past where the audio fades to silence"
        );
        let ui = WorldEventMsg::Sound {
            sound_id: 0,
            pos: None,
        };
        assert!(far.perceives(reach(&ui)), "a non-spatial sound reaches everyone");
        let stop = WorldEventMsg::SpatialSound(SpatialSoundMsg::Stop { handle: 3 });
        assert!(far.perceives(reach(&stop)), "handle commands reach everyone");

        let burst = WorldEventMsg::EmitterBurst {
            emitter_id: 0,
            pos: WorldPos::new(120.0, 64.0, 0.0),
            intensity: 1.0,
            direction: None,
            texture: None,
        };
        assert!(near.perceives(reach(&burst)), "within the view radius");
        assert!(!far.perceives(reach(&burst)));
    }

    #[test]
    fn a_mob_pinned_sound_uses_its_emission_position() {
        let at = WorldPos::new(1.0, 64.0, 0.0);
        let pinned = WorldEventMsg::SpatialSound(SpatialSoundMsg::PlayOnMob {
            handle: 9,
            sound_id: 0,
            mob_id: 42,
            volume: 1.0,
            pitch: 1.0,
            last_pos: at,
        });
        let cells = |_: IVec3| true;
        assert!(viewer(0.0, &cells).perceives(reach(&pinned)));
        assert!(!viewer(5000.0, &cells).perceives(reach(&pinned)));
    }

    #[test]
    fn entering_a_moving_mob_loops_replays_its_current_position() {
        let mut live = LiveSpatialLoops::new();
        live.insert(
            9,
            SpatialSoundMsg::PlayOnMob {
                handle: 9,
                sound_id: 0,
                mob_id: 42,
                volume: 1.0,
                pitch: 1.0,
                last_pos: WorldPos::ZERO,
            },
        );
        let now = WorldPos::new(32.0, 64.0, 0.0);
        let loops = live_loops(&live, |mob| (mob == 42).then_some(now));
        let [loop_row] = loops.as_slice() else {
            panic!("one live loop")
        };
        assert_eq!(loop_row.at, now);
        assert!(matches!(
            loop_row.cmd,
            SpatialSoundMsg::PlayOnMob { last_pos, .. } if last_pos == now
        ));
    }

    #[test]
    fn loops_start_within_earshot_and_stop_past_the_band() {
        let range = range_of_row_zero();
        let play = SpatialSoundMsg::PlayAt {
            handle: 7,
            sound_id: 0,
            pos: WorldPos::ZERO,
            volume: 1.0,
            pitch: 1.0,
        };
        let loops = |x: f64| {
            vec![LiveLoop {
                handle: 7,
                cmd: play,
                at: WorldPos::new(x, 64.0, 0.0),
                range,
            }]
        };
        let cells = |_: IVec3| true;
        let listener = viewer(0.0, &cells);
        let mut heard = FxHashSet::default();

        let (starts, stops) = sync_loops(&listener, &mut heard, &loops(range + 100.0));
        assert!(starts.is_empty() && stops.is_empty(), "out of earshot: nothing");

        let (starts, _) = sync_loops(&listener, &mut heard, &loops(1.0));
        assert_eq!(starts, vec![WorldEventMsg::SpatialSound(play)], "entering earshot starts it");
        let (starts, stops) = sync_loops(&listener, &mut heard, &loops(1.0));
        assert!(starts.is_empty() && stops.is_empty(), "a heard loop is not restarted");

        let band = range + HEARING_MARGIN_BLOCKS + LOOP_HYSTERESIS_BLOCKS / 2.0;
        let (_, stops) = sync_loops(&listener, &mut heard, &loops(band));
        assert!(stops.is_empty(), "inside the hysteresis band it keeps playing");

        let gone = range + HEARING_MARGIN_BLOCKS + LOOP_HYSTERESIS_BLOCKS + 1.0;
        let (_, stops) = sync_loops(&listener, &mut heard, &loops(gone));
        assert_eq!(
            stops,
            vec![WorldEventMsg::SpatialSound(SpatialSoundMsg::Stop { handle: 7 })],
            "leaving earshot stops it for this recipient"
        );

        sync_loops(&listener, &mut heard, &loops(1.0));
        sync_loops(&listener, &mut heard, &[]);
        assert!(heard.is_empty(), "a loop no longer live is forgotten");
    }
}
