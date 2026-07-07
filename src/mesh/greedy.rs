//! Greedy meshing: flat (uniform-across-corners) opaque cube faces deferred during
//! the builder's cell scan are 2D-merged per direction/slice into maximal tiled
//! quads, pixel-identical to the per-cell faces they replace.

use std::cell::RefCell;

use crate::chunk::{section_idx, SECTION_SIZE, SECTION_VOLUME};

use super::builder::face_axes;
use super::face::{Face, FACES};
use super::vertex::{
    pack_normal_code, pack_vertex, pack_vertex2, Vertex, UV_MODE_NONE, UV_MODE_SHIFT,
};

// Long greedy edges can meet subdivided neighbour faces as T-junctions; a tiny tangent-only
// overlap covers the rasterizer crack without moving the face plane or affecting water.
const GREEDY_FACE_OVERLAP: f32 = 1.0 / 1024.0;

/// A flat (uniform-across-corners) opaque cube face, recorded per (direction, cell) so a
/// run of identical adjacent faces can collapse into ONE tiled quad (greedy meshing). Only
/// faces whose four corners share the same AO + light + tint + tile qualify — then the merged
/// quad, drawn flat with its layer tiled W×H (REPEAT sampler), is pixel-identical to the
/// per-cell faces it replaces. `gen` matches the current build's generation for a live face
/// (a generation counter avoids re-zeroing the whole 6×4096 scratch every section — the fixed
/// cost that otherwise ~doubled meshing throughput; a stale entry from a prior build has an
/// old `gen` and reads as absent).
#[derive(Copy, Clone, PartialEq)]
pub(super) struct FlatFace {
    pub(super) gen: u32,
    pub(super) tile: u32,
    pub(super) ao: u32,
    pub(super) light6: u32,
    /// Second light channel (block light) — merges require BOTH channels equal, so
    /// a merged quad's `packed2` word is exact for every cell it replaces.
    pub(super) block6: u32,
    pub(super) tint: u32,
}

const FLAT_ABSENT: FlatFace = FlatFace {
    gen: 0,
    tile: 0,
    ao: 0,
    light6: 0,
    block6: 0,
    tint: 0,
};

/// Reused per-thread greedy-merge scratch: a `FlatFace` per (face direction 0..6, cell), a
/// per-slice merged-flag grid, the current build generation, and a deferred-face count per
/// direction (to skip merging directions that received none). Thread-local + reused so meshing
/// a section allocates nothing AND clears nothing (the `gen` bump retires the prior build).
pub(super) struct GreedyScratch {
    pub(super) faces: Vec<FlatFace>,
    pub(super) merged: Vec<bool>,
    pub(super) gen: u32,
    /// Deferred-face count per (direction, slice), so the merge pass scans only the few slices
    /// that actually received flat faces instead of all 6×16 (empty slices dominate — flat
    /// faces cluster in the surface/floor layers).
    pub(super) slice_counts: [u32; FACES.len() * SECTION_SIZE],
}

impl GreedyScratch {
    /// Retire the previous build and return this build's generation. No `faces` reset: a bumped
    /// `gen` makes every prior entry read as absent. Only allocates on first use per thread, and
    /// only re-zeroes on the (≈4-billion-build) `gen` wrap so a stale entry can't alias.
    pub(super) fn begin(&mut self) -> u32 {
        if self.faces.len() != FACES.len() * SECTION_VOLUME {
            self.faces = vec![FLAT_ABSENT; FACES.len() * SECTION_VOLUME];
        }
        if self.merged.len() != SECTION_SIZE * SECTION_SIZE {
            self.merged = vec![false; SECTION_SIZE * SECTION_SIZE];
        }
        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 {
            self.gen = 1;
            self.faces.fill(FLAT_ABSENT);
        }
        self.slice_counts = [0; FACES.len() * SECTION_SIZE];
        self.gen
    }
}

thread_local! {
    pub(super) static GREEDY: RefCell<GreedyScratch> = const {
        RefCell::new(GreedyScratch {
            faces: Vec::new(),
            merged: Vec::new(),
            gen: 0,
            slice_counts: [0; FACES.len() * SECTION_SIZE],
        })
    };
}

/// Greedy-merge every deferred flat face (in `scratch.faces`) into the fewest tiled quads and
/// push them to the opaque buffers. For each direction and each 16-cell slice, it 2D-merges
/// maximal rectangles of identical `FlatFace`s (extend width along U, then height along V),
/// emitting one quad per rectangle with `(W-1, H-1)` packed so the shader tiles its layer.
pub(super) fn emit_greedy_quads(
    scratch: &mut GreedyScratch,
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    ox: i32,
    oy: i32,
    oz: i32,
) {
    let cur = scratch.gen;
    let slice_counts = scratch.slice_counts;
    let key_at = |faces: &[FlatFace],
                  fi: usize,
                  n: usize,
                  s: usize,
                  ua: usize,
                  u: usize,
                  va: usize,
                  v: usize|
     -> FlatFace {
        let mut l = [0usize; 3];
        l[n] = s;
        l[ua] = u;
        l[va] = v;
        faces[fi * SECTION_VOLUME + section_idx(l[0], l[1], l[2])]
    };
    for (fi, face) in FACES.into_iter().enumerate() {
        let (n, ua, va) = face_axes(face);
        for s in 0..SECTION_SIZE {
            if slice_counts[fi * SECTION_SIZE + s] == 0 {
                continue; // no deferred faces in this slice — skip its 16×16 scan + fill.
            }
            scratch.merged.fill(false);
            for v in 0..SECTION_SIZE {
                for u in 0..SECTION_SIZE {
                    if scratch.merged[v * SECTION_SIZE + u] {
                        continue;
                    }
                    let key = key_at(&scratch.faces, fi, n, s, ua, u, va, v);
                    if key.gen != cur {
                        continue; // stale (prior build) or never written = absent.
                    }
                    // Extend the run along U while cells match and are unmerged.
                    let mut w = 1;
                    while u + w < SECTION_SIZE
                        && !scratch.merged[v * SECTION_SIZE + u + w]
                        && key_at(&scratch.faces, fi, n, s, ua, u + w, va, v) == key
                    {
                        w += 1;
                    }
                    // Extend along V while the whole W-wide row matches and is unmerged.
                    let mut h = 1;
                    'grow: while v + h < SECTION_SIZE {
                        for k in 0..w {
                            if scratch.merged[(v + h) * SECTION_SIZE + u + k]
                                || key_at(&scratch.faces, fi, n, s, ua, u + k, va, v + h) != key
                            {
                                break 'grow;
                            }
                        }
                        h += 1;
                    }
                    for dv in 0..h {
                        for du in 0..w {
                            scratch.merged[(v + dv) * SECTION_SIZE + u + du] = true;
                        }
                    }
                    let mut lmin = [0i32; 3];
                    let mut lmax = [0i32; 3];
                    lmin[n] = s as i32;
                    lmax[n] = s as i32 + 1;
                    lmin[ua] = u as i32;
                    lmax[ua] = (u + w) as i32;
                    lmin[va] = v as i32;
                    lmax[va] = (v + h) as i32;
                    let min = [
                        (ox + lmin[0]) as f32,
                        (oy + lmin[1]) as f32,
                        (oz + lmin[2]) as f32,
                    ];
                    let max = [
                        (ox + lmax[0]) as f32,
                        (oy + lmax[1]) as f32,
                        (oz + lmax[2]) as f32,
                    ];
                    push_greedy_quad(opaque, opaque_idx, face, min, max, key, w as u32, h as u32);
                }
            }
        }
    }
}

/// Push one greedy-merged quad: four flat vertices over the world box `[min,max]` with the
/// merge extents `(w,h)` packed into the overlay-tile bits (`(W-1) | (H-1)<<4`), which the
/// block shader reads to tile the layer. Uniform AO ⇒ no diagonal flip (default winding).
fn push_greedy_quad(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    face: Face,
    min: [f32; 3],
    max: [f32; 3],
    key: FlatFace,
    w: u32,
    h: u32,
) {
    let (min, max) = overlap_greedy_box(face, min, max);
    let corners = face.quad_box(min, max);
    let shade_idx = face.shade_idx();
    let wh = ((w - 1) & 0xF) | (((h - 1) & 0xF) << 4);
    let start = opaque.len() as u32;
    for (corner, p) in corners.into_iter().enumerate() {
        opaque.push(Vertex {
            pos: p,
            tint: key.tint,
            packed: pack_vertex(
                key.tile,
                corner as u32,
                shade_idx,
                wh,
                false,
                key.ao,
                key.light6,
            ) | (UV_MODE_NONE << UV_MODE_SHIFT),
            packed2: pack_vertex2(key.block6) | pack_normal_code(face.normal_code()),
        });
    }
    opaque_idx.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
}

#[inline]
fn overlap_greedy_box(face: Face, mut min: [f32; 3], mut max: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let (_, u, v) = face_axes(face);
    min[u] -= GREEDY_FACE_OVERLAP;
    max[u] += GREEDY_FACE_OVERLAP;
    min[v] -= GREEDY_FACE_OVERLAP;
    max[v] += GREEDY_FACE_OVERLAP;
    (min, max)
}
