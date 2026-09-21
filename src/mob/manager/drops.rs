use crate::mob::{EntityRef, Mob, MobDamageFeedback};

use super::Mobs;

/// What a mob leaves behind the instant it dies, so `Game` can roll its loot table and
/// spawn the drops (the manager has only `&World` and can't spawn item entities itself).
#[derive(Copy, Clone, Debug)]
pub struct DeathDrop {
    pub kind: Mob,
    pub pos: petramond_math::world_pos::WorldPos,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

/// Stacks a mob carried when it left the world (death or despawn): scattered at
/// its body so nothing a mob held is destroyed with it.
#[derive(Clone, Debug)]
pub struct MobSpill {
    pub pos: petramond_math::world_pos::WorldPos,
    pub stacks: Vec<petramond_world::item::ItemStack>,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

/// What a successful shear yields, so `Game` can spawn the drop (like [`DeathDrop`],
/// the manager can't spawn item entities itself). The count is already rolled from the
/// mob's own deterministic RNG.
#[derive(Copy, Clone, Debug)]
pub struct ShearDrop {
    pub item: petramond_world::item::ItemType,
    pub count: u8,
    pub pos: petramond_math::world_pos::WorldPos,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

impl Mobs {
    /// Apply `amount` damage to the mob at `index`. `attacker` (when the damage
    /// source names one) lands in the mob's retaliation memory.
    /// Returns the loot drop the mob leaves if the hit killed it, else `None`. Keeps
    /// `list` private — `Game` never holds a `&mut Instance`.
    pub fn damage_mob(
        &mut self,
        index: usize,
        amount: f32,
        origin: Option<petramond_math::world_pos::WorldPos>,
        attack: bool,
        attacker: Option<EntityRef>,
        feedback: &MobDamageFeedback,
    ) -> Option<DeathDrop> {
        let mob = self.mob_mut(index)?;
        if !mob.damage(amount, origin, attack, attacker, feedback) {
            return None;
        }
        let drop = DeathDrop {
            kind: mob.kind,
            pos: mob.pos,
            skylight: mob.skylight,
            blocklight: mob.blocklight,
        };
        self.spill_container(index);
        Some(drop)
    }

    /// Shear the mob at `index`: `Some` drop when it is a coated shearable species
    /// (its coat is hidden and the regrow countdown starts), else `None`. Keeps
    /// `list` private, like [`damage_mob`](Self::damage_mob).
    pub fn shear_mob(&mut self, index: usize) -> Option<ShearDrop> {
        let mob = self.mob_mut(index)?;
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
