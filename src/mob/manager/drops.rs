use crate::mob::{EntityRef, Mob, MobDamageFeedback, MobId};

use super::Mobs;

#[derive(Copy, Clone, Debug)]
pub struct DeathDrop {
    pub kind: Mob,
    pub pos: petramond_math::world_pos::WorldPos,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

#[derive(Clone, Debug)]
pub struct MobSpill {
    pub pos: petramond_math::world_pos::WorldPos,
    pub stacks: Vec<petramond_world::item::ItemStack>,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

#[derive(Copy, Clone, Debug)]
pub struct ShearDrop {
    pub item: petramond_world::item::ItemType,
    pub count: u8,
    pub pos: petramond_math::world_pos::WorldPos,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

impl Mobs {
    pub fn damage_mob(
        &mut self,
        id: MobId,
        amount: f32,
        origin: Option<petramond_math::world_pos::WorldPos>,
        attack: bool,
        attacker: Option<EntityRef>,
        feedback: &MobDamageFeedback,
    ) -> Option<DeathDrop> {
        let slot = self.slot(id)?;
        let mob = &mut self.list[slot];
        if !mob.damage(amount, origin, attack, attacker, feedback) {
            return None;
        }
        let drop = DeathDrop {
            kind: mob.kind,
            pos: mob.pos,
            skylight: mob.skylight,
            blocklight: mob.blocklight,
        };
        self.spill_container(slot);
        Some(drop)
    }

    pub fn shear_mob(&mut self, id: MobId) -> Option<ShearDrop> {
        let mob = self.mob_mut(id)?;
        let spec = super::def(mob.kind).shear?;
        let count = mob.shear()?;
        Some(ShearDrop {
            item: spec.drop,
            count,
            pos: mob.pos,
            skylight: mob.skylight,
            blocklight: mob.blocklight,
        })
    }
}
