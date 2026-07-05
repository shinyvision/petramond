//! Idle animations: occasionally plays one of a model's `idle_*` animations while the
//! mob is standing around.
//!
//! When the mob is idle and its species has idle animations, there's a small per-tick
//! chance to start a random one, which plays for a while, then a cooldown before the
//! next. Navigating (or a species with no idle animations) keeps it off. The renderer
//! maps the chosen index to the model's `idle_*` animation, and — when that animation
//! moves the head — overrides head-look for its duration.

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput};

/// Per-tick chance to start an idle animation when idle and not already playing one
/// (~1/120 ≈ once every several seconds of standing around).
const PLAY_CHANCE: f32 = 1.0 / 120.0;
/// How long a *looping* idle animation plays before stopping (ticks; ~2–5 s at 20
/// TPS). One-shot (non-looping) idles instead play for exactly their own length.
const PLAY_MIN_TICKS: u32 = 40;
const PLAY_SPAN_TICKS: u32 = 60;
/// Quiet gap after an idle animation ends before another can start.
const COOLDOWN_TICKS: u32 = 30;
/// Game ticks per second — converts a one-shot animation's length to a play duration.
/// Matches the fixed simulation tick.
const TICKS_PER_SECOND: f32 = 20.0;

pub struct IdleAnimAi {
    /// The idle animation index currently playing, if any.
    playing: Option<u8>,
    /// Ticks left in the current play / cooldown.
    timer: u32,
}

impl IdleAnimAi {
    pub fn new() -> Self {
        IdleAnimAi {
            playing: None,
            timer: 0,
        }
    }
}

impl AiBehavior for IdleAnimAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        // Only while standing still on land, and only if the species has idle
        // animations. A mob in water is busy swimming — it plays no idle animation
        // (though head-look still runs).
        if !ctx.nav_idle || ctx.in_water || ctx.idle_anims.is_empty() {
            self.playing = None;
            self.timer = 0;
            return BehaviorOutput::default();
        }

        if self.timer > 0 {
            self.timer -= 1;
        } else if self.playing.is_some() {
            // The animation just finished — stop and start a cooldown.
            self.playing = None;
            self.timer = COOLDOWN_TICKS;
        } else if ctx.rng.next_f32() < PLAY_CHANCE {
            // Start a random idle animation. A looping one plays for a random while; a
            // one-shot plays for exactly its length (so it isn't cut off or looped).
            let index = ctx.rng.next_range(0, ctx.idle_anims.len() as i32 - 1) as usize;
            let meta = ctx.idle_anims[index];
            self.playing = Some(index as u8);
            self.timer = if meta.looping {
                PLAY_MIN_TICKS + (ctx.rng.next_f32() * PLAY_SPAN_TICKS as f32) as u32
            } else {
                ((meta.length * TICKS_PER_SECOND).ceil() as u32).max(1)
            };
        }

        BehaviorOutput {
            idle_anim: self.playing,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mathh::{IVec3, Vec3};
    use crate::mob::brain::AiCtx;
    use crate::mob::model_meta::IdleAnimMeta;
    use crate::mob::MobRng;
    use crate::world::World;

    /// Run `ai` until it starts an idle animation, returning the play-timer it set.
    fn ticks_until_start(ai: &mut IdleAnimAi, idle: &[IdleAnimMeta]) -> Option<u32> {
        let world = World::new(0, 1);
        let mut rng = MobRng::new(7);
        for _ in 0..20_000 {
            let out = {
                let mut ctx = AiCtx {
                    mob_id: 1,
                    pos: Vec3::ZERO,
                    cell: IVec3::ZERO,
                    yaw: 0.0,
                    head_height: 0.7,
                    half_width: 0.25,
                    world: &world,
                    player_pos: Vec3::ZERO,
                    nav_idle: true,
                    in_water: false,
                    head: 1,
                    idle_anims: idle,
                    mob_index: 0,
                    mobs: &[],
                    rng: &mut rng,
                };
                ai.tick(&mut ctx)
            };
            if out.idle_anim.is_some() {
                return Some(ai.timer);
            }
        }
        None
    }

    #[test]
    fn one_shot_idle_plays_for_exactly_its_length() {
        // A 1.0 s one-shot at 20 TPS -> a 20-tick play (not the looping 40–100).
        let idle = [IdleAnimMeta {
            length: 1.0,
            looping: false,
        }];
        let timer = ticks_until_start(&mut IdleAnimAi::new(), &idle).expect("idle starts");
        assert_eq!(timer, 20, "one-shot plays for its length in ticks");
    }

    #[test]
    fn looping_idle_plays_for_a_longer_random_while() {
        // A looping idle ignores its (short) length and plays for the random window.
        let idle = [IdleAnimMeta {
            length: 0.1,
            looping: true,
        }];
        let timer = ticks_until_start(&mut IdleAnimAi::new(), &idle).expect("idle starts");
        assert!(
            timer >= PLAY_MIN_TICKS,
            "looping idle uses the random play window: {timer}"
        );
    }

    #[test]
    fn no_idle_animations_means_never_plays() {
        assert_eq!(ticks_until_start(&mut IdleAnimAi::new(), &[]), None);
    }

    #[test]
    fn never_plays_an_idle_animation_while_in_water() {
        let world = World::new(0, 1);
        let mut rng = MobRng::new(7);
        let idle = [IdleAnimMeta {
            length: 1.0,
            looping: false,
        }];
        let mut ai = IdleAnimAi::new();
        for _ in 0..20_000 {
            let mut ctx = AiCtx {
                mob_id: 1,
                pos: Vec3::ZERO,
                cell: IVec3::ZERO,
                yaw: 0.0,
                head_height: 0.7,
                half_width: 0.25,
                world: &world,
                player_pos: Vec3::ZERO,
                nav_idle: true,
                in_water: true,
                head: 1,
                idle_anims: &idle,
                mob_index: 0,
                mobs: &[],
                rng: &mut rng,
            };
            assert!(
                ai.tick(&mut ctx).idle_anim.is_none(),
                "no idle plays in water"
            );
        }
    }
}
