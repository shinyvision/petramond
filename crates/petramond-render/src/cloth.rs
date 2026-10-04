//! Cloth sheets as lit, textured triangles in the item-sprite stream: one vertex per
//! simulated point, double-sided through the stream's cull-free pipeline.

use glam::{IVec3, Vec3};

use crate::item_model::ItemVertex;
use crate::lighting::{fold_tint, DynLight, LightEnv};
use crate::views::{ClothPoint, ClothPresentation};

pub fn build_cloths(
    cloths: &[ClothPresentation],
    points: &[ClothPoint],
    origin: IVec3,
    env: LightEnv,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    for cloth in cloths {
        let (cols, rows) = (usize::from(cloth.cols), usize::from(cloth.rows));
        let first = cloth.first as usize;
        let Some(grid) = points.get(first..first + cols * rows) else {
            continue;
        };
        if cols < 2 || rows < 2 {
            continue;
        }
        let offset = (cloth.cell - origin).as_vec3();
        let [tu0, tv0, tu1, tv1] = crate::atlas::tile_uv(cloth.tile);
        let [su0, sv0, su1, sv1] = cloth.uv;
        let at = |u: usize, v: usize| grid[v * cols + u].pos;
        let base = verts.len() as u32;
        for v in 0..rows {
            for u in 0..cols {
                let p = grid[v * cols + u];
                let du = at((u + 1).min(cols - 1), v) - at(u.saturating_sub(1), v);
                let dv = at(u, (v + 1).min(rows - 1)) - at(u, v.saturating_sub(1));
                let fu = su0 + (su1 - su0) * u as f32 / (cols - 1) as f32;
                let fv = sv0 + (sv1 - sv0) * v as f32 / (rows - 1) as f32;
                verts.push(ItemVertex {
                    pos: (offset + p.pos).to_array(),
                    uv: [tu0 + (tu1 - tu0) * fu, tv0 + (tv1 - tv0) * fv],
                    shade: shade(du.cross(dv)),
                    tint: fold_tint([1.0; 3], DynLight::new(p.skylight, p.blocklight), env),
                });
            }
        }
        for v in 0..rows as u32 - 1 {
            for u in 0..cols as u32 - 1 {
                let i = base + v * cols as u32 + u;
                let below = i + cols as u32;
                indices.extend([i, i + 1, below, i + 1, below + 1, below]);
            }
        }
    }
}

/// The block faces' directional shading, blended by the normal's direction so a sheet
/// darkens as it turns the way a cube's sides do. Both sides of a sheet shade alike.
fn shade(normal: Vec3) -> f32 {
    let n = normal.normalize_or(Vec3::Z);
    let n2 = n * n;
    n2.x * 0.6 + n2.y * 0.9 + n2.z * 0.8
}
