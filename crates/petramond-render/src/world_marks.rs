use std::ops::Range;

use glam::IVec3;
use petramond_math::world_pos::WorldPos;

#[derive(Default)]
pub struct WorldMarks {
    pub items: Vec<WorldMark>,
    pub paint: petramond_ui::DrawList,
}

pub enum WorldMark {
    Line {
        from: WorldPos,
        to: WorldPos,
        color: [f32; 4],
        width: f32,
        occluded: f32,
    },
    Image {
        at: WorldPos,
        occluded: f32,
        tint: [f32; 4],
        image: crate::ClientOverlayImage,
    },
    Paint {
        at: WorldPos,
        occluded: f32,
        batches: Range<usize>,
    },
}

impl WorldMarks {
    pub fn clear(&mut self) {
        self.items.clear();
        self.paint.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn paint(
        &mut self,
        at: WorldPos,
        occluded: f32,
        draw: impl FnOnce(&mut petramond_ui::DrawList),
    ) {
        self.paint.begin_overlay();
        let start = self.paint.batches.len();
        draw(&mut self.paint);
        let end = self.paint.batches.len();
        if end > start {
            self.items.push(WorldMark::Paint {
                at,
                occluded,
                batches: start..end,
            });
        }
    }

    pub(crate) fn tests_depth(&self) -> bool {
        self.items.iter().any(|item| {
            let occluded = match item {
                WorldMark::Line { occluded, .. }
                | WorldMark::Image { occluded, .. }
                | WorldMark::Paint { occluded, .. } => *occluded,
            };
            occluded < 1.0
        })
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct MarkVertex {
    pub at: [f32; 3],
    pub other: [f32; 3],
    pub offset: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub style: [f32; 2],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum MarkTex {
    Solid,
    Font,
    Theme(u16),
    Image(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct MarkBatch {
    pub tex: MarkTex,
    pub start: u32,
    pub count: u32,
}

const SOLID_UV: [f32; 2] = [-1.0, -1.0];

pub(crate) fn bake(
    marks: &WorldMarks,
    origin: IVec3,
    verts: &mut Vec<MarkVertex>,
    batches: &mut Vec<MarkBatch>,
) {
    verts.clear();
    batches.clear();
    let same_image = |a: usize, b: usize| match (&marks.items[a], &marks.items[b]) {
        (WorldMark::Image { image: x, .. }, WorldMark::Image { image: y, .. }) => x.key == y.key,
        _ => false,
    };
    let mut push = |verts: &mut Vec<MarkVertex>, tex: MarkTex, new: &[MarkVertex]| {
        let start = verts.len() as u32;
        verts.extend_from_slice(new);
        let merges = batches.last().is_some_and(|last| {
            last.start + last.count == start
                && match (last.tex, tex) {
                    (MarkTex::Image(a), MarkTex::Image(b)) => same_image(a, b),
                    (a, b) => a == b,
                }
        });
        match batches.last_mut() {
            Some(last) if merges => last.count += new.len() as u32,
            _ => batches.push(MarkBatch {
                tex,
                start,
                count: new.len() as u32,
            }),
        }
    };
    for (index, item) in marks.items.iter().enumerate() {
        match item {
            WorldMark::Line {
                from,
                to,
                color,
                width,
                occluded,
            } => {
                let (a, b) = (from.relative_to(origin), to.relative_to(origin));
                let end = |at: glam::Vec3, other: glam::Vec3, side: f32| MarkVertex {
                    at: at.to_array(),
                    other: other.to_array(),
                    offset: [side, width * 0.5],
                    uv: SOLID_UV,
                    color: *color,
                    style: [*occluded, 1.0],
                };
                let (a_left, a_right) = (end(a, b, 1.0), end(a, b, -1.0));
                let (b_left, b_right) = (end(b, a, -1.0), end(b, a, 1.0));
                push(
                    verts,
                    MarkTex::Solid,
                    &[a_left, a_right, b_right, a_left, b_right, b_left],
                );
            }
            WorldMark::Image {
                at,
                occluded,
                tint,
                image,
            } => {
                let at = at.relative_to(origin).to_array();
                let [x, y, w, h] = image.rect;
                let [u0, v0, u1, v1] = image.uv;
                let corner = |px: [f32; 2], uv: [f32; 2]| MarkVertex {
                    at,
                    other: at,
                    offset: px,
                    uv,
                    color: *tint,
                    style: [*occluded, 0.0],
                };
                let tl = corner([x, y], [u0, v0]);
                let tr = corner([x + w, y], [u1, v0]);
                let br = corner([x + w, y + h], [u1, v1]);
                let bl = corner([x, y + h], [u0, v1]);
                push(verts, MarkTex::Image(index), &[tl, bl, br, tl, br, tr]);
            }
            WorldMark::Paint {
                at,
                occluded,
                batches: range,
            } => {
                let at = at.relative_to(origin).to_array();
                let Some(painted) = marks.paint.batches.get(range.clone()) else {
                    continue;
                };
                for batch in painted {
                    let tex = match batch.tex {
                        petramond_ui::TexId::Solid => MarkTex::Solid,
                        petramond_ui::TexId::Font => MarkTex::Font,
                        petramond_ui::TexId::ThemePage(page) => MarkTex::Theme(page),
                        petramond_ui::TexId::DocImage(_) => continue,
                    };
                    let span = batch.start as usize..(batch.start + batch.count) as usize;
                    let Some(quads) = marks.paint.vertices.get(span) else {
                        continue;
                    };
                    let pinned: Vec<MarkVertex> = quads
                        .iter()
                        .map(|v| MarkVertex {
                            at,
                            other: at,
                            offset: v.pos,
                            uv: v.uv,
                            color: v.color,
                            style: [*occluded, 0.0],
                        })
                        .collect();
                    push(verts, tex, &pinned);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_marks(from: WorldPos, to: WorldPos) -> WorldMarks {
        WorldMarks {
            items: vec![WorldMark::Line {
                from,
                to,
                color: [1.0; 4],
                width: 3.0,
                occluded: 0.0,
            }],
            paint: Default::default(),
        }
    }

    #[test]
    fn a_far_mark_bakes_exactly_like_the_same_mark_near_the_origin() {
        let far = IVec3::new(536_870_000, 64, -536_870_000);
        let offset = |o: IVec3, x: f64, y: f64, z: f64| {
            WorldPos::new(f64::from(o.x) + x, f64::from(o.y) + y, f64::from(o.z) + z)
        };
        let bake_at = |origin: IVec3| {
            let marks = line_marks(
                offset(origin, 0.125, 0.5, 3.0625),
                offset(origin, 7.75, 1.25, -2.5),
            );
            let (mut verts, mut batches) = (Vec::new(), Vec::new());
            bake(&marks, origin, &mut verts, &mut batches);
            verts
        };
        assert_eq!(bake_at(far), bake_at(IVec3::ZERO));
    }
}
