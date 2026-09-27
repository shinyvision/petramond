use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput};

const PLAY_CHANCE: f32 = 1.0 / 120.0;
const PLAY_MIN_TICKS: u32 = 40;
const PLAY_SPAN_TICKS: u32 = 60;
const COOLDOWN_TICKS: u32 = 30;
const TICKS_PER_SECOND: f32 = 20.0;

pub struct IdleAnimAi {
    playing: Option<u8>,
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
        if !ctx.nav_idle || ctx.in_fluid.is_some() || ctx.idle_anims.is_empty() {
            self.playing = None;
            self.timer = 0;
            return BehaviorOutput::default();
        }

        if self.timer > 0 {
            self.timer -= 1;
        } else if self.playing.is_some() {
            self.playing = None;
            self.timer = COOLDOWN_TICKS;
        } else if ctx.rng.next_f32() < PLAY_CHANCE {
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
    use crate::mob::model_meta::IdleAnimMeta;
    use crate::mob::MobRng;
    use crate::world::ServerWorld;

    fn ticks_until_start(ai: &mut IdleAnimAi, idle: &[IdleAnimMeta]) -> Option<u32> {
        let world = ServerWorld::new(0, 1);
        let mut rng = MobRng::new(7);
        for _ in 0..20_000 {
            let out = {
                let mut ctx = crate::mob::behavior::test_support::ctx(&world, &mut rng);
                ctx.idle_anims = idle;
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
        let idle = [IdleAnimMeta {
            length: 1.0,
            looping: false,
        }];
        let timer = ticks_until_start(&mut IdleAnimAi::new(), &idle).expect("idle starts");
        assert_eq!(timer, 20, "one-shot plays for its length in ticks");
    }

    #[test]
    fn looping_idle_plays_for_a_longer_random_while() {
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
    fn never_plays_an_idle_animation_while_in_fluid() {
        let world = ServerWorld::new(0, 1);
        let mut rng = MobRng::new(7);
        let idle = [IdleAnimMeta {
            length: 1.0,
            looping: false,
        }];
        let mut ai = IdleAnimAi::new();
        for _ in 0..20_000 {
            let mut ctx = crate::mob::behavior::test_support::ctx(&world, &mut rng);
            ctx.in_fluid = Some(petramond_world::block::Block::Water);
            ctx.idle_anims = &idle;
            assert!(
                ai.tick(&mut ctx).idle_anim.is_none(),
                "no idle plays in a fluid"
            );
        }
    }
}
