use crate::biome::Biome;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL};
use crate::mathh::IVec3;
use crate::section::Section;

use super::super::biome::{self, spec, TreeSupport};
use super::super::rng::FeatureRng;
use super::tree::{redwood_base_trunk_contains, REDWOOD_BASE_SUPPORT_REACH};
use super::{ChunkSink, FeatureCtx, FeatureField, SectionSink, TREELINE};

/// Salt distinguishing the tree-feature positional RNG stream from other users.
const FEATURE_SALT: u64 = 0x0000_7A3E_0AC0_FFEE;
/// Separate stream used only to break ties between nearby tree candidates.
const TREE_PRIORITY_SALT: u64 = 0x0000_7A3E_51AC_1EAF;

#[derive(Copy, Clone)]
pub(super) struct TreeCandidate {
    anchor: i32,
    biome: Biome,
    density: f32,
    pub(super) spacing_radius: i32,
    priority: u64,
}

#[inline]
fn tree_priority(seed: u32, wx: i32, wz: i32) -> u64 {
    FeatureRng::positional(seed, TREE_PRIORITY_SALT, wx, 0, wz).next_u64()
}

#[inline]
fn tree_candidate_beats(
    lhs_priority: u64,
    lhs_wx: i32,
    lhs_wz: i32,
    rhs_priority: u64,
    rhs_wx: i32,
    rhs_wz: i32,
) -> bool {
    lhs_priority > rhs_priority
        || (lhs_priority == rhs_priority && (lhs_wz, lhs_wx) < (rhs_wz, rhs_wx))
}

pub(super) fn tree_candidate_at(
    field: &mut impl FeatureField,
    seed: u32,
    wx: i32,
    wz: i32,
) -> Option<TreeCandidate> {
    // Anchor on the final region surface. Ocean and wet river-channel columns sit
    // at/below their waterline, so the water guard keeps trees off them.
    let (surf, biome) = field.column_at(wx, wz);
    let anchor = surf;
    if anchor <= SEA_LEVEL || surf > TREELINE {
        return None;
    }

    let tree = spec(biome).trees;
    // place_oak height guard (origin too low / too near the world top).
    if anchor < 1 || anchor + tree.height_clearance >= CHUNK_SY as i32 {
        return None;
    }

    let density = tree.density;
    if density <= 0.0 {
        return None;
    }

    let mut rng = FeatureRng::positional(seed, FEATURE_SALT, wx, 0, wz);
    if !rng.chance(density) {
        return None;
    }

    match tree.support {
        TreeSupport::None => {}
        TreeSupport::RedwoodBase => {
            if !redwood_trunk_is_supported(field, wx, wz, anchor) {
                return None;
            }
        }
    }

    Some(TreeCandidate {
        anchor,
        biome,
        density,
        spacing_radius: tree.spacing_radius,
        priority: tree_priority(seed, wx, wz),
    })
}

pub(super) fn tree_spacing_allows(
    candidate: TreeCandidate,
    field: &mut impl FeatureField,
    seed: u32,
    wx: i32,
    wz: i32,
) -> bool {
    for dz in -biome::MAX_TREE_SPACING_RADIUS..=biome::MAX_TREE_SPACING_RADIUS {
        for dx in -biome::MAX_TREE_SPACING_RADIUS..=biome::MAX_TREE_SPACING_RADIUS {
            if dx == 0 && dz == 0 {
                continue;
            }
            let nx = wx + dx;
            let nz = wz + dz;
            if let Some(other) = tree_candidate_at(field, seed, nx, nz) {
                let spacing = candidate.spacing_radius.max(other.spacing_radius);
                if dx.abs() > spacing || dz.abs() > spacing {
                    continue;
                }
                if tree_candidate_beats(other.priority, nx, nz, candidate.priority, wx, wz) {
                    return false;
                }
            }
        }
    }
    true
}

fn redwood_trunk_is_supported(
    field: &mut impl FeatureField,
    wx: i32,
    wz: i32,
    anchor: i32,
) -> bool {
    for dz in -REDWOOD_BASE_SUPPORT_REACH..=REDWOOD_BASE_SUPPORT_REACH {
        for dx in -REDWOOD_BASE_SUPPORT_REACH..=REDWOOD_BASE_SUPPORT_REACH {
            if !redwood_base_trunk_contains(dx, dz) {
                continue;
            }
            let support_surf = field.surf_at(wx + dx, wz + dz);
            if support_surf < anchor - 1 {
                return false;
            }
        }
    }
    true
}

/// Per-chunk feature placement (P4). Iterates feature origins across the chunk
/// plus a `MARGIN` border, in canonical (wz, wx) order, so a tree rooted in a
/// neighbour that reaches into this chunk is generated here too. Each origin
/// seeds its OWN positional RNG (`FeatureRng::positional`), so the per-biome
/// density roll, variant pick, and geometry are pure functions of (seed, wx, wz)
/// — independent of chunk and order. Candidate origins are then thinned by a
/// deterministic configured spacing rule. Features write in world coords and
/// are clipped to this chunk, so seams are continuous with no double-placement
/// and the old chunk-edge skip is gone.
pub(crate) fn place_features_with_field(
    chunk: &mut Chunk,
    field: &mut impl FeatureField,
    seed: u32,
) {
    let (ox, oz) = chunk.chunk_origin_world();
    let mut sink = ChunkSink::new(chunk);
    let mut ctx = FeatureCtx::new(&mut sink);
    place_feature_origins(&mut ctx, field, seed, ox, oz);
}

/// Cubic per-section feature placement: run the SAME origin loop into one 16³
/// [`Section`] through a [`SectionSink`]. Because each feature write predicates only
/// on its own cell, the section's voxels come out byte-identical to what the
/// whole-column [`place_features_with_field`] would write there — for the section's
/// own vertical slab, with no neighbour buffer. `field` covers this section's column
/// (origin `ox,oz = section column origin`) plus the feature margin.
pub(crate) fn place_features_section(
    section: &mut Section,
    field: &mut impl FeatureField,
    seed: u32,
) {
    let (ox, _oy, oz) = section.origin_world();
    let mut sink = SectionSink::new(section);
    let mut ctx = FeatureCtx::new(&mut sink);
    place_feature_origins(&mut ctx, field, seed, ox, oz);
}

/// The shared feature origin loop: iterate candidate origins across one column's XZ
/// footprint plus a `MARGIN` border, thin by the spacing rule, and generate each
/// accepted tree into `ctx` (whose sink clips to wherever the caller is writing —
/// a chunk or one section). `ox,oz` is the column's world origin.
fn place_feature_origins(
    ctx: &mut FeatureCtx,
    field: &mut impl FeatureField,
    seed: u32,
    ox: i32,
    oz: i32,
) {
    let margin = super::proto::MARGIN;
    for wz in (oz - margin)..(oz + CHUNK_SZ as i32 + margin) {
        for wx in (ox - margin)..(ox + CHUNK_SX as i32 + margin) {
            let Some(candidate) = tree_candidate_at(field, seed, wx, wz) else {
                continue;
            };

            if !tree_spacing_allows(candidate, field, seed, wx, wz) {
                continue;
            }

            // Recreate the accepted origin's stream and consume the already-proven
            // density roll so variant and geometry draws stay on the tree stream.
            let mut rng = FeatureRng::positional(seed, FEATURE_SALT, wx, 0, wz);
            let _density_hit = rng.chance(candidate.density);
            debug_assert!(_density_hit);
            let cf = (spec(candidate.biome).trees.picker)(&mut rng);
            let origin = IVec3::new(wx, candidate.anchor, wz);
            // Ground-anchoring gate, on the accepted origin only. Spacing-scan
            // neighbours are NOT gated, so an unanchorable neighbour still
            // suppresses candidates around it — deterministic either way, and
            // it keeps the gate's surface reads inside the candidate window
            // (origins lie within MARGIN of the chunk; the gate adds at most
            // MAX_TREE_SPACING_RADIUS). Every chunk replaying this origin
            // reaches the same verdict: the window values are world-anchored.
            if !cf
                .feature
                .is_anchored(&mut |sx, sz| field.surf_at(sx, sz), origin, rng)
            {
                continue;
            }
            cf.feature.generate(ctx, origin, &mut rng);
        }
    }
}
