use glam::Vec3;
use petramond_world::{block::Block, light::BlockLight6};

use crate::vertex::{self, BlockLightVertexExt, Vertex};

const CORNER_INSET: f32 = 5.0 / 64.0;

pub(in crate::builder) struct CrownCorners {
    origin: Vec3,
    offsets: [Vec3; 8],
}

impl CrownCorners {
    /// `world` addresses the neighbourhood reads; `origin` is the cell's
    /// minimum corner in the space its face vertices are emitted in.
    pub(in crate::builder) fn new(
        world: [i32; 3],
        origin: Vec3,
        block: impl Fn(i32, i32, i32) -> Block,
        loaded: impl Fn(i32, i32, i32) -> bool,
    ) -> Self {
        let [wx, wy, wz] = world;
        let mut shape = Self {
            origin,
            offsets: [Vec3::ZERO; 8],
        };
        if crate::face::Face::ALL
            .into_iter()
            .filter(|face| {
                let (x, y, z) = face.dir();
                block(wx + x, wy + y, wz + z) == Block::Air
            })
            .count()
            < 2
        {
            return shape;
        }
        let mut cells = [0_u8; 27];
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    let b = block(wx + x, wy + y, wz + z);
                    cells[((z + 1) * 9 + (y + 1) * 3 + x + 1) as usize] =
                        if !loaded(wx + x, wy + y, wz + z) {
                            2
                        } else if b.is_canopy() {
                            1
                        } else if b == Block::Air {
                            0
                        } else {
                            2
                        };
                }
            }
        }
        for (corner, offset) in shape.offsets.iter_mut().enumerate() {
            let c = [
                (corner & 1) as i32,
                ((corner >> 1) & 1) as i32,
                ((corner >> 2) & 1) as i32,
            ];
            let mut count = 0;
            let mut sum = [0_i32; 3];
            let mut pinned = false;
            for z in 0..2 {
                for y in 0..2 {
                    for x in 0..2 {
                        let kind = cells[((c[2] + z) * 9 + (c[1] + y) * 3 + c[0] + x) as usize];
                        pinned |= kind == 2;
                        if kind == 1 {
                            count += 1;
                            for (axis, v) in [x, y, z].into_iter().enumerate() {
                                sum[axis] += v * 2 - 1;
                            }
                        }
                    }
                }
            }
            // Only convex edges/corners: a flat canopy plane has just one
            // unanimous axis. Every cell sharing this lattice point agrees.
            let axes = sum.map(|s| count > 0 && s.abs() == count);
            if !pinned && axes.into_iter().filter(|&a| a).count() >= 2 {
                *offset = Vec3::from_array(std::array::from_fn(|a| {
                    if axes[a] {
                        sum[a].signum() as f32 * CORNER_INSET
                    } else {
                        0.0
                    }
                }));
            }
        }
        shape
    }

    pub(in crate::builder) fn soften_face(
        &self,
        vertices: &mut Vec<Vertex>,
        start: u32,
        exterior: bool,
    ) {
        let start = start as usize;
        let mut original = [vertices[start]; 4];
        for v in &vertices[start..start + 4] {
            original[((v.packed >> vertex::CORNER_SHIFT) & 3) as usize] = *v;
        }
        let mut moved = original.map(|v| Vec3::from_array(v.pos));
        let mut changed = false;
        for p in &mut moved {
            let local = *p - self.origin;
            let key = usize::from(local.x > 0.5)
                | (usize::from(local.y > 0.5) << 1)
                | (usize::from(local.z > 0.5) << 2);
            changed |= self.offsets[key] != Vec3::ZERO;
            *p += self.offsets[key];
        }
        if !changed {
            return;
        }
        if !exterior {
            for v in &mut vertices[start..start + 4] {
                v.pos = moved[((v.packed >> vertex::CORNER_SHIFT) & 3) as usize].to_array();
            }
            return;
        }
        let center = original
            .iter()
            .map(|v| Vec3::from_array(v.pos))
            .sum::<Vec3>()
            * 0.25;
        let mut grid = [original[0]; 9];
        for y in 0..3 {
            for x in 0..3 {
                let weights = [(2 - x) * (2 - y), x * (2 - y), x * y, (2 - x) * y];
                let p = if x == 1 && y == 1 {
                    center
                } else {
                    (0..4).map(|i| moved[i] * weights[i] as f32 * 0.25).sum()
                };
                grid[y * 3 + x] = sample(
                    &original,
                    weights.map(|w| w as u32),
                    p,
                    x as u32 * 8,
                    16 - y as u32 * 8,
                );
            }
        }
        for (i, ids) in [[0, 1, 4, 3], [1, 2, 5, 4], [3, 4, 7, 6], [4, 5, 8, 7]]
            .into_iter()
            .enumerate()
        {
            let quad = ids.map(|id| grid[id]);
            if i == 0 {
                vertices[start..start + 4].copy_from_slice(&quad);
            } else {
                vertices.extend_from_slice(&quad);
            }
        }
    }
}

fn sample(corners: &[Vertex; 4], weights: [u32; 4], pos: Vec3, u: u32, v: u32) -> Vertex {
    let blend = |values: [u32; 4]| {
        (values
            .into_iter()
            .zip(weights)
            .map(|(a, w)| a * w)
            .sum::<u32>()
            + 2)
            / 4
    };
    let lights = corners.map(|c| vertex::decode_vertex_light(&c).channels());
    let light = BlockLight6::new(
        blend(lights.map(|c| c[0])),
        blend(lights.map(|c| c[1])),
        blend(lights.map(|c| c[2])),
    );
    let mut out = corners[0];
    out.pos = pos.to_array();
    out.tint = light.tint_word(vertex::unpack_tint(out.tint));
    let mask = (3 << vertex::CORNER_SHIFT)
        | (3 << vertex::AO_SHIFT)
        | (63 << vertex::SKY_SHIFT)
        | (7 << vertex::UV_MODE_SHIFT)
        | (vertex::CHROMA_HI_MASK << vertex::CHROMA_HI_SHIFT);
    out.packed = (out.packed & !mask)
        | light.packed_bits()
        | (vertex::UV_MODE_CELL_LOCAL << vertex::UV_MODE_SHIFT)
        | (blend(corners.map(|c| (c.packed >> vertex::AO_SHIFT) & 3)) << vertex::AO_SHIFT)
        | (blend(corners.map(|c| (c.packed >> vertex::SKY_SHIFT) & 63)) << vertex::SKY_SHIFT);
    let uv_mask = (vertex::CELL_UV_MASK << vertex::CELL_UV_U_SHIFT)
        | (vertex::CELL_UV_MASK << vertex::CELL_UV_V_SHIFT);
    out.packed2 = (out.packed2 & !(uv_mask | vertex::BLOCK_LIGHT_MASK))
        | vertex::pack_cell_uv(u, v)
        | light.packed2_bits();
    out
}

#[cfg(test)]
mod tests;
