use super::{State, SLOT_FUEL, SLOT_METAL};
use crate::content::Casting;
use machine_core::Caches;
use mod_sdk::*;

#[derive(Default)]
pub struct Storage {
    blocks: Vec<BlockId>,
    feed: Vec<String>,
}

impl Storage {
    pub fn resolve() -> Self {
        let blocks = blocks_with_data("forge:storage")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let ids = items_with_data("forge:feed")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let feed = item_names(ids).into_iter().flatten().collect();
        Self { blocks, feed }
    }

    pub fn adjacent(&self, anchor: [i32; 3]) -> Vec<[i32; 3]> {
        // Transform opposite footprint corners through the placed facing.
        let Some(corners) = block_local_to_world(anchor, vec![[0.0, 0.0, 0.0], [2.0, 3.0, 2.0]])
        else {
            return Vec::new();
        };
        if corners.len() != 2 {
            return Vec::new();
        }
        let lo = std::array::from_fn(|i| corners[0][i].min(corners[1][i]).round() as i32);
        let hi = std::array::from_fn(|i| corners[0][i].max(corners[1][i]).round() as i32 - 1);
        find_blocks(lo.map(|v| v - 1), hi.map(|v| v + 1), self.blocks.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| touches(*p, lo, hi))
            .collect()
    }

    pub fn deliver(&self, anchor: [i32; 3], stack: ItemStackData) -> Option<ItemStackData> {
        let mut remaining = Some(stack);
        for pos in self.adjacent(anchor) {
            remaining = container_insert(pos.into(), remaining?);
        }
        remaining
    }

    pub fn feed(
        &self,
        anchor: [i32; 3],
        slots: &mut [Option<ItemStackData>],
        state: &State,
        casting: &Casting,
        caches: &mut Caches,
        keep: u8,
    ) {
        let mould = slots[super::SLOT_MOULD].as_ref().map(|s| s.item.clone());
        let positions = self.adjacent(anchor);
        let containers = container_get_many(
            positions
                .iter()
                .copied()
                .map(ContainerAddress::from)
                .collect(),
        );
        for (pos, container) in positions.into_iter().zip(containers) {
            for (index, stack) in container.unwrap_or_default().into_iter().enumerate() {
                let Some(stack) = stack else {
                    continue;
                };
                let target = if caches.fuel_ticks_for(&stack.item) > 0 {
                    SLOT_FUEL
                } else if self.feed.contains(&stack.item)
                    && casting.is_metal(&stack.item)
                    && caches
                        .recipe_for(
                            super::cast_class(casting, mould.as_deref()),
                            &casting.molten_form(&stack.item),
                        )
                        .is_none_or(|result| result.item != stack.item)
                    && (state.metal.is_empty() || state.metal == casting.molten_form(&stack.item))
                {
                    SLOT_METAL
                } else {
                    continue;
                };
                let current = &slots[target];
                if current
                    .as_ref()
                    .is_some_and(|s| s.item != stack.item || s.data != stack.data)
                {
                    continue;
                }
                let room = keep
                    .min(caches.max_stack_for(&stack.item))
                    .saturating_sub(current.as_ref().map_or(0, |s| s.count));
                if room == 0 {
                    continue;
                }
                if let Some(taken) = container_take(pos.into(), index as u32, room.min(stack.count))
                {
                    machine_core::merge_output(&mut slots[target], &taken);
                }
            }
        }
    }
}

fn touches(p: [i32; 3], lo: [i32; 3], hi: [i32; 3]) -> bool {
    let mut outside = 0;
    for i in 0..3 {
        if p[i] < lo[i] || p[i] > hi[i] {
            if p[i] != lo[i] - 1 && p[i] != hi[i] + 1 {
                return false;
            }
            outside += 1;
        }
    }
    outside == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adjacency_accepts_faces_but_not_edges_corners_or_interior() {
        let lo = [-3, 5, 2];
        let hi = [-2, 7, 3];
        for p in [
            [-4, 6, 2],
            [-1, 6, 3],
            [-2, 4, 3],
            [-2, 8, 2],
            [-3, 6, 1],
            [-2, 6, 4],
        ] {
            assert!(touches(p, lo, hi));
        }
        for p in [[-4, 4, 2], [-4, 4, 1], [-2, 6, 2], [-5, 6, 2]] {
            assert!(!touches(p, lo, hi));
        }
    }
}
