//! A skeleton's animation layers. Layers add up, so a clip that moves a hand replaces the stance
//! of that hand while it plays. Held layers (stances, a raised guard, a drawn bow) are kept in
//! step with what the fight says each tick; one-shot clips are started over and left to retire.

use mod_sdk::*;

#[derive(Default)]
pub struct Presence {
    held: Vec<String>,
    display: Option<[Option<String>; 2]>,
}

/// A clip to play from its start this tick; `hold_at` freezes it at that phase (seconds).
pub struct Play<'a> {
    pub clip: &'a str,
    pub hold_at: Option<f32>,
}

impl Presence {
    pub fn display(&mut self, wanted: [Option<&str>; 2]) -> Option<[Option<String>; 2]> {
        if self
            .display
            .as_ref()
            .is_some_and(|old| old.each_ref().map(|v| v.as_deref()) == wanted)
        {
            return None;
        }
        let names = wanted.map(|v| v.map(str::to_owned));
        self.display = Some(names.clone());
        Some(names)
    }

    /// Brings the held layers to `wanted`, then starts `plays`. Deactivations go out before
    /// activations so the engine's layer cap never refuses a swap.
    pub fn frame(
        &mut self,
        id: u64,
        wanted: &[&str],
        plays: &[Play<'_>],
        ops: &mut Vec<MobAnimOp>,
    ) {
        let set = |anim: &str, active: bool| MobAnimOp::Set {
            mob_id: id,
            anim: anim.to_owned(),
            active,
        };
        let playing = |clip: &str| plays.iter().any(|p| p.clip == clip);
        self.held.retain(|clip| {
            let keep = wanted.contains(&clip.as_str()) && !playing(clip);
            if !keep {
                ops.push(set(clip, false));
            }
            keep
        });
        for play in plays {
            ops.push(set(play.clip, false));
            ops.push(set(play.clip, true));
            if let Some(phase) = play.hold_at {
                ops.push(MobAnimOp::Seek {
                    mob_id: id,
                    anim: play.clip.to_owned(),
                    phase,
                    rate: 1.0,
                });
                self.held.push(play.clip.to_owned());
            }
        }
        for clip in wanted {
            if !self.held.iter().any(|h| h == clip) {
                ops.push(set(clip, true));
                self.held.push((*clip).to_owned());
            }
        }
    }
}

pub fn send(ops: Vec<MobAnimOp>) {
    paged(ops, mob_anim_many);
}
