use std::collections::{HashMap, HashSet};

use crate::entity::{DroppedItem, Motion};
use petramond_math::math::Vec3;
use petramond_world::collision::MAX_SAFE_EXTERNAL_SWEEP_DISTANCE;

use super::{terrain_under_drop_is_final, World};

#[cfg(test)]
mod tests;

impl World {
    /// Nearest active item entities over terrain ready for simulation.
    pub(crate) fn nearest_item_entities(
        &self,
        pos: petramond_math::world_pos::WorldPos,
        radius: f32,
        limit: usize,
    ) -> Vec<&DroppedItem> {
        if limit == 0 {
            return Vec::new();
        }
        let mut candidates: Vec<_> = self
            .dropped_items
            .items
            .iter()
            .filter_map(|item| {
                let distance = (item.pos - pos).length_squared();
                (distance <= radius * radius && terrain_under_drop_is_final(self, item.pos))
                    .then_some((distance, item))
            })
            .collect();
        let compare = |a: &(f32, &DroppedItem), b: &(f32, &DroppedItem)| {
            a.0.total_cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id))
        };
        if candidates.len() > limit {
            candidates.select_nth_unstable_by(limit, compare);
            candidates.truncate(limit);
        }
        candidates.sort_unstable_by(compare);
        candidates.into_iter().map(|(_, item)| item).collect()
    }

    /// Apply additive velocity changes without changing an item's lifecycle.
    pub(crate) fn impulse_item_entities(&mut self, impulses: &[(u64, Vec3)]) -> Vec<bool> {
        if impulses.is_empty() {
            return Vec::new();
        }
        let requested: HashSet<_> = impulses.iter().map(|(id, _)| *id).collect();
        // Scan once; only requested entities need the terrain readiness probe.
        let eligible: HashMap<_, _> = self
            .dropped_items
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                requested.contains(&item.id)
                    && !matches!(item.motion, Motion::Stuck(_))
                    && item.pickup_requested.is_none()
                    && terrain_under_drop_is_final(self, item.pos)
            })
            .map(|(index, item)| (item.id, index))
            .collect();
        let max_speed = MAX_SAFE_EXTERNAL_SWEEP_DISTANCE / crate::events::tick::TICK_DT;
        impulses
            .iter()
            .map(|(id, delta)| {
                let Some(&index) = eligible.get(id) else {
                    return false;
                };
                let item = &mut self.dropped_items.items[index];
                let velocity = item.vel + *delta;
                if !velocity.length_squared().is_finite()
                    || velocity.length_squared() > max_speed * max_speed
                {
                    return false;
                }
                item.vel = velocity;
                true
            })
            .collect()
    }
}
