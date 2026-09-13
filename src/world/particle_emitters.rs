//! Presentation collection for block-row particle emitters.
//!
//! A block's emitter is immutable row data (`blocks.json`). The world only answers
//! which loaded cells have such a row and where the emitter anchor is in world space;
//! the renderer derives transient particles from that. This keeps visual particles
//! out of simulation, saves, and the fixed tick.

use std::sync::LazyLock;

use petramond_math::facing::Facing;
use petramond_math::math::{IVec3, Vec3};
use petramond_world::block::{Block, ParticleEmitter, ParticleEmitterAnchor, ShapeFamily};
use petramond_world::block_model::{self, BlockModelKind};
use petramond_world::chunk::{section_local, SectionPos, SECTION_SIZE};
use petramond_world::light::BlockLight6;
use petramond_world::particle_emitters::particle_size;
use petramond_world::torch::POLE_HEIGHT;
use petramond_world::view_volume::ViewVolume;

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
    pub origin: petramond_math::world_pos::WorldPos,
    pub emitter: ParticleEmitter,
    pub seed: u64,
    pub skylight: u8,
    pub blocklight: BlockLight6,
    /// World Y of the surface a `lands` row's particles die on — the top of
    /// the first movement-blocking cell under the anchor within the row's
    /// fall reach — or `NEG_INFINITY` when nothing is there (and for every
    /// row that does not land).
    pub floor_y: f32,
}

/// Half-extents of the box a row's particles can ever occupy around their
/// anchor: the spawn box, plus how far the fastest particle travels in the
/// longest lifetime, plus the spiral's horizontal orbit, plus the biggest
/// cube's own size.
pub fn emitter_envelope(e: &ParticleEmitter) -> Vec3 {
    let max_life = e.lifetime[1].max(e.lifetime[0]);
    let max_size = particle_size(e);
    let velocity = Vec3::from_array(e.velocity);
    let jitter = Vec3::from_array(e.velocity_jitter);
    let travel = Vec3::new(
        velocity.x.abs() + jitter.x,
        velocity.y.abs() + jitter.y,
        velocity.z.abs() + jitter.z,
    ) * max_life
        + Vec3::new(0.0, fall_reach(e), 0.0);
    // The orbit is applied on top of the travelled position, around the
    // anchor's vertical axis, so it widens the box in X and Z.
    let orbit = Vec3::new(e.spiral[0], 0.0, e.spiral[0]);
    Vec3::from_array(e.spawn_box) + travel + orbit + Vec3::splat(max_size + 0.05)
}

/// How far a row's particles can fall under its `gravity` in their longest
/// life — the extra vertical travel a constant drift does not account for.
fn fall_reach(e: &ParticleEmitter) -> f32 {
    let max_life = e.lifetime[1].max(e.lifetime[0]);
    0.5 * e.gravity * max_life * max_life
}

/// The biggest particle any loaded row can put on screen. A section whose box
/// is too far for even THAT to cover a pixel holds nothing anyone can see, so
/// the whole section rejects on one compare.
fn max_emitter_particle() -> f32 {
    static SIZE: LazyLock<f32> = LazyLock::new(|| {
        let mut size: f32 = 0.0;
        for &block in Block::all() {
            let Some(rows) = block.particle_emitter() else {
                continue;
            };
            for row in rows {
                size = size.max(particle_size(row));
            }
        }
        size
    });
    *SIZE
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

/// The block at `q` read from `section` when `q` is one of its own cells, and
/// through the world store otherwise. The gather asks this once per candidate
/// cell of a `requires_open` row, and a body of them (a pool, a run) answers
/// from inside the section fifteen times out of sixteen — a dense index
/// instead of a store lookup per cell.
#[inline]
fn neighbour_id(
    world: &World,
    section: &petramond_world::section::Section,
    sp: &SectionPos,
    q: IVec3,
) -> u16 {
    let sec = SECTION_SIZE as i32;
    let (lx, ly, lz) = (q.x - sp.cx * sec, q.y - sp.cy * sec, q.z - sp.cz * sec);
    if (0..sec).contains(&lx) && (0..sec).contains(&ly) && (0..sec).contains(&lz) {
        section.block_at_idx(petramond_world::chunk::section_idx(
            lx as usize,
            ly as usize,
            lz as usize,
        ))
    } else {
        world.chunk_block(q.x, q.y, q.z)
    }
}

impl World {
    /// Fill `out` with every loaded block cell whose row declares a particle
    /// emitter AND whose particles are inside `view`.
    ///
    /// Visits only the maintained `particle_emitter_sections` index, rejects a
    /// whole section on two box tests before touching its dense block ids, and
    /// rejects an individual emitter before sampling light for it — so a
    /// cavern's worth of loaded emitters behind the camera costs a handful of
    /// box tests rather than a scan.
    ///
    /// The second section test is by SIZE: emitter particles are centimetres
    /// across, so most of a render distance is further than any of them can
    /// cover a pixel from. Without it the cost tracks how much emitting BLOCK
    /// is in the frustum — a lava pool's every cell, out to the fog — for
    /// particles nobody can see.
    pub fn collect_particle_emitters(&self, view: &ViewVolume, out: &mut Vec<PlacedEmitter>) {
        out.clear();
        let reach = Vec3::splat(max_emitter_reach());
        let biggest = max_emitter_particle();
        let span = Vec3::splat(SECTION_SIZE as f32);
        let sec = SECTION_SIZE as i32;
        for sp in &self.particle_emitter_sections {
            // The section's box comes from its POSITION, so both rejects run
            // before the store is even looked up — at a render distance most
            // of this index is far away, and the distance compare is the
            // cheapest thing here.
            let origin = petramond_math::world_pos::WorldPos::block_min(IVec3::new(
                sp.cx * sec,
                sp.cy * sec,
                sp.cz * sec,
            ));
            let (lo, hi) = (origin - reach, origin + span + reach);
            if !view.covers_a_pixel(lo, hi, biggest) || !view.aabb_visible(lo, hi) {
                continue;
            }
            let Some(section) = self.sections.get(sp) else {
                continue;
            };
            if !section.has_particle_emitters() {
                continue;
            }
            let (ox, oy, oz) = section.origin_world();
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
                    if let Some(side) = emitter.requires_open {
                        // "Open" is the surface of a body, not merely a cell
                        // nothing collides with: a cell of the SAME block is
                        // the body's inside, however walkable that block is,
                        // and a cell holding ANY fluid is another body's
                        // inside (embers never spark into the water over a
                        // pool). Fluids have no collision boxes at all.
                        let q = side.support_cell(cell);
                        let over = Block::from_id(neighbour_id(self, section, sp, q));
                        if over == block || over.blocks_movement() || over.fluid().is_some() {
                            continue;
                        }
                    }
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
                        petramond_math::world_pos::WorldPos::block_min(cell) + local
                    };
                    let envelope = emitter_envelope(&emitter);
                    let (lo, hi) = (origin - envelope, origin + envelope);
                    if !view.covers_a_pixel(lo, hi, particle_size(&emitter))
                        || !view.aabb_visible(lo, hi)
                    {
                        continue;
                    }
                    let sample = origin.block();
                    let (sky, block_light) =
                        self.dynamic_light_at_world(sample.x, sample.y, sample.z);
                    let floor_y = if emitter.lands {
                        self.emitter_floor_y(origin, &emitter)
                    } else {
                        f32::NEG_INFINITY
                    };
                    out.push(PlacedEmitter {
                        origin,
                        emitter,
                        seed: emitter_seed(*sp, cell_idx, block)
                            ^ (row_idx as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93),
                        skylight: sky,
                        blocklight: block_light,
                        floor_y,
                    });
                }
            }
        }
    }
}

impl World {
    /// The surface a `lands` row's particles die on: the top of the first
    /// movement-blocking cell under `origin` within the row's whole vertical
    /// reach (drift plus fall), or `NEG_INFINITY`. The scan starts one cell
    /// under the anchor's own so a tip hanging inside its cell does not land
    /// on itself.
    fn emitter_floor_y(
        &self,
        origin: petramond_math::world_pos::WorldPos,
        e: &ParticleEmitter,
    ) -> f32 {
        let max_life = e.lifetime[1].max(e.lifetime[0]);
        let reach = (e.velocity[1].abs() + e.velocity_jitter[1]) * max_life + fall_reach(e);
        let top = origin.block();
        let bottom = top.y - reach.ceil() as i32 - 1;
        for y in (bottom..top.y).rev() {
            if Block::from_id(self.chunk_block(top.x, y, top.z)).blocks_movement() {
                return (y + 1) as f32;
            }
        }
        f32::NEG_INFINITY
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
) -> petramond_math::world_pos::WorldPos {
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
    let m = block_model::placement_transform(kind, facing);
    petramond_math::world_pos::WorldPos::block_min(base)
        + m.transform_point3(anchor + Vec3::from_array(emitter.offset))
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

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::world_pos::WorldPos;
    use petramond_world::chunk::{Chunk, ChunkPos};

    fn drip() -> ParticleEmitter {
        serde_json::from_str(
            r#"{"rate": 1, "lifetime": [2, 2], "size": [0.05, 0.05], "alpha": [1, 1],
                "color": [[0,0,1],[0,0,1]], "gravity": 10, "lands": true}"#,
        )
        .expect("row parses")
    }

    /// The envelope must include the fall, or a section's cull box misses
    /// drops that are on screen.
    #[test]
    fn the_envelope_covers_the_fall() {
        let e = drip();
        // 0.5 · 10 · 2² = 20 blocks of fall in the longest life.
        assert!(emitter_envelope(&e).y >= 20.0);
        let mut still = e;
        still.gravity = 0.0;
        assert!(emitter_envelope(&still).y < 1.0);
    }

    /// A landing row's floor is the top of the first movement-blocking cell
    /// under the anchor within its reach; a walk-through plant is not a
    /// floor, and nothing in reach is no floor at all.
    #[test]
    fn a_landing_emitter_finds_the_first_floor_under_its_anchor() {
        let mut w = World::new(1, 1);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let origin = WorldPos::new(4.5, 80.0, 4.5);
        let e = drip();
        assert_eq!(w.emitter_floor_y(origin, &e), f32::NEG_INFINITY, "open air");
        w.set_block_world(4, 76, 4, Block::ShortGrass);
        assert_eq!(
            w.emitter_floor_y(origin, &e),
            f32::NEG_INFINITY,
            "grass is no floor"
        );
        w.set_block_world(4, 74, 4, Block::Stone);
        assert_eq!(w.emitter_floor_y(origin, &e), 75.0);
        w.set_block_world(4, 78, 4, Block::Stone);
        assert_eq!(
            w.emitter_floor_y(origin, &e),
            79.0,
            "the nearest floor wins"
        );
    }
}
