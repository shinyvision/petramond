//! Edit-bounded relighting over the STORED section light cubes: the classic
//! decrease/increase BFS pair, so a torch placed or a door-like shape toggled
//! touches the few thousand cells whose value can change instead of re-flooding
//! every 48³ neighbourhood the edit dirties.
//!
//! # The fixpoint being maintained
//!
//! A full bake ([`super::bake`]) computes, for its kept section, the fixpoint
//! of: above-cover cells pinned at `SKY_FULL`, every emitter seeded with its
//! row colour (and its own emission stepping straight out, however opaque the
//! lamp), and every other cell the best neighbour value minus [`DECAY`] across
//! a pair of overlapping apertures — plus the lossless straight-down step of
//! full skylight through a direct-sky cell. Influence is bounded (a value
//! decays 2 per step from at most 30), so each section's bake equals that
//! fixpoint over the whole loaded world, and a clean baked section's cubes
//! hold exactly its cells' fixpoint values. That is what makes the stored
//! cubes a valid starting state.
//!
//! # The update
//!
//! For a set of changed cells (block, shape state, or custom aperture — the
//! sky-cover map unchanged):
//!
//! 1. **Decrease.** Each changed cell's stored value seeds a removal wave: a
//!    neighbour whose value is at most `v - DECAY` could have been derived
//!    through the removed cell, so it is zeroed and propagates the wave; a
//!    neighbour at `v - 1` or above holds light the removed cell cannot have
//!    supplied, so it joins the increase frontier. Above-cover cells are
//!    pinned and never removed; a removed emitter re-seeds its own emission.
//! 2. **Increase.** The frontier, the re-seeded emitters and the changed cells'
//!    neighbours relax outward exactly as the full flood does (same aperture
//!    rule, same escape rule for emitters, same lossless down step).
//!
//! Every value the removal wave keeps is achievable without any changed cell,
//! and the relaxation reaches everything the new world makes brighter, so the
//! result is the new fixpoint — bit for bit what a full rebake computes.
//! Block light runs the pair once per colour channel: channels decay
//! independently (the full flood's fused vector relaxation equals three scalar
//! floods), and removal must be channel-exact.
//!
//! # When it declines
//!
//! The waves never travel farther than [`CHANGE_REACH`] from a changed cell,
//! and the increase frontier sits one ring beyond. Every section within that
//! region must therefore hold trustworthy values: loaded, and either clean with
//! baked cubes, or fully opaque and free of changed cells (such a section's
//! light is implied: dark sky, and each emitter cell holding its own
//! emission). Anything else — an absent neighbour (the full bake reads it as
//! air, and nothing stores the light that air would carry), a section still
//! awaiting a bake, a region crossing the world's vertical range — declines,
//! and the caller falls back to full rebakes. So does a cover change: the
//! engine invalidates those through its sky-cover segments, which dirties the
//! region.

use std::collections::VecDeque;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::block::{Block, LIGHT_APERTURES_OPEN, LIGHT_CELL_DIRECT_SKY, LIGHT_CELL_SHAPED};
use crate::chunk::{section_idx, section_local, ChunkPos, SectionPos, SECTION_SIZE, SKY_FULL};
use crate::column::Column;
use crate::light::{LightRgb, DECAY};
use crate::mathh::IVec3;
use crate::section::Section;
use crate::world::WorldData;

use super::cube_region_changes;
use super::flood::{face_mask, DOWN};
use super::shape::{collect_light_overrides, resolve_word};

/// The farthest (L1, in cells) a single changed cell can move any light
/// value: the flood loses [`DECAY`] per step from at most `SKY_FULL`.
pub const CHANGE_REACH: i32 = (SKY_FULL / DECAY) as i32 - 1;

/// Region radius: the change reach plus the increase frontier's ring.
const REGION_REACH: i32 = CHANGE_REACH + 1;

/// Edits one relight takes on. A bulk edit past this (a schematic paste, a
/// blast) amortises better as full rebakes, which also run off-thread.
pub const MAX_EDITS: usize = 1024;

/// Neighbour steps in the flood's face order (`k` in [`face_mask`]'s
/// convention: the source's outgoing face is `k ^ 1`, the destination's
/// incoming face `k`).
const STEPS: [IVec3; 6] = crate::mathh::FACE_NEIGHBORS;

/// One section whose cubes an incremental relight changed.
pub struct RelitSection {
    pub pos: SectionPos,
    pub skylight: Arc<[u8]>,
    pub blocklight: Arc<[LightRgb]>,
    /// Which mesh-sampling regions changed (see [`super::cube_region_changes`]).
    pub mask: u32,
}

/// Whether `block`'s light apertures depend on its cell's stored state — the
/// ids for which a state write alone (a shape refine, a custom bake) is a
/// light change.
pub fn light_depends_on_state(block: Block) -> bool {
    crate::block::light_cells()
        .get(block.id() as usize)
        .is_some_and(|&w| w & LIGHT_CELL_SHAPED != 0)
}

/// Whether a change at `cell` could be relit incrementally against the loaded
/// sections as they stand (see the module doc's decline rules). A cheap
/// early answer; [`relight_edits`] re-checks the whole batch.
pub fn edit_relightable(sections: &FxHashMap<SectionPos, Arc<Section>>, cell: IVec3) -> bool {
    let mut positions = Vec::with_capacity(27);
    let Some(home) = SectionPos::from_world(cell.x, cell.y, cell.z) else {
        return false;
    };
    region(cell, &mut positions)
        && positions.iter().all(|pos| {
            sections
                .get(pos)
                .is_some_and(|s| source(s, *pos == home).is_some())
        })
}

/// Relight after `edits` — world cells whose light-relevant content (block,
/// shape state, custom aperture) changed since the stored cubes were baked,
/// with the sky-cover map unchanged. Returns the sections whose cubes changed,
/// or `None` when any edit's region cannot be trusted (see the module doc);
/// the caller then falls back to full rebakes for every edit.
pub fn relight_edits(
    sections: &FxHashMap<SectionPos, Arc<Section>>,
    columns: &FxHashMap<ChunkPos, Column>,
    edits: &[IVec3],
) -> Option<Vec<RelitSection>> {
    if edits.len() > MAX_EDITS {
        return None;
    }
    let mut region = Region::build(sections, columns, edits)?;
    region.relight_sky(edits);
    for ch in 0..3 {
        region.relight_block_channel(edits, ch);
    }
    region.finish()
}

/// L1 distance from a section-local cell to the nearest cell of the
/// neighbouring section at axis delta `d` (0 within this section).
#[inline]
fn axis_gap(local: usize, d: i32) -> i32 {
    match d {
        -1 => local as i32 + 1,
        1 => SECTION_SIZE as i32 - local as i32,
        _ => 0,
    }
}

/// Push every section within [`REGION_REACH`] of `cell`. `false` when the
/// cell or any of those sections lies outside the world's vertical range.
fn region(cell: IVec3, out: &mut Vec<SectionPos>) -> bool {
    let Some((center, lx, ly, lz)) = WorldData::split_world(cell.x, cell.y, cell.z) else {
        return false;
    };
    for dy in -1..=1 {
        for dz in -1..=1 {
            for dx in -1..=1 {
                if axis_gap(lx, dx) + axis_gap(ly, dy) + axis_gap(lz, dz) > REGION_REACH {
                    continue;
                }
                let cy = center.cy + dy;
                if !SectionPos::cy_in_range(cy) {
                    return false;
                }
                out.push(SectionPos::new(center.cx + dx, cy, center.cz + dz));
            }
        }
    }
    true
}

/// Where a region section's starting values come from.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Source {
    /// Its clean baked cubes.
    Stored,
    /// Fully opaque with no changed cell: no light enters an opaque cell, so
    /// its sky is dark and each emitter cell holds exactly its own emission.
    Implied,
}

fn source(section: &Section, holds_edit: bool) -> Option<Source> {
    if !section.light_dirty && section.has_baked_light() {
        Some(Source::Stored)
    } else if section.all_opaque() && !holds_edit {
        Some(Source::Implied)
    } else {
        None
    }
}

fn implied_cubes(section: &Section) -> (Box<[u8]>, Box<[LightRgb]>) {
    let sky = vec![0u8; crate::chunk::SECTION_VOLUME].into_boxed_slice();
    let mut block = vec![LightRgb::ZERO; crate::chunk::SECTION_VOLUME].into_boxed_slice();
    if section.has_light_emitters() {
        for (i, cell) in block.iter_mut().enumerate() {
            *cell = emission_of(section.block_at_idx(i));
        }
    }
    (sky, block)
}

#[inline]
fn emission_of(id: u16) -> LightRgb {
    let block = Block::from_id(id);
    if block.light_emission() == 0 {
        return LightRgb::ZERO;
    }
    let [r, g, b] = block.light_emission_rgb();
    LightRgb::new(r, g, b)
}

#[inline]
fn channel(c: LightRgb, ch: usize) -> u8 {
    c.channels()[ch]
}

#[inline]
fn with_channel(c: LightRgb, ch: usize, v: u8) -> LightRgb {
    let mut rgb = c.channels();
    rgb[ch] = v;
    LightRgb::new(rgb[0], rgb[1], rgb[2])
}

/// One region section's working state.
struct Work<'a> {
    pos: SectionPos,
    section: &'a Section,
    source: Source,
    /// World Y of the section's lowest cell layer.
    oy: i32,
    /// The owning column's sky-cover map (`[lz][lx]`).
    cover: &'a [i32],
    /// Per-cell aperture overrides (section index → aperture word), from the
    /// same producer the full bakes gather through.
    overrides: FxHashMap<u16, u32>,
    sky: Box<[u8]>,
    block: Box<[LightRgb]>,
}

/// The sections an incremental relight may read and write, addressed by world
/// cell through a cached last-section handle (the BFS walks neighbours, so the
/// section almost always repeats).
struct Region<'a> {
    works: Vec<Work<'a>>,
    index: FxHashMap<SectionPos, usize>,
    last: Option<(SectionPos, usize)>,
    cells: &'static [u32],
}

impl<'a> Region<'a> {
    fn build(
        sections: &'a FxHashMap<SectionPos, Arc<Section>>,
        columns: &'a FxHashMap<ChunkPos, Column>,
        edits: &[IVec3],
    ) -> Option<Self> {
        let edit_sections: FxHashSet<SectionPos> = edits
            .iter()
            .filter_map(|c| SectionPos::from_world(c.x, c.y, c.z))
            .collect();
        let mut positions = Vec::new();
        for &cell in edits {
            if !region(cell, &mut positions) {
                return None;
            }
        }
        let mut works = Vec::new();
        let mut index = FxHashMap::default();
        for pos in positions {
            if index.contains_key(&pos) {
                continue;
            }
            let section = sections.get(&pos)?;
            let source = source(section, edit_sections.contains(&pos))?;
            let cover = columns.get(&pos.chunk_pos())?.sky_cover_slice();
            let (sky, block) = match source {
                Source::Stored => (
                    section.skylight_arc()?.to_vec().into_boxed_slice(),
                    match section.blocklight_arc() {
                        Some(b) => b.to_vec().into_boxed_slice(),
                        None => {
                            vec![LightRgb::ZERO; crate::chunk::SECTION_VOLUME].into_boxed_slice()
                        }
                    },
                ),
                Source::Implied => implied_cubes(section),
            };
            let mut states = Vec::new();
            collect_light_overrides(section, section_idx, &mut states);
            // Later entries win, exactly as the gathers' densify does.
            let overrides = states
                .into_iter()
                .map(|s| (s.idx as u16, s.masks))
                .collect();
            index.insert(pos, works.len());
            works.push(Work {
                pos,
                section,
                source,
                oy: pos.origin_world().1,
                cover,
                overrides,
                sky,
                block,
            });
        }
        Some(Self {
            works,
            index,
            last: None,
            cells: crate::block::light_cells(),
        })
    }

    /// The region work and section index holding world cell `c`, or `None`
    /// outside the region.
    #[inline]
    fn locate(&mut self, c: IVec3) -> Option<(usize, usize)> {
        let (sp, lx, ly, lz) = WorldData::split_world(c.x, c.y, c.z)?;
        let w = match self.last {
            Some((last, w)) if last == sp => w,
            _ => {
                let w = *self.index.get(&sp)?;
                self.last = Some((sp, w));
                w
            }
        };
        Some((w, section_idx(lx, ly, lz)))
    }

    /// The cell's light word — [`super::shape::LightCells::word`]'s rule over
    /// the live section.
    #[inline]
    fn word(&self, w: usize, i: usize) -> u32 {
        let work = &self.works[w];
        resolve_word(self.cells, work.section.block_at_idx(i), || {
            work.overrides.get(&(i as u16)).copied()
        })
    }

    /// Whether the cell sits above its column's sky cover — pinned at full
    /// skylight, the flood's pre-fill.
    #[inline]
    fn above_cover(&self, w: usize, i: usize) -> bool {
        let work = &self.works[w];
        let (lx, ly, lz) = section_local(i);
        work.oy + ly as i32 > work.cover[lz * SECTION_SIZE + lx]
    }

    #[inline]
    fn emission(&self, w: usize, i: usize) -> LightRgb {
        emission_of(self.works[w].section.block_at_idx(i))
    }

    fn relight_sky(&mut self, edits: &[IVec3]) {
        let mut remove: VecDeque<(IVec3, u8)> = VecDeque::new();
        let mut add: VecDeque<IVec3> = VecDeque::new();
        for &c in edits {
            let Some((w, i)) = self.locate(c) else {
                continue;
            };
            let old = self.works[w].sky[i];
            if old > 0 {
                // A pinned cell keeps its value but still clears whatever it
                // fed through its old shape.
                if !self.above_cover(w, i) {
                    self.works[w].sky[i] = 0;
                }
                remove.push_back((c, old));
            }
            add.push_back(c);
            add.extend(STEPS.iter().map(|&d| c + d));
        }
        while let Some((p, v)) = remove.pop_front() {
            for d in STEPS {
                let n = p + d;
                let Some((w, i)) = self.locate(n) else {
                    continue;
                };
                let level = self.works[w].sky[i];
                if level == 0 {
                    continue;
                }
                if level + DECAY <= v && !self.above_cover(w, i) {
                    self.works[w].sky[i] = 0;
                    remove.push_back((n, level));
                } else {
                    add.push_back(n);
                }
            }
        }
        while let Some(p) = add.pop_front() {
            let Some((w, i)) = self.locate(p) else {
                continue;
            };
            let level = self.works[w].sky[i];
            if level <= DECAY {
                continue;
            }
            let fw = self.word(w, i);
            if fw & LIGHT_APERTURES_OPEN == 0 {
                continue;
            }
            for (k, &d) in STEPS.iter().enumerate() {
                let out = face_mask(fw, k ^ 1);
                if out == 0 {
                    continue;
                }
                let n = p + d;
                let Some((nw, ni)) = self.locate(n) else {
                    continue;
                };
                let tw = self.word(nw, ni);
                if face_mask(tw, k) & out == 0 {
                    continue;
                }
                let next = if level == SKY_FULL && k == DOWN && tw & LIGHT_CELL_DIRECT_SKY != 0 {
                    SKY_FULL
                } else {
                    level - DECAY
                };
                if self.works[nw].sky[ni] < next {
                    self.works[nw].sky[ni] = next;
                    add.push_back(n);
                }
            }
        }
    }

    /// Restore an emitter cell's own emission on channel `ch` and queue it to
    /// radiate again.
    fn reseed(&mut self, w: usize, i: usize, p: IVec3, ch: usize, add: &mut VecDeque<IVec3>) {
        let e = channel(self.emission(w, i), ch);
        if e == 0 {
            return;
        }
        let cell = &mut self.works[w].block[i];
        if channel(*cell, ch) < e {
            *cell = with_channel(*cell, ch, e);
        }
        add.push_back(p);
    }

    fn relight_block_channel(&mut self, edits: &[IVec3], ch: usize) {
        let mut remove: VecDeque<(IVec3, u8)> = VecDeque::new();
        let mut add: VecDeque<IVec3> = VecDeque::new();
        for &c in edits {
            let Some((w, i)) = self.locate(c) else {
                continue;
            };
            let old = channel(self.works[w].block[i], ch);
            if old > 0 {
                self.works[w].block[i] = with_channel(self.works[w].block[i], ch, 0);
                remove.push_back((c, old));
            }
            self.reseed(w, i, c, ch, &mut add);
            add.push_back(c);
            add.extend(STEPS.iter().map(|&d| c + d));
        }
        while let Some((p, v)) = remove.pop_front() {
            for d in STEPS {
                let n = p + d;
                let Some((w, i)) = self.locate(n) else {
                    continue;
                };
                let level = channel(self.works[w].block[i], ch);
                if level == 0 {
                    continue;
                }
                if level + DECAY <= v {
                    self.works[w].block[i] = with_channel(self.works[w].block[i], ch, 0);
                    remove.push_back((n, level));
                } else {
                    add.push_back(n);
                }
            }
            if let Some((w, i)) = self.locate(p) {
                self.reseed(w, i, p, ch, &mut add);
            }
        }
        while let Some(p) = add.pop_front() {
            let Some((w, i)) = self.locate(p) else {
                continue;
            };
            // The emitter's own emission steps straight out, gated only on
            // the neighbour accepting it — the full flood's seeding rule.
            let e = channel(self.emission(w, i), ch);
            if e > DECAY {
                for (k, &d) in STEPS.iter().enumerate() {
                    let n = p + d;
                    let Some((nw, ni)) = self.locate(n) else {
                        continue;
                    };
                    if face_mask(self.word(nw, ni), k) == 0 {
                        continue;
                    }
                    self.raise(nw, ni, n, ch, e - DECAY, &mut add);
                }
            }
            let level = channel(self.works[w].block[i], ch);
            if level <= DECAY {
                continue;
            }
            let fw = self.word(w, i);
            if fw & LIGHT_APERTURES_OPEN == 0 {
                continue;
            }
            for (k, &d) in STEPS.iter().enumerate() {
                let out = face_mask(fw, k ^ 1);
                if out == 0 {
                    continue;
                }
                let n = p + d;
                let Some((nw, ni)) = self.locate(n) else {
                    continue;
                };
                if face_mask(self.word(nw, ni), k) & out == 0 {
                    continue;
                }
                self.raise(nw, ni, n, ch, level - DECAY, &mut add);
            }
        }
    }

    #[inline]
    fn raise(
        &mut self,
        w: usize,
        i: usize,
        n: IVec3,
        ch: usize,
        next: u8,
        add: &mut VecDeque<IVec3>,
    ) {
        let cell = &mut self.works[w].block[i];
        if channel(*cell, ch) < next {
            *cell = with_channel(*cell, ch, next);
            add.push_back(n);
        }
    }

    /// The changed stored sections. An implied section can never change (no
    /// light enters an opaque cell); if one did, the region's premise was
    /// wrong and the whole relight is declined.
    fn finish(self) -> Option<Vec<RelitSection>> {
        let mut relit = Vec::new();
        for work in self.works {
            match work.source {
                Source::Implied => {
                    let (sky, block) = implied_cubes(work.section);
                    if sky != work.sky || block != work.block {
                        return None;
                    }
                }
                Source::Stored => {
                    let old_sky = work.section.skylight_arc()?;
                    let old_block = work.section.blocklight_arc();
                    let sky_same = old_sky[..] == work.sky[..];
                    let block_same = match &old_block {
                        Some(b) => b[..] == work.block[..],
                        None => work.block.iter().all(|c| c.is_dark()),
                    };
                    if sky_same && block_same {
                        continue;
                    }
                    let mask = cube_region_changes(Some(&old_sky[..]), &work.sky, SKY_FULL)
                        | cube_region_changes(old_block.as_deref(), &work.block, LightRgb::ZERO);
                    relit.push(RelitSection {
                        pos: work.pos,
                        skylight: Arc::from(work.sky),
                        blocklight: Arc::from(work.block),
                        mask,
                    });
                }
            }
        }
        Some(relit)
    }
}

#[cfg(test)]
mod tests;
