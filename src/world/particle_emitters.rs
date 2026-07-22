//! Presentation collection for block-row particle emitters.
//!
//! A block's emitter is immutable row data (`blocks.json`). The world only answers
//! which loaded cells have such a row and where the emitter anchor is in world space;
//! the renderer derives transient particles from that. This keeps visual particles
//! out of simulation, saves, and the fixed tick.

use crate::block::{Block, ParticleEmitter, ParticleEmitterAnchor, ShapeFamily};
use crate::block_model::{self, BlockModelKind};
use crate::chunk::{section_local, SectionPos};
use crate::facing::Facing;
use crate::mathh::{voxel_at, IVec3, Vec3};
use crate::torch::POLE_HEIGHT;

use super::store::World;

impl World {
    /// Append `(world_origin, emitter, deterministic_seed, sky6, block6)` for every
    /// loaded block cell whose row declares a particle emitter. Visits only the
    /// maintained `particle_emitter_sections` index, then scans that section's dense
    /// block ids.
    pub fn collect_particle_emitters(&self, out: &mut Vec<(Vec3, ParticleEmitter, u64, u8, u8)>) {
        out.clear();
        for sp in &self.particle_emitter_sections {
            let Some(section) = self.sections.get(sp) else {
                continue;
            };
            if !section.has_particle_emitters() {
                continue;
            }
            let (ox, oy, oz) = section.origin_world();
            for (idx, &id) in section.blocks_slice().iter().enumerate() {
                let block = Block::from_id(id);
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
                    let sample = voxel_at(origin);
                    let (sky, block_light, _) =
                        self.dynamic_light_at_world(sample.x, sample.y, sample.z);
                    out.push((
                        origin,
                        emitter,
                        emitter_seed(*sp, idx as u16, block)
                            ^ (row_idx as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93),
                        sky,
                        block_light,
                    ));
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
    section: &crate::section::Section,
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
