use petramond_world::biome::Biome;
use petramond_world::block::Block;
use petramond_world::chunk::{CHUNK_SX, CHUNK_SZ, SEA_LEVEL, WORLD_MAX_Y};
use petramond_world::mathh::IVec3;
#[cfg(test)]
use petramond_world::section::Section;

use super::super::biome::spec;
use super::super::biome::trees::{
    self, RuleStage, SpeciesTable, Territory, TreeProfile, TreeSupport, MAX_TREE_SPACING_RADIUS,
};
use super::super::rng::FeatureRng;
use super::tree::{redwood_base_trunk_contains, REDWOOD_BASE_SUPPORT_REACH};
#[cfg(test)]
use super::SectionSink;
use super::{FeatureCtx, FeatureField, TREELINE};

use crate::salts;

mod groves;
use groves::{GroveField, Window};

const BRANCH_REACH: i32 = 8;
const BRANCH_PER_TREE: (i32, i32) = (0, 2);

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct TreeCandidate {
    anchor: i32,
    biome: Biome,
    density: f32,
    pub(super) spacing_radius: i32,
    priority: u64,
}

#[inline]
fn tree_priority(seed: u32, wx: i32, wz: i32) -> u64 {
    FeatureRng::positional(seed, salts::TREE_PRIORITY, wx, 0, wz).next_u64()
}

#[inline]
fn tree_candidate_beats(
    lhs: TreeCandidate,
    lhs_wx: i32,
    lhs_wz: i32,
    rhs: TreeCandidate,
    rhs_wx: i32,
    rhs_wz: i32,
) -> bool {
    lhs.spacing_radius > rhs.spacing_radius
        || (lhs.spacing_radius == rhs.spacing_radius
            && (lhs.priority > rhs.priority
                || (lhs.priority == rhs.priority && (lhs_wz, lhs_wx) < (rhs_wz, rhs_wx))))
}

struct Selection<'p> {
    species: Option<&'p SpeciesTable>,
    density: f32,
    spacing_radius: i32,
    rule: Option<usize>,
}

#[derive(Copy, Clone)]
enum Slot {
    Unknown,
    Absent,
    Present(TreeCandidate),
}

pub(super) struct TreeCandidates {
    seed: u32,
    window: Window,
    groves: GroveField,
    slots: Box<[Slot]>,
}

impl TreeCandidates {
    pub(super) fn new(seed: u32, ox: i32, oz: i32) -> Self {
        let reach = super::MARGIN + MAX_TREE_SPACING_RADIUS;
        let window = Window {
            x_min: ox - reach,
            x_max: ox + CHUNK_SX as i32 + reach,
            z_min: oz - reach,
            z_max: oz + CHUNK_SZ as i32 + reach,
        };
        let cells = ((window.x_max - window.x_min) * (window.z_max - window.z_min)) as usize;
        Self {
            seed,
            window,
            groves: GroveField::new(seed, window),
            slots: vec![Slot::Unknown; cells].into_boxed_slice(),
        }
    }

    #[inline]
    fn slot_index(&self, wx: i32, wz: i32) -> usize {
        debug_assert!(
            self.window.contains(wx, wz),
            "tree candidate probe outside the placement window"
        );
        ((wz - self.window.z_min) * (self.window.x_max - self.window.x_min)
            + (wx - self.window.x_min)) as usize
    }

    pub(super) fn at(
        &mut self,
        field: &mut impl FeatureField,
        wx: i32,
        wz: i32,
    ) -> Option<TreeCandidate> {
        let i = self.slot_index(wx, wz);
        match self.slots[i] {
            Slot::Present(c) => Some(c),
            Slot::Absent => None,
            Slot::Unknown => {
                let candidate = self.compute(field, wx, wz);
                self.slots[i] = candidate.map_or(Slot::Absent, Slot::Present);
                candidate
            }
        }
    }

    fn compute(
        &mut self,
        field: &mut impl FeatureField,
        wx: i32,
        wz: i32,
    ) -> Option<TreeCandidate> {
        let seed = self.seed;
        let (surf, biome) = field.column_at(wx, wz);
        let anchor = surf;
        if anchor <= SEA_LEVEL || surf > TREELINE {
            return None;
        }

        let profile = trees::profile(biome);
        if anchor + profile.height_clearance >= WORLD_MAX_Y {
            return None;
        }

        let peak_density = profile.peak_density();
        if peak_density <= 0.0 {
            return None;
        }
        let mut rng = FeatureRng::positional(seed, salts::TREE_FEATURE, wx, 0, wz);
        let density_roll = rng.next_f32();
        if density_roll >= peak_density {
            return None;
        }

        let selection = self.select(profile, RuleStage::Candidate, field, wx, wz);
        if density_roll >= selection.density {
            return None;
        }

        match profile.support {
            TreeSupport::None => {}
            TreeSupport::RedwoodBase => {
                if !redwood_trunk_is_supported(field, wx, wz, anchor) {
                    return None;
                }
            }
        }

        let mut spacing_radius = selection.spacing_radius;
        if profile.deferred_rule_may_win(selection.rule) {
            spacing_radius = spacing_radius.max(profile.spacing_radius);
        }
        Some(TreeCandidate {
            anchor,
            biome,
            density: selection.density,
            spacing_radius,
            priority: tree_priority(seed, wx, wz),
        })
    }

    fn select<'p>(
        &mut self,
        profile: &'p TreeProfile,
        stage: RuleStage,
        field: &mut impl FeatureField,
        wx: i32,
        wz: i32,
    ) -> Selection<'p> {
        for (i, rule) in profile.rules.iter().enumerate() {
            if rule.territory.stage() > stage {
                continue;
            }
            let holds = match rule.territory {
                Territory::Grove(lattice) => self.groves.claims(&lattice, wx, wz),
                Territory::NearbyBiome { biome, radius } => {
                    biome_within(field, wx, wz, biome, radius)
                }
            };
            if holds {
                return Selection {
                    species: Some(&rule.species),
                    density: rule.density.unwrap_or(profile.density),
                    spacing_radius: rule.spacing_radius.unwrap_or(profile.spacing_radius),
                    rule: Some(i),
                };
            }
        }
        Selection {
            species: profile.species.as_ref(),
            density: profile.density,
            spacing_radius: profile.spacing_radius,
            rule: None,
        }
    }

    fn accepted_species<'p>(
        &mut self,
        profile: &'p TreeProfile,
        field: &mut impl FeatureField,
        wx: i32,
        wz: i32,
    ) -> &'p SpeciesTable {
        self.select(profile, RuleStage::Accepted, field, wx, wz)
            .species
            .expect("a rooted candidate's profile states a species table")
    }

    pub(super) fn spacing_allows(
        &mut self,
        candidate: TreeCandidate,
        field: &mut impl FeatureField,
        wx: i32,
        wz: i32,
    ) -> bool {
        for dz in -MAX_TREE_SPACING_RADIUS..=MAX_TREE_SPACING_RADIUS {
            for dx in -MAX_TREE_SPACING_RADIUS..=MAX_TREE_SPACING_RADIUS {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let nx = wx + dx;
                let nz = wz + dz;
                if let Some(other) = self.at(field, nx, nz) {
                    let spacing = candidate.spacing_radius.max(other.spacing_radius);
                    if dx.abs() > spacing || dz.abs() > spacing {
                        continue;
                    }
                    if tree_candidate_beats(other, nx, nz, candidate, wx, wz) {
                        return false;
                    }
                }
            }
        }
        true
    }
}

fn biome_within(
    field: &mut impl FeatureField,
    wx: i32,
    wz: i32,
    biome: Biome,
    radius: i32,
) -> bool {
    for ring in 1..=radius {
        for dz in -ring..=ring {
            for dx in -ring..=ring {
                if dx.abs().max(dz.abs()) != ring {
                    continue;
                }
                if field.column_at(wx + dx, wz + dz).1 == biome {
                    return true;
                }
            }
        }
    }
    false
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

/// Cubic per-section feature placement: run the origin loop into one 16³
/// [`Section`] through a [`SectionSink`]. Because each feature write predicates only
/// on its own cell, a tree straddling sections materialises identically from each
/// of them, with no neighbour buffer. `field` covers this section's column
/// (origin `ox,oz = section column origin`) plus the feature margin.
///
/// Production replays a recorded [`super::FeaturePlan`] instead; this is the
/// direct reference the plan's replay is proven against.
#[cfg(test)]
pub(super) fn place_features_section(
    section: &mut Section,
    field: &mut impl FeatureField,
    seed: u32,
) {
    let (ox, _oy, oz) = section.origin_world();
    let mut sink = SectionSink::new(section);
    let mut ctx = FeatureCtx::new(&mut sink);
    place_feature_origins(&mut ctx, field, seed, ox, oz);
}

pub(crate) fn place_feature_origins(
    ctx: &mut FeatureCtx,
    field: &mut impl FeatureField,
    seed: u32,
    ox: i32,
    oz: i32,
) {
    let margin = super::MARGIN;
    let mut candidates = TreeCandidates::new(seed, ox, oz);
    for wz in (oz - margin)..(oz + CHUNK_SZ as i32 + margin) {
        for wx in (ox - margin)..(ox + CHUNK_SX as i32 + margin) {
            let Some(candidate) = candidates.at(field, wx, wz) else {
                continue;
            };

            if !candidates.spacing_allows(candidate, field, wx, wz) {
                continue;
            }

            let mut rng = FeatureRng::positional(seed, salts::TREE_FEATURE, wx, 0, wz);
            let density_hit = rng.chance(candidate.density);
            debug_assert!(density_hit);
            let cf = candidates
                .accepted_species(trees::profile(candidate.biome), field, wx, wz)
                .pick(&mut rng);
            let origin = IVec3::new(wx, candidate.anchor, wz);
            if !cf
                .feature
                .is_anchored(&mut |sx, sz| field.surf_at(sx, sz), origin, rng)
            {
                continue;
            }
            cf.feature.generate(
                ctx,
                &mut |p: IVec3| p.y > field.surf_at(p.x, p.z),
                origin,
                &mut rng,
            );
            scatter_fallen_branches(ctx, field, seed, wx, wz);
        }
    }
}

fn scatter_fallen_branches(
    ctx: &mut FeatureCtx,
    field: &mut impl FeatureField,
    seed: u32,
    wx: i32,
    wz: i32,
) {
    let mut rng = FeatureRng::positional(seed, salts::TREE_BRANCH, wx, 0, wz);
    let count = rng.next_i32(BRANCH_PER_TREE.0, BRANCH_PER_TREE.1);
    for _ in 0..count {
        let dx = rng.next_i32(-BRANCH_REACH, BRANCH_REACH);
        let dz = rng.next_i32(-BRANCH_REACH, BRANCH_REACH);
        let variant = rng.next_i32(0, 2);
        if dx.abs() <= 1 && dz.abs() <= 1 {
            continue;
        }
        let (sx, sz) = (wx + dx, wz + dz);
        let (surf, biome) = field.column_at(sx, sz);
        if surf <= SEA_LEVEL {
            continue;
        }
        let skin = super::super::surface::SurfaceSystem.skin_block(
            &crate::surface::rule::SurfaceCtx {
                seed,
                wx: sx,
                wz: sz,
                y: surf,
                surf_y: surf,
                depth_from_top: 0,
            },
            spec(biome).surface,
        );
        if !super::vegetation::litter_ground(skin) {
            continue;
        }
        ctx.set_ground_litter(
            IVec3::new(sx, surf + 1, sz),
            match variant {
                0 => Block::FallenBranch,
                1 => Block::FallenBranch2,
                _ => Block::FallenBranch3,
            },
        );
    }
}

#[cfg(test)]
mod tests;
