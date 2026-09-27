use std::collections::btree_map::{BTreeMap, Entry};

use petramond::net::protocol::{EntityLane, EntityRow, ItemStateRow, MobStateRow, PlayerStateRow};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Adopt {
    Advance,
    Reseed,
}

pub(crate) trait Replica<R>: Sized {
    type Ctx;
    fn spawn(row: &R, ctx: &mut Self::Ctx) -> Self;
    fn advance(&mut self, row: &R, ctx: &mut Self::Ctx);
    fn reseed(&mut self, row: &R, ctx: &mut Self::Ctx);
    fn hold(&mut self);
}

struct Slot<E> {
    entry: E,
    window: u64,
}

pub(crate) struct EntityStore<K, E> {
    slots: BTreeMap<K, Slot<E>>,
    window: u64,
}

impl<K, E> Default for EntityStore<K, E> {
    fn default() -> Self {
        Self {
            slots: BTreeMap::new(),
            window: 0,
        }
    }
}

impl<K: Ord + Copy, E> EntityStore<K, E> {
    pub(crate) fn apply<R>(
        &mut self,
        lane: &EntityLane<R, K>,
        adopt: Adopt,
        ctx: &mut E::Ctx,
        keep: impl Fn(&R) -> bool,
    ) where
        R: EntityRow<Id = K>,
        E: Replica<R>,
    {
        self.window += 1;
        let window = self.window;
        for id in &lane.despawned {
            self.slots.remove(id);
        }
        for row in lane.spawned.iter().filter(|row| keep(row)) {
            let entry = E::spawn(row, ctx);
            self.slots.insert(row.entity_id(), Slot { entry, window });
        }
        for row in lane.updated.iter().filter(|row| keep(row)) {
            match self.slots.entry(row.entity_id()) {
                Entry::Occupied(mut occupied) => {
                    let slot = occupied.get_mut();
                    match adopt {
                        Adopt::Advance => slot.entry.advance(row, ctx),
                        Adopt::Reseed => slot.entry.reseed(row, ctx),
                    }
                    slot.window = window;
                }
                Entry::Vacant(vacant) => {
                    let entry = E::spawn(row, ctx);
                    vacant.insert(Slot { entry, window });
                }
            }
        }
        for slot in self.slots.values_mut() {
            if slot.window != window {
                slot.entry.hold();
            }
        }
    }

    pub(crate) fn get(&self, id: &K) -> Option<&E> {
        self.slots.get(id).map(|slot| &slot.entry)
    }

    pub(crate) fn get_mut(&mut self, id: &K) -> Option<&mut E> {
        self.slots.get_mut(id).map(|slot| &mut slot.entry)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &E> {
        self.slots.values().map(|slot| &slot.entry)
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut E> {
        self.slots.values_mut().map(|slot| &mut slot.entry)
    }

    pub(crate) fn iter_with_ids(&self) -> impl Iterator<Item = (K, &E)> {
        self.slots.iter().map(|(id, slot)| (*id, &slot.entry))
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    #[cfg(test)]
    pub(crate) fn keys(&self) -> impl Iterator<Item = K> + '_ {
        self.slots.keys().copied()
    }
}

pub(crate) trait AssignFrom {
    fn assign_from(&mut self, src: &Self);
}

fn assign_named(dst: &mut Vec<(String, f32)>, src: &[(String, f32)]) {
    dst.truncate(src.len());
    for ((name, value), (src_name, src_value)) in dst.iter_mut().zip(src) {
        name.clone_from(src_name);
        *value = *src_value;
    }
    let kept = dst.len();
    dst.extend_from_slice(&src[kept..]);
}

impl AssignFrom for MobStateRow {
    fn assign_from(&mut self, src: &Self) {
        let MobStateRow {
            id,
            kind_id,
            pos,
            yaw,
            tilt,
            anim_time,
            moving,
            idle_anim,
            head_yaw,
            head_pitch,
            hurt_timer,
            dead,
            shorn,
            emitters,
            conditions,
            anims,
            ragdoll,
            dig,
            held,
            draw,
        } = src;
        self.id = *id;
        self.kind_id = *kind_id;
        self.pos = *pos;
        self.yaw = *yaw;
        self.tilt.clone_from(tilt);
        self.anim_time = *anim_time;
        self.moving = *moving;
        self.idle_anim = *idle_anim;
        self.head_yaw = *head_yaw;
        self.head_pitch = *head_pitch;
        self.hurt_timer = *hurt_timer;
        self.dead = *dead;
        self.shorn = *shorn;
        self.emitters.clone_from(emitters);
        self.conditions.clone_from(conditions);
        assign_named(&mut self.anims, anims);
        self.ragdoll.clone_from(ragdoll);
        self.dig = *dig;
        self.held = *held;
        self.draw.clone_from(draw);
    }
}

impl AssignFrom for ItemStateRow {
    fn assign_from(&mut self, src: &Self) {
        let ItemStateRow {
            id,
            item_id,
            count,
            data,
            pos,
            spin,
            flight,
        } = src;
        self.id = *id;
        self.item_id = *item_id;
        self.count = *count;
        self.data.clone_from(data);
        self.pos = *pos;
        self.spin = *spin;
        self.flight = *flight;
    }
}

impl AssignFrom for PlayerStateRow {
    fn assign_from(&mut self, src: &Self) {
        let PlayerStateRow {
            conditions,
            id,
            transform,
            on_ground,
            sneaking,
            sleeping,
            sleep_yaw,
            alive,
            visible,
            held_item,
            held_data,
            off_hand_item,
            off_hand_data,
            mining,
            eating,
            eating_off_hand,
            held_pose_main,
            held_pose_off,
            held_display,
            bone_poses,
            animator,
            hurt_recent,
            snap,
            mount,
        } = src;
        self.conditions.clone_from(conditions);
        self.id.clone_from(id);
        self.transform.clone_from(transform);
        self.on_ground = *on_ground;
        self.sneaking = *sneaking;
        self.sleeping = *sleeping;
        self.sleep_yaw = *sleep_yaw;
        self.alive = *alive;
        self.visible = *visible;
        self.held_item = *held_item;
        self.held_data.clone_from(held_data);
        self.off_hand_item = *off_hand_item;
        self.off_hand_data.clone_from(off_hand_data);
        self.mining = *mining;
        self.eating = *eating;
        self.eating_off_hand = *eating_off_hand;
        self.held_pose_main.clone_from(held_pose_main);
        self.held_pose_off.clone_from(held_pose_off);
        self.held_display = *held_display;
        self.bone_poses.clone_from(bone_poses);
        let petramond::player::AnimatorClaims { params, plays } = animator;
        self.animator.params.clone_from(params);
        self.animator.plays.clone_from(plays);
        self.hurt_recent = *hurt_recent;
        self.snap = *snap;
        self.mount.clone_from(mount);
    }
}
