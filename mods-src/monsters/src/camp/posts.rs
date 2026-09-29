//! Spawn posts: the floor cells a camp's guards stand on. Each one carries `[role, yaw]` cell
//! data, so whatever populates the camp finds its posts when their section generates.
//!
//! A post is the cell BELOW the feet: something a body stands on centred (a full block, a log, a
//! top slab or the camp's ground) with two clear cells above it. Posts are picked last, from the
//! finished plan, so they can never sit on something a later pass wrote over.

use std::f32::consts::TAU;

use mod_sdk::build::{Dir, Draw, Form, Half, Material};

use super::ground::natural_top;
use super::layout::HutKind;
use super::{Camp, Res};
use crate::keys::POST_MARKER;

/// The yaw byte of a post whose guard may face any way.
const NO_YAW: u8 = 0xFF;

/// Most posts a camp gets.
const MAX_POSTS: usize = 14;

/// Posts keep this far apart in x/z (squared).
const MIN_GAP_SQ: i32 = 9;

/// What a guard on a post is for; the discriminant is the first byte of the cell data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PostRole {
    Yard = 0,
    /// A tower top or a fortress wall walk.
    Watch = 1,
    /// Just inside a gate.
    Gate = 2,
    /// By the well, statue or arena.
    Centre = 3,
    /// By a hut's door, or inside it.
    Hut = 4,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Post {
    pub floor: [i32; 3],
    pub role: PostRole,
    /// `yaw = yaw / 256 * TAU` (the way mobs face), or [`NO_YAW`].
    pub yaw: u8,
}

/// The yaw byte for facing along `(dx, dz)`. Mobs face `(-sin yaw, -cos yaw)`; the byte for a
/// bearing that would round to [`NO_YAW`] is nudged off it.
fn yaw_byte(dx: f32, dz: f32) -> u8 {
    if dx == 0.0 && dz == 0.0 {
        return NO_YAW;
    }
    let turns = ((-dx).atan2(-dz) / TAU).rem_euclid(1.0);
    ((turns * 256.0).round() as u32 % 256).min(u32::from(NO_YAW) - 1) as u8
}

/// Whether a body stands centred on this material with its feet at the cell's top.
fn solid_floor(m: &Material) -> bool {
    match m.form {
        Form::Block | Form::Log => true,
        Form::Slab => m.get_half() == Half::Top,
        Form::Other => matches!(
            m.block.as_str(),
            "petramond:grass"
                | "petramond:dirt"
                | "petramond:coarse_dirt"
                | "petramond:podzol"
                | "petramond:sand"
                | "petramond:gravel"
                | "petramond:mud"
                | "petramond:stone"
                | "petramond:cobblestone"
                | "petramond:sandstone"
                | "petramond:tuff"
        ),
        _ => false,
    }
}

/// Ground clutter a post can clear from above its floor: it is only there for looks.
fn litter(m: &Material) -> bool {
    m.form == Form::Decor
        && matches!(
            m.block.as_str(),
            "petramond:short_grass"
                | "petramond:dead_bush"
                | "petramond:snow_layer"
                | "petramond:pebbles_small"
                | "petramond:pebbles_medium"
                | "petramond:pebbles_large"
        )
}

impl Camp<'_> {
    /// Picks the camp's posts and marks them in the plan. `fort_h` is the fortress wall height.
    pub(super) fn build_posts(&mut self, fort_h: i32) {
        let target = ((self.radius * 0.5).round() as usize).clamp(6, MAX_POSTS);
        let mut posts = Vec::with_capacity(MAX_POSTS);
        self.watch_posts(&mut posts, fort_h);
        let flanked = target >= 11 && self.gates.len() <= 2;
        self.gate_posts(&mut posts, 1 + usize::from(flanked));
        // What the towers and gates leave, less one post for the yard.
        let room = |posts: &[Post]| (MAX_POSTS - 1).saturating_sub(posts.len());
        let want = (1 + usize::from(target >= 12)).min(room(&posts));
        self.centre_posts(&mut posts, want);
        let want = (1 + usize::from(target >= 9) + usize::from(target >= 12)).min(room(&posts));
        self.hut_posts(&mut posts, want);
        let yard = target
            .saturating_sub(posts.len())
            .max(2)
            .min(MAX_POSTS.saturating_sub(posts.len()));
        self.yard_posts(&mut posts, yard);
        for post in &posts {
            self.mark_post(post);
        }
        self.posts = posts;
    }

    /// What a body at `floor` would stand on: what the plan builds there, or the natural surface
    /// the camp left as it found it.
    fn floor_material(&self, floor: [i32; 3]) -> Option<Material> {
        let [x, y, z] = floor;
        match self.plan.get(floor) {
            Some(m) => Some(*m),
            None if self.ground.contains([x, z])
                && y == self.g([x, z])
                && self.natural.get(x, z) == Some(y) =>
            {
                natural_top(self.mats.style)
            }
            None => None,
        }
    }

    /// Whether a body standing on `floor` would have room: a solid floor and two clear cells.
    fn is_post_floor(&self, floor: [i32; 3]) -> bool {
        let [x, y, z] = floor;
        self.floor_material(floor).is_some_and(|m| solid_floor(&m))
            && (1..=2).all(|k| {
                self.plan
                    .get([x, y + k, z])
                    .is_none_or(|m| m.is_air() || (k == 1 && litter(m)))
            })
    }

    /// How many of the four neighbours of `floor` are floors at the same height: a body there
    /// can walk somewhere.
    fn open_sides(&self, floor: [i32; 3]) -> usize {
        let [x, y, z] = floor;
        Dir::ALL
            .into_iter()
            .filter(|d| {
                let [nx, nz] = d.step([x, z], 1);
                self.is_post_floor([nx, y, nz])
            })
            .count()
    }

    /// The floor of open camp ground at column `c`: inside the wall at least `depth` columns
    /// from it, off the plateaus and not claimed by any other feature.
    fn open_ground(&self, c: [i32; 2], depth: i32) -> Option<[i32; 3]> {
        (self.ground.contains(c)
            && self.depth(c) >= depth
            && self.plateau_at.get(c) == 0
            && matches!(self.resv.get(c), Res::Free | Res::Path))
        .then(|| [c[0], self.g(c), c[1]])
    }

    /// The yaw byte for a guard at `from` facing `target`.
    fn face(&self, from: [i32; 2], target: [i32; 2]) -> u8 {
        yaw_byte((target[0] - from[0]) as f32, (target[1] - from[1]) as f32)
    }

    /// The yaw byte for a guard at `from` facing away from the camp's centre.
    fn face_out(&self, from: [i32; 2]) -> u8 {
        self.face(self.center, from)
    }

    /// Adds a post at `floor` if it keeps its distance from the others and has room, and at
    /// least `open_sides` of its neighbours are floors too.
    fn try_post(
        &self,
        posts: &mut Vec<Post>,
        role: PostRole,
        floor: [i32; 3],
        yaw: u8,
        open_sides: usize,
    ) -> bool {
        let apart = posts.iter().all(|p| {
            let (dx, dz) = (p.floor[0] - floor[0], p.floor[2] - floor[2]);
            dx * dx + dz * dz >= MIN_GAP_SQ
        });
        if !apart || !self.is_post_floor(floor) || self.open_sides(floor) < open_sides {
            return false;
        }
        posts.push(Post { floor, role, yaw });
        true
    }

    /// One post on each tower's platform, and some along the fortress walls' walkways.
    fn watch_posts(&mut self, posts: &mut Vec<Post>, fort_h: i32) {
        for t in 0..self.towers.len() {
            let (s, min, top) = (self.towers[t].s, self.towers[t].min, self.towers[t].top);
            let mut open: Vec<([i32; 2], usize)> = Vec::new();
            for dz in 0..s {
                for dx in 0..s {
                    let c = [min[0] + dx, min[1] + dz];
                    let floor = [c[0], top, c[1]];
                    if self.is_post_floor(floor) {
                        let inner = dx > 0 && dz > 0 && dx < s - 1 && dz < s - 1;
                        open.push((c, self.open_sides(floor) * 2 + usize::from(inner)));
                    }
                }
            }
            let best = open.iter().map(|o| o.1).max().unwrap_or(0);
            open.retain(|o| o.1 == best);
            if !open.is_empty() {
                let c = open[self.rng.int(0, open.len() as i32 - 1) as usize].0;
                self.try_post(
                    posts,
                    PostRole::Watch,
                    [c[0], top, c[1]],
                    self.face_out(c),
                    0,
                );
            }
        }
        let len = self.ring_len();
        let mut walls_left = 5usize.saturating_sub(posts.len()).min(2);
        for a in 0..self.fort_arcs.len() {
            let (full, center, span) = (
                self.fort_arcs[a].full,
                self.fort_arcs[a].center,
                self.fort_arcs[a].span,
            );
            let want = if full { (len / 36).max(2) } else { 1 }.min(walls_left);
            let mut made = 0;
            for _ in 0..30 {
                if made >= want {
                    break;
                }
                let j = if full {
                    self.rng.int(0, len as i32 - 1) as usize
                } else {
                    let shift = self.rng.int(-span, span);
                    self.wrap(center as i64 + i64::from(shift))
                };
                if self.fort[j] == 0 || self.in_gate[j] || self.fort_cut[j] > 0 {
                    continue;
                }
                let y = self.fortress_top(j, fort_h).1;
                let mut walk: Vec<[i32; 2]> = self
                    .inner_by_src
                    .get(&j)
                    .map(|cells| {
                        cells
                            .iter()
                            .copied()
                            .filter(|&c| self.depth(c) < self.fort[j] && y - self.g(c) >= 2)
                            .collect()
                    })
                    .unwrap_or_default();
                self.rng.shuffle(&mut walk);
                for c in walk {
                    if self.try_post(posts, PostRole::Watch, [c[0], y, c[1]], self.face_out(c), 1) {
                        made += 1;
                        walls_left -= 1;
                        break;
                    }
                }
            }
        }
    }

    /// Guards just inside each gate, on its axis first and then to either side, facing out.
    fn gate_posts(&mut self, posts: &mut Vec<Post>, per_gate: usize) {
        const AXIS: [(f32, f32); 5] = [(3.0, 0.0), (4.0, 0.0), (5.0, 0.0), (6.0, 0.0), (7.0, 0.0)];
        const FLANKS: [(f32, f32); 6] = [
            (4.0, 3.0),
            (4.0, -3.0),
            (5.0, 3.0),
            (5.0, -3.0),
            (6.0, 3.0),
            (6.0, -3.0),
        ];
        for gi in 0..self.gates.len() {
            let c = self.ring[self.gates[gi].i];
            let out = self.gates[gi].out;
            let yaw = yaw_byte(out[0], out[1]);
            let at = |(back, side): (f32, f32)| {
                [
                    (c[0] as f32 - out[0] * back - out[1] * side).round() as i32,
                    (c[1] as f32 - out[1] * back + out[0] * side).round() as i32,
                ]
            };
            let mut placed = 0;
            for offsets in [&AXIS[..], &FLANKS[..]] {
                if placed >= per_gate {
                    break;
                }
                for &o in offsets {
                    let Some(floor) = self.open_ground(at(o), 2) else {
                        continue;
                    };
                    if self.try_post(posts, PostRole::Gate, floor, yaw, 2) {
                        placed += 1;
                        break;
                    }
                }
            }
        }
    }

    /// Guards around the well, statue or arena, facing it.
    fn centre_posts(&mut self, posts: &mut Vec<Post>, want: usize) {
        let Some((at, need)) = self.centre.as_ref().map(|c| (c.at, c.need)) else {
            return;
        };
        let mut placed = 0;
        for _ in 0..40 {
            if placed >= want {
                break;
            }
            let (angle, r) = (self.rng.range(0.0, TAU), need + self.rng.range(1.5, 3.5));
            let c = [
                (at[0] as f32 + angle.cos() * r).round() as i32,
                (at[1] as f32 + angle.sin() * r).round() as i32,
            ];
            let Some(floor) = self.open_ground(c, 2) else {
                continue;
            };
            if self.try_post(posts, PostRole::Centre, floor, self.face(c, at), 2) {
                placed += 1;
            }
        }
    }

    /// One guard at each of `want` huts: at the door, or now and then inside a hut whose door
    /// stands open.
    fn hut_posts(&mut self, posts: &mut Vec<Post>, want: usize) {
        // Huts on a plateau top are up a ladder or a stair run: no way to walk to their door.
        let mut order: Vec<usize> = (0..self.huts.len())
            .filter(|&h| {
                let hut = &self.huts[h];
                self.plateau_at.get(hut.frame.at(hut.w / 2, hut.d / 2)) == 0
            })
            .collect();
        self.rng.shuffle(&mut order);
        let mut placed = 0;
        for h in order {
            if placed >= want {
                break;
            }
            let inside = self.hut_door_open(h) && self.rng.roll(0.35);
            if (inside && self.hut_inside_post(posts, h)) || self.hut_front_post(posts, h) {
                placed += 1;
            }
        }
    }

    /// The floor level inside hut `h`: its highest ground.
    fn hut_floor(&self, h: usize) -> i32 {
        let hut = &self.huts[h];
        (0..hut.d)
            .flat_map(|lz| (0..hut.w).map(move |lx| (lx, lz)))
            .map(|(lx, lz)| self.g(hut.frame.at(lx, lz)))
            .max()
            .unwrap_or(i32::MIN)
    }

    /// Whether a body can walk in through hut `h`'s door: a doorway or a door standing open, with
    /// level ground in front of it.
    fn hut_door_open(&self, h: usize) -> bool {
        let hut = &self.huts[h];
        if hut.kind != HutKind::Hut {
            return false;
        }
        let floor_y = self.hut_floor(h);
        let (door, front) = (
            hut.frame.at((hut.w - 1) / 2, hut.d - 1),
            hut.frame.at((hut.w - 1) / 2, hut.d),
        );
        self.plan
            .get([door[0], floor_y + 1, door[1]])
            .is_none_or(|m| m.is_air() || m.state_pairs().any(|(k, v)| k == "open" && v == "true"))
            && self.is_post_floor([front[0], floor_y, front[1]])
    }

    fn hut_inside_post(&mut self, posts: &mut Vec<Post>, h: usize) -> bool {
        let (frame, w, d) = (self.huts[h].frame, self.huts[h].w, self.huts[h].d);
        let floor_y = self.hut_floor(h);
        let mut cells: Vec<[i32; 2]> = (1..d - 1)
            .flat_map(|lz| (1..w - 1).map(move |lx| (lx, lz)))
            .map(|(lx, lz)| frame.at(lx, lz))
            .collect();
        self.rng.shuffle(&mut cells);
        cells.into_iter().any(|c| {
            let yaw = self.face(c, self.center);
            self.try_post(posts, PostRole::Hut, [c[0], floor_y, c[1]], yaw, 1)
        })
    }

    /// A post on the ground in front of hut `h`'s door, or at its sides.
    fn hut_front_post(&self, posts: &mut Vec<Post>, h: usize) -> bool {
        let (frame, w, d) = (self.huts[h].frame, self.huts[h].w, self.huts[h].d);
        let mid = (w - 1) / 2;
        [(mid, d), (mid - 1, d), (mid + 1, d), (mid, d + 1)]
            .into_iter()
            .map(|(lx, lz)| frame.at(lx, lz))
            .any(|c| {
                self.ground.contains(c)
                    && self.try_post(
                        posts,
                        PostRole::Hut,
                        [c[0], self.g(c), c[1]],
                        self.face(c, self.center),
                        2,
                    )
            })
    }

    /// Guards scattered over the open yard: each is the best of a few random spots, the one
    /// farthest from every post so far.
    fn yard_posts(&mut self, posts: &mut Vec<Post>, want: usize) {
        if self.interior.is_empty() {
            return;
        }
        for _ in 0..want {
            let mut best: Option<([i32; 3], i32)> = None;
            for _ in 0..3 {
                for _ in 0..8 {
                    let c = self.interior[self.rng.int(0, self.interior.len() as i32 - 1) as usize];
                    let Some(floor) = self.open_ground(c, 3) else {
                        continue;
                    };
                    let gap = posts
                        .iter()
                        .map(|p| {
                            let (dx, dz) = (p.floor[0] - floor[0], p.floor[2] - floor[2]);
                            dx * dx + dz * dz
                        })
                        .min()
                        .unwrap_or(i32::MAX);
                    if gap >= MIN_GAP_SQ
                        && best.is_none_or(|b| gap > b.1)
                        && self.is_post_floor(floor)
                        && self.open_sides(floor) >= 2
                    {
                        best = Some((floor, gap));
                    }
                }
                if best.is_some() {
                    break;
                }
            }
            if let Some((floor, _)) = best {
                let yaw = self.face([floor[0], floor[2]], self.center);
                posts.push(Post {
                    floor,
                    role: PostRole::Yard,
                    yaw,
                });
            }
        }
    }

    /// Writes the post's floor where the camp left natural ground unwritten (data only rides a
    /// cell the plan writes), clears clutter from above it and attaches the data.
    fn mark_post(&mut self, post: &Post) {
        let [x, y, z] = post.floor;
        let Some(floor) = self.floor_material(post.floor) else {
            return;
        };
        if self.plan.get(post.floor).is_none() {
            self.plan.set(post.floor, &floor);
        }
        if self.plan.get([x, y + 1, z]).is_some_and(litter) {
            self.plan.unset([x, y + 1, z]);
        }
        self.plan
            .data(post.floor, POST_MARKER, vec![post.role as u8, post.yaw]);
    }
}
