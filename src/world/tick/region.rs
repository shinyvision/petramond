use crate::world::World;
use petramond_math::math::IVec3;
use petramond_world::{
    block::Block,
    border::WORLD_BORDER,
    chunk::{section_idx, SectionPos, SECTION_SIZE, WORLD_MAX_Y, WORLD_MIN_Y},
};

impl World {
    /// Recheck existing blocks in inclusive bounds without announcing fake terrain edits.
    pub(crate) fn queue_non_air_updates_in_box(&mut self, min: IVec3, max: IVec3) {
        let min = min.max(IVec3::new(-WORLD_BORDER, WORLD_MIN_Y, -WORLD_BORDER));
        let max = max.min(IVec3::new(
            WORLD_BORDER - 1,
            WORLD_MAX_Y - 1,
            WORLD_BORDER - 1,
        ));
        if !min.cmple(max).all() {
            return;
        }
        let first = SectionPos::from_world(min.x, min.y, min.z).unwrap();
        let last = SectionPos::from_world(max.x, max.y, max.z).unwrap();
        let mut pending = Vec::new();
        for cy in first.cy..=last.cy {
            for cz in first.cz..=last.cz {
                for cx in first.cx..=last.cx {
                    let sp = SectionPos::new(cx, cy, cz);
                    let Some(section) = self.sections.get(&sp).filter(|s| !s.is_empty_air()) else {
                        continue;
                    };
                    let (x, y, z) = sp.origin_world();
                    let origin = IVec3::new(x, y, z);
                    let lo = (min - origin).max(IVec3::ZERO);
                    let hi = (max - origin).min(IVec3::splat(SECTION_SIZE as i32 - 1));
                    let blocks = section.blocks();
                    for y in lo.y..=hi.y {
                        for z in lo.z..=hi.z {
                            for x in lo.x..=hi.x {
                                if blocks.get(section_idx(x as usize, y as usize, z as usize))
                                    != Block::Air.id()
                                {
                                    pending.push(origin + IVec3::new(x, y, z));
                                }
                            }
                        }
                    }
                    for pos in pending.drain(..) {
                        self.queue_block_update(pos);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
