//! Presentation collection for block-row particle emitters.
//!
//! A block's emitter is immutable row data (`blocks.json`). The world only answers
//! which loaded cells have such a row and where the emitter anchor is in world space;
//! the renderer derives transient particles from that. This keeps visual particles
//! out of simulation, saves, and the fixed tick.

use std::sync::LazyLock;

use petramond_world::block::{Block, ParticleEmitter, ParticleEmitterAnchor, ShapeFamily};
use petramond_world::block_model::{self, BlockModelKind};
use petramond_world::view_volume::ViewVolume;
use petramond_world::chunk::{section_local, SectionPos, SECTION_SIZE};
use petramond_math::facing::Facing;
use petramond_world::light::BlockLight6;
use petramond_math::math::{voxel_at, IVec3, Vec3};
use petramond_world::torch::POLE_HEIGHT;

use super::store::World;

/// One placed particle emitter to draw this frame: where it sits, the row that
/// describes it, its deterministic particle-schedule seed, and the light
/// sampled at the anchor. Purely presentation — nothing here is saved, ticked
/// or replicated.
///
/// Mobs produce these too (anchored at the body instead of a cell), and the
/// render layer consumes them unchanged, so this is the single row type the
/// whole emitter path speaks.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlacedEmitter {
    pub origin: Vec3,
    pub emitter: ParticleEmitter,
    pub seed: u64,
    pub skylight: u8,
    pub blocklight: BlockLight6,
}

/// Half-extents of the box a row's particles can ever occupy around their
/// anchor: the spawn box, plus how far the fastest particle travels in the
/// longest lifetime, plus the spiral's horizontal orbit, plus the biggest
/// cube's own size.
pub fn emitter_envelope(e: &ParticleEmitter) -> Vec3 {
    let max_life = e.lifetime[1].max(e.lifetime[0]);
    let max_size = e.size[1].max(e.size[0]);
    let velocity = Vec3::from_array(e.velocity);
    let jitter = Vec3::from_array(e.velocity_jitter);
    let travel = Vec3::new(
        velocity.x.abs() + jitter.x,
        velocity.y.abs() + jitter.y,
        velocity.z.abs() + jitter.z,
    ) * max_life;
    // The orbit is applied on top of the travelled position, around the
    // anchor's vertical axis, so it widens the box in X and Z.
    let orbit = Vec3::new(e.spiral[0], 0.0, e.spiral[0]);
    Vec3::from_array(e.spawn_box) + travel + orbit + Vec3::splat(max_size + 0.05)
}

/// How far, in blocks, any loaded block row's emitter particles can reach from
/// the CELL that carries the row — anchor displacement (a `local` anchor may
/// leave the cell, and a model block anchors anywhere in its footprint after
/// placement) plus the particle envelope.
///
/// Padding a section's box by this makes the section-level reject exact: a
/// section whose padded box misses the view cannot hold an emitter whose
/// particles are on screen. Derived from the loaded rows rather than guessed,
/// so a pack with a far-flung emitter widens it automatically.
fn max_emitter_reach() -> f32 {
    static REACH: LazyLock<f32> = LazyLock::new(|| {
        let mut reach: f32 = 0.0;
        for &block in Block::all() {
            let Some(rows) = block.particle_emitter() else {
                continue;
            };
            // A model anchors in FOOTPRINT space and its base is placed off the
            // authored-origin cell, so twice the footprint bounds both.
            let footprint = block.model_kind().map_or(0.0, |kind| {
                let fp = block_model::def(kind).cells;
                2.0 * fp.iter().copied().max().unwrap_or(0) as f32
            });
            for row in rows {
                let anchor =
                    Vec3::from_array(row.origin).abs() + Vec3::from_array(row.offset).abs();
                let far = Vec3::splat(1.0 + footprint) + anchor + emitter_envelope(row);
                reach = reach.max(far.max_element());
            }
        }
        reach
    });
    *REACH
}

impl World {
    /// Fill `out` with every loaded block cell whose row declares a particle
    /// emitter AND whose particles are inside `view`.
    ///
    /// Visits only the maintained `particle_emitter_sections` index, rejects a
    /// whole section on one box test before touching its dense block ids, and
    /// rejects an individual emitter before sampling light for it — so a
    /// cavern's worth of loaded emitters behind the camera costs a handful of
    /// box tests rather than a scan.
    pub fn collect_particle_emitters(&self, view: &ViewVolume, out: &mut Vec<PlacedEmitter>) {
        out.clear();
        let reach = Vec3::splat(max_emitter_reach());
        let span = Vec3::splat(SECTION_SIZE as f32);
        for sp in &self.particle_emitter_sections {
            let Some(section) = self.sections.get(sp) else {
                continue;
            };
            if !section.has_particle_emitters() {
                continue;
            }
            let (ox, oy, oz) = section.origin_world();
            let origin = Vec3::new(ox as f32, oy as f32, oz as f32);
            if !view.aabb_visible(origin - reach, origin + span + reach) {
                continue;
            }
            for &cell_idx in section.particle_emitter_cells() {
                let idx = cell_idx as usize;
                let block = Block::from_id(section.block_at_idx(idx));
                let Some(rows) = block.particle_emitter() else {
                    continue;
                };
                let (lx, ly, lz) = section_local(idx);
                let cell = IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32);
                // A referenced bundle may carry several rows (each with its own
                // anchor/offset); every row reports separately with a distinct
                // seed stream so sibling schedules don't pulse in lockstep.
                for (row_idx, &emitter) in rows.iter().enumerate() {
                    let origin = if let Some(kind) = block.model_kind() {
                        // A multi-cell model emits ONCE per placed group (from its
                        // authored-origin cell), at the FOOTPRINT-space anchor
                        // rotated by the placed facing — never once per occupied
                        // cell, which would wrap a 2×3×2 oven in twelve flames.
                        if section.model_offset(lx, ly, lz) != [0, 0, 0] {
                            continue;
                        }
                        let facing = section.model_facing(lx, ly, lz);
                        model_emitter_origin(emitter, kind, cell, facing)
                    } else {
                        let local = emitter_anchor_local(emitter, block, section, lx, ly, lz);
                        Vec3::new(cell.x as f32, cell.y as f32, cell.z as f32) + local
                    };
                    let envelope = emitter_envelope(&emitter);
                    if !view.aabb_visible(origin - envelope, origin + envelope) {
                        continue;
                    }
                    let sample = voxel_at(origin);
                    let (sky, block_light) =
                        self.dynamic_light_at_world(sample.x, sample.y, sample.z);
                    out.push(PlacedEmitter {
                        origin,
                        emitter,
                        seed: emitter_seed(*sp, cell_idx, block)
                            ^ (row_idx as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93),
                        skylight: sky,
                        blocklight: block_light,
                    });
                }
            }
        }
    }
}

/// World-space emitter origin for a model block: anchors resolve in FOOTPRINT
/// space (`local` origins are authored footprint coordinates, in blocks, and
/// may exceed the unit cell; `block_top`/`block_center` span the whole
/// footprint box), then rotate/translate through the same placement transform
/// the mesher uses, so the flame sits on the model whichever way it faces.
fn model_emitter_origin(
    emitter: ParticleEmitter,
    kind: BlockModelKind,
    origin_cell: IVec3,
    facing: Facing,
) -> Vec3 {
    let fp = block_model::def(kind).cells;
    let (fx, fy, fz) = (fp[0] as f32, fp[1] as f32, fp[2] as f32);
    let anchor = match emitter.anchor {
        ParticleEmitterAnchor::Local => Vec3::from_array(emitter.origin),
        ParticleEmitterAnchor::BlockCenter => Vec3::new(fx * 0.5, fy * 0.5, fz * 0.5),
        // TorchTop is torch-shaped-block data; on a model it degrades to the top.
        ParticleEmitterAnchor::BlockTop | ParticleEmitterAnchor::TorchTop => {
            Vec3::new(fx * 0.5, fy, fz * 0.5)
        }
    };
    let base = block_model::base_from_cell(origin_cell, kind, [0, 0, 0], facing);
    let m = block_model::placement_transform(base, kind, facing);
    m.transform_point3(anchor + Vec3::from_array(emitter.offset))
}

fn emitter_anchor_local(
    emitter: ParticleEmitter,
    block: Block,
    section: &petramond_world::section::Section,
    lx: usize,
    ly: usize,
    lz: usize,
) -> Vec3 {
    let base = match emitter.anchor {
        ParticleEmitterAnchor::BlockTop => Vec3::new(0.5, 1.0, 0.5),
        ParticleEmitterAnchor::BlockCenter => Vec3::splat(0.5),
        ParticleEmitterAnchor::Local => Vec3::from_array(emitter.origin),
        ParticleEmitterAnchor::TorchTop => {
            if block.shape_family() == ShapeFamily::Torch {
                section
                    .torch_placement(lx, ly, lz)
                    .model_transform()
                    .transform_point3(Vec3::new(0.0, POLE_HEIGHT, 0.0))
            } else {
                Vec3::new(0.5, 1.0, 0.5)
            }
        }
    };
    base + Vec3::from_array(emitter.offset)
}

fn emitter_seed(sp: SectionPos, local_idx: u16, block: Block) -> u64 {
    let mut x = (sp.cx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (sp.cy as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ (sp.cz as u64).wrapping_mul(0x94D0_49BB_1331_11EB)
        ^ ((local_idx as u64) << 8)
        ^ block.id() as u64;
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}
