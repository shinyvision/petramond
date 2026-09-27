use super::*;

/// One quarter turn about Y. `FACE_BEFORE_TURN[i]` is the source face for turned slot `i`.
/// `(x, z) -> (1 - z, x)`: the authored `-Z` front ends up at `+X`, same as `Facing` North -> East.
pub const FACE_BEFORE_TURN: [usize; 6] = [5, 4, 2, 3, 0, 1];

pub const FRONT_AFTER_TURN: [usize; 4] = [5, 0, 4, 1];

pub fn face_uv_turns(face: usize, turns: u8) -> u8 {
    match face {
        2 => turns & 3,
        3 => (4 - (turns & 3)) & 3,
        _ => 0,
    }
}

impl BoxDef {
    pub fn turned(&self) -> BoxDef {
        if let Some(pose) = self.pose {
            return BoxDef {
                pose: Some(pose.turned()),
                ..*self
            };
        }
        let (min, max) = (self.aabb.min, self.aabb.max);
        BoxDef {
            aabb: Aabb {
                min: [1.0 - max[2], min[1], min[0]],
                max: [1.0 - min[2], max[1], max[0]],
            },
            faces: std::array::from_fn(|i| self.faces[FACE_BEFORE_TURN[i]]),
            tiles: std::array::from_fn(|i| self.tiles[FACE_BEFORE_TURN[i]]),
            occludes: self.occludes,
            collides: self.collides,
            double_sided: self.double_sided,
            casts_ao: self.casts_ao,
            art_turns: std::array::from_fn(|i| self.art_turns[FACE_BEFORE_TURN[i]]),
            uv: std::array::from_fn(|i| self.uv[FACE_BEFORE_TURN[i]]),
            uv_turns: std::array::from_fn(|i| self.uv_turns[FACE_BEFORE_TURN[i]]),
            pose: None,
        }
    }

    pub fn mirrored_y(&self) -> BoxDef {
        const SWAP_Y: [usize; 6] = [0, 1, 3, 2, 4, 5];
        let (min, max) = (self.aabb.min, self.aabb.max);
        BoxDef {
            aabb: Aabb {
                min: [min[0], 1.0 - max[1], min[2]],
                max: [max[0], 1.0 - min[1], max[2]],
            },
            faces: std::array::from_fn(|i| self.faces[SWAP_Y[i]]),
            tiles: std::array::from_fn(|i| self.tiles[SWAP_Y[i]]),
            occludes: self.occludes,
            collides: self.collides,
            double_sided: self.double_sided,
            casts_ao: self.casts_ao,
            art_turns: std::array::from_fn(|i| self.art_turns[SWAP_Y[i]]),
            uv: std::array::from_fn(|i| self.uv[SWAP_Y[i]]),
            uv_turns: std::array::from_fn(|i| self.uv_turns[SWAP_Y[i]]),
            pose: self.pose.map(|p| {
                let q = p.rotation;
                crate::block::BoxPose {
                    rotation: crate::mathh::Quat::from_xyzw(-q.x, q.y, -q.z, q.w),
                    origin: crate::mathh::Vec3::new(p.origin.x, 1.0 - p.origin.y, p.origin.z),
                }
            }),
        }
    }

    fn art_advanced(&self, turns: u8) -> BoxDef {
        BoxDef {
            art_turns: self.art_turns.map(|t| (t + turns) & 3),
            ..*self
        }
    }
}

pub(super) fn turned_list(list: &[BoxDef], turns: u8) -> Vec<BoxDef> {
    let mut v: Vec<BoxDef> = list.to_vec();
    for _ in 0..(turns & 3) {
        v = v.iter().map(BoxDef::turned).collect();
    }
    v
}

/// The quarter-turned donor list a corner form composes against: turned
/// geometry whose faces also REMEMBER they were authored one turn round.
pub(super) fn donor_list(list: &[BoxDef], turns: u8) -> Vec<BoxDef> {
    turned_list(list, turns)
        .iter()
        .map(|b| b.art_advanced(turns))
        .collect()
}

/// Intersection of two box lists, which is the outer corner form. It's the stair rule's
/// `back_mask & back_mask` lifted from quadrant masks to boxes: what's left is the matter both
/// orientations agree on, so the front art wraps around the turned side. Each result face takes
/// its style from the parent whose face plane it lies on (`self` wins ties, e.g. the top of a
/// full-cell slab), including that parent's [`art_turns`](BoxDef::art_turns). That's how the
/// turned parent's front tile and UV frame reach the wrapped face.
pub(super) fn intersect_lists(a: &[BoxDef], b: &[BoxDef]) -> Vec<BoxDef> {
    let mut out = Vec::new();
    for pa in a {
        for pb in b {
            let mut r = pa.aabb;
            for ax in 0..3 {
                r.min[ax] = r.min[ax].max(pb.aabb.min[ax]);
                r.max[ax] = r.max[ax].min(pb.aabb.max[ax]);
            }
            if (0..3).any(|ax| r.min[ax] >= r.max[ax]) {
                continue;
            }
            let mut piece = BoxDef { aabb: r, ..*pa };
            for i in 0..6 {
                let (axis, high) = [
                    (0, true),
                    (0, false),
                    (1, true),
                    (1, false),
                    (2, true),
                    (2, false),
                ][i];
                let plane = if high { r.max[axis] } else { r.min[axis] };
                let of = |p: &BoxDef| {
                    if high {
                        p.aabb.max[axis] == plane
                    } else {
                        p.aabb.min[axis] == plane
                    }
                };
                let parent = if of(pa) { pa } else { pb };
                piece.faces[i] = parent.faces[i];
                piece.tiles[i] = parent.tiles[i];
                piece.art_turns[i] = parent.art_turns[i];
            }
            piece.occludes = pa.occludes && pb.occludes;
            piece.collides = pa.collides && pb.collides;
            piece.double_sided = pa.double_sided || pb.double_sided;
            piece.casts_ao = pa.casts_ao && pb.casts_ao;
            if !out.contains(&piece) {
                out.push(piece);
            }
        }
    }
    out
}

pub(super) fn union_lists(a: &[BoxDef], b: &[BoxDef]) -> Vec<BoxDef> {
    let mut out: Vec<BoxDef> = a.to_vec();
    for pb in b {
        if !out.iter().any(|pa| pa.aabb == pb.aabb) {
            out.push(*pb);
        }
    }
    out
}

pub(super) fn union_bounds(set: &[BoxDef]) -> Aabb {
    let mut bounds = Aabb {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    for b in set
        .iter()
        .filter_map(|b| b.posed_bounds().clipped_to_cell())
    {
        for a in 0..3 {
            bounds.min[a] = bounds.min[a].min(b.min[a]);
            bounds.max[a] = bounds.max[a].max(b.max[a]);
        }
    }
    if bounds.min[0] > bounds.max[0] {
        return Aabb {
            min: [0.0; 3],
            max: [0.0; 3],
        };
    }
    bounds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_posed_box_turns_by_its_pose_not_by_permuting_its_faces() {
        let mut faces = [false; 6];
        faces[2] = true;
        let b = BoxDef {
            aabb: Aabb {
                min: [0.0, 0.5, -0.2],
                max: [1.0, 0.5, 1.2],
            },
            faces,
            tiles: [None; 6],
            occludes: false,
            collides: false,
            double_sided: true,
            casts_ao: true,
            art_turns: [0; 6],
            uv: [None; 6],
            uv_turns: [0; 6],
            pose: Some(crate::block::BoxPose::from_euler_degrees(
                [45.0, 0.0, 0.0],
                [0.5; 3],
            )),
        };
        let t = b.turned();
        assert_eq!(t.faces, b.faces, "a posed box keeps its authored faces");
        assert_eq!(t.aabb, b.aabb, "a posed box keeps its authored extent");
        let (p0, p1) = (b.pose.unwrap(), t.pose.unwrap());
        let turn = |p: glam::Vec3| glam::Vec3::new(1.0 - p.z, p.y, p.x);
        for c in [[0.0, 0.5, -0.2], [1.0, 0.5, 1.2], [0.3, 0.5, 0.4]] {
            let c = glam::Vec3::from(c);
            assert!((p1.apply(c) - turn(p0.apply(c))).length() < 1e-4);
        }
        assert_eq!(t.face_frame_turns(1, 2), 0);
    }
}
