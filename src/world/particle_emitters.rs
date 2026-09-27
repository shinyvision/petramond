#[cfg(test)]
use crate::world::ServerWorld;
use crate::world::{World, WorldSide};
use petramond_math::facing::Facing;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::view_volume::ViewVolume;
use petramond_world::block::{Block, CellView, ParticleEmitter, ParticleEmitterAnchor};
use petramond_world::block_model::{self, BlockModelKind};
use petramond_world::chunk::{section_local, SectionPos, SECTION_SIZE};
use petramond_world::light::BlockLight6;
use petramond_world::particle_emitters::particle_size;
use petramond_world::torch::{TorchPlacement, POLE_HEIGHT};

use super::store::WorldData;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlacedEmitter {
    pub origin: petramond_math::world_pos::WorldPos,
    pub emitter: ParticleEmitter,
    pub seed: u64,
    pub skylight: u8,
    pub blocklight: BlockLight6,
    pub floor_y: f32,
}

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
    let orbit = Vec3::new(e.spiral[0], 0.0, e.spiral[0]);
    Vec3::from_array(e.spawn_box) + travel + orbit + Vec3::splat(max_size + 0.05)
}

fn fall_reach(e: &ParticleEmitter) -> f32 {
    let max_life = e.lifetime[1].max(e.lifetime[0]);
    0.5 * e.gravity * max_life * max_life
}

fn max_emitter_particle() -> f32 {
    EMITTER_BOUNDS.current().0
}

fn max_emitter_reach() -> f32 {
    EMITTER_BOUNDS.current().1
}

static EMITTER_BOUNDS: petramond_world::content::Slot<(f32, f32)> =
    petramond_world::content::Slot::new(
        "particle emitter bounds",
        &[petramond_world::content::stage::BLOCKS],
        emitter_bounds,
    );

fn emitter_bounds(_: &petramond_world::content::ContentRegistry) -> Result<(f32, f32), String> {
    let mut size: f32 = 0.0;
    let mut reach: f32 = 0.0;
    for &block in Block::all() {
        let Some(rows) = block.particle_emitter() else {
            continue;
        };
        let footprint = block.model_kind().map_or(0.0, |kind| {
            let fp = block_model::def(kind).cells;
            2.0 * fp.iter().copied().max().unwrap_or(0) as f32
        });
        for row in rows {
            size = size.max(particle_size(row));
            let anchor = Vec3::from_array(row.origin).abs() + Vec3::from_array(row.offset).abs();
            let far = Vec3::splat(1.0 + footprint) + anchor + emitter_envelope(row);
            reach = reach.max(far.max_element());
        }
    }
    Ok((size, reach))
}

#[inline]
fn neighbour_id(
    world: &WorldData,
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

impl<S: WorldSide> World<S> {
    pub fn collect_particle_emitters(&self, view: &ViewVolume, out: &mut Vec<PlacedEmitter>) {
        out.clear();
        let reach = Vec3::splat(max_emitter_reach());
        let biggest = max_emitter_particle();
        let span = Vec3::splat(SECTION_SIZE as f32);
        let sec = SECTION_SIZE as i32;
        for sp in &self.data.particle_emitter_sections {
            let origin = petramond_math::world_pos::WorldPos::block_min(IVec3::new(
                sp.cx * sec,
                sp.cy * sec,
                sp.cz * sec,
            ));
            let (lo, hi) = (origin - reach, origin + span + reach);
            if !view.covers_a_pixel(lo, hi, biggest) || !view.aabb_visible(lo, hi) {
                continue;
            }
            let Some(section) = self.data.sections.get(sp) else {
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
                for (row_idx, &emitter) in rows.iter().enumerate() {
                    if let Some(side) = emitter.requires_open {
                        let q = side.support_cell(cell);
                        let over = Block::from_id(neighbour_id(&self.data, section, sp, q));
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
                    let (sky, block_light) = self
                        .data
                        .dynamic_light_at_world(sample.x, sample.y, sample.z);
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

impl<S: WorldSide> World<S> {
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
            if Block::from_id(self.data.chunk_block(top.x, y, top.z)).blocks_movement() {
                return (y + 1) as f32;
            }
        }
        f32::NEG_INFINITY
    }
}

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
            if TorchPlacement::owns(block) {
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

    #[test]
    fn the_envelope_covers_the_fall() {
        let e = drip();
        assert!(emitter_envelope(&e).y >= 20.0);
        let mut still = e;
        still.gravity = 0.0;
        assert!(emitter_envelope(&still).y < 1.0);
    }

    #[test]
    fn a_landing_emitter_finds_the_first_floor_under_its_anchor() {
        let mut w = ServerWorld::new(1, 1);
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
