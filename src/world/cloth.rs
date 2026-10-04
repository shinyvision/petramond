use crate::world::{World, WorldSide};
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::chunk::{section_local, SECTION_SIZE};

/// A placed block that hangs a cloth: its cell and the cloth row it names.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PlacedCloth {
    pub cell: IVec3,
    pub cloth: u8,
}

impl<S: WorldSide> World<S> {
    /// Every cloth-carrying cell within `radius` blocks of `center`. Distance, not the
    /// view frustum: a cloth keeps moving while the camera looks away, so turning
    /// back never finds it frozen mid-swing.
    pub fn collect_cloths(&self, center: IVec3, radius: i32, out: &mut Vec<PlacedCloth>) {
        out.clear();
        let sec = SECTION_SIZE as i32;
        let r2 = i64::from(radius) * i64::from(radius);
        for sp in &self.data.presented_sections {
            let lo = IVec3::new(sp.cx * sec, sp.cy * sec, sp.cz * sec);
            let near = center.clamp(lo, lo + IVec3::splat(sec - 1));
            if (near - center).as_i64vec3().length_squared() > r2 {
                continue;
            }
            let Some(section) = self.data.sections.get(sp) else {
                continue;
            };
            for &cell_idx in section.presented_cells() {
                let idx = cell_idx as usize;
                let Some(def) = Block::from_id(section.block_at_idx(idx)).cloth() else {
                    continue;
                };
                let (lx, ly, lz) = section_local(idx);
                let cell = lo + IVec3::new(lx as i32, ly as i32, lz as i32);
                if (cell - center).as_i64vec3().length_squared() <= r2 {
                    out.push(PlacedCloth {
                        cell,
                        cloth: def.id,
                    });
                }
            }
        }
    }
}
