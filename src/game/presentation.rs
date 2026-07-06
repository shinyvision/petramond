//! Neutral per-frame presentation data read from [`Game`].
//!
//! `Game` owns simulation state and transient client animation state. The app builds
//! this snapshot once per draw and passes it to presentation consumers, keeping render
//! wire structs out of `Game` while avoiding direct `Game` reads from those consumers.

use std::sync::Arc;

use glam::{IVec3, Quat, Vec3};

use crate::atlas::Tile;
use crate::block::{Block, BlockParticleEmitter, RenderShape};
use crate::block_model::BlockModelKind;
use crate::door::DoorState;
use crate::furnace::Facing;
use crate::item::ItemType;
use crate::mob::Mob;
use crate::stair::StairShape;

use super::Game;

/// The block-break overlay to draw this frame: a cracked-texture overlay over
/// `block` at crack `stage` (0..=9, where 9 is fully cracked / about to break).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BreakOverlayView {
    pub block: IVec3,
    /// The cell-local visual box the crack hugs. `None` means an ordinary full cube.
    pub visual_box: Option<([f32; 3], [f32; 3])>,
    /// A stair's corner-resolved shape: the crack rebuilds the exact
    /// quads the chunk mesher emitted for it (`mesh::stair::plane_quads`).
    pub stair_shape: Option<StairShape>,
    /// A slab cell's layer state: the crack rebuilds the exact per-layer quads
    /// the chunk mesher emitted (`mesh::slab::layer_quads`), so the decal is
    /// cropped to the occupied halves rather than stretched over them.
    pub slab_state: Option<crate::block_state::SlabState>,
    /// A model block cracks over its cell's actual model cubes, including the targeted
    /// cell's authored footprint offset and placed facing.
    pub model: Option<(BlockModelKind, [u8; 3], Facing)>,
    /// 0..=9 crack stage.
    pub stage: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ChestPresentation {
    pub(crate) pos: IVec3,
    pub(crate) facing: Facing,
    pub(crate) lid_progress: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DoorPresentation {
    pub(crate) pos: IVec3,
    pub(crate) state: DoorState,
    pub(crate) tiles: [Tile; 3],
    pub(crate) swing_progress: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DroppedItemPresentation {
    pub(crate) prev_pos: Vec3,
    pub(crate) pos: Vec3,
    pub(crate) item: ItemType,
    pub(crate) count: u8,
    pub(crate) prev_spin: f32,
    pub(crate) spin: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParticleAtlas {
    Block,
    Model,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ParticlePresentation {
    pub(crate) atlas: ParticleAtlas,
    pub(crate) pos: Vec3,
    pub(crate) uv_min: [f32; 2],
    pub(crate) uv_size: f32,
    pub(crate) tint: [f32; 3],
    pub(crate) warm: u8,
    pub(crate) alpha: f32,
    pub(crate) size: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ParticleEmitterPresentation {
    pub(crate) origin: Vec3,
    pub(crate) emitter: BlockParticleEmitter,
    pub(crate) seed: u64,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MobPresentation {
    pub(crate) id: u64,
    pub(crate) kind: Mob,
    pub(crate) prev_pos: Vec3,
    pub(crate) pos: Vec3,
    pub(crate) prev_yaw: f32,
    pub(crate) yaw: f32,
    pub(crate) prev_anim_time: f32,
    pub(crate) anim_time: f32,
    pub(crate) moving: bool,
    pub(crate) idle_anim: Option<u8>,
    pub(crate) prev_head_yaw: f32,
    pub(crate) head_yaw: f32,
    pub(crate) prev_head_pitch: f32,
    pub(crate) head_pitch: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
    pub(crate) hurt_flash: f32,
    pub(crate) dead: bool,
    pub(crate) shorn: bool,
    pub(crate) ragdoll_pose: Option<Arc<[(Vec3, Quat)]>>,
}

/// The local player's third-person body for this frame, or absent in first person.
/// Player movement/look are per-frame (already smooth), so unlike mobs there are
/// no prev/current pairs to interpolate.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct PlayerPresentation {
    /// Feet centre (model `y=0`).
    pub(crate) pos: Vec3,
    /// Body facing yaw (engine yaw space).
    pub(crate) body_yaw: f32,
    /// Head yaw relative to the body (radians) and look pitch.
    pub(crate) head_yaw: f32,
    pub(crate) head_pitch: f32,
    /// Seconds into the walk animation.
    pub(crate) anim_time: f32,
    /// Walk-pose blend weight (`0` standing … `1` full walk cycle).
    pub(crate) walk_weight: f32,
    /// Asleep in a bed: the body renders lying on its back, feet at `pos`,
    /// head toward `body_yaw`.
    pub(crate) sleeping: bool,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

pub(crate) struct GamePresentation<'a> {
    pub(crate) tick_alpha: f32,
    pub(crate) item_entities: &'a [DroppedItemPresentation],
    pub(crate) particles: &'a [ParticlePresentation],
    pub(crate) particle_emitters: &'a [ParticleEmitterPresentation],
    pub(crate) chests: &'a [ChestPresentation],
    pub(crate) doors: &'a [DoorPresentation],
    pub(crate) mobs: &'a [MobPresentation],
    pub(crate) player: Option<PlayerPresentation>,
    pub(crate) held_item_light: (u8, u8, u8),
    pub(crate) break_overlay: Option<BreakOverlayView>,
}

#[derive(Default)]
pub(crate) struct GamePresentationScratch {
    item_entities: Vec<DroppedItemPresentation>,
    particles: Vec<ParticlePresentation>,
    particle_emitter_rows: Vec<(Vec3, BlockParticleEmitter, u64, u8, u8)>,
    particle_emitters: Vec<ParticleEmitterPresentation>,
    chest_rows: Vec<(IVec3, Facing, u8, u8)>,
    door_rows: Vec<(IVec3, DoorState, [Tile; 3], u8, u8)>,
    chests: Vec<ChestPresentation>,
    doors: Vec<DoorPresentation>,
    mobs: Vec<MobPresentation>,
}

impl GamePresentationScratch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn snapshot<'a>(&'a mut self, game: &Game) -> GamePresentation<'a> {
        let tick_alpha = game.tick_alpha();
        self.collect_item_entities(game);
        self.collect_particles(game);
        self.collect_particle_emitters(game);
        self.collect_chests(game);
        self.collect_doors(game);
        self.collect_mobs(game, tick_alpha);

        GamePresentation {
            tick_alpha,
            item_entities: &self.item_entities,
            particles: &self.particles,
            particle_emitters: &self.particle_emitters,
            chests: &self.chests,
            doors: &self.doors,
            mobs: &self.mobs,
            player: collect_player(game),
            held_item_light: game.held_item_light(),
            break_overlay: mining_break_overlay(game),
        }
    }

    fn collect_item_entities(&mut self, game: &Game) {
        self.item_entities.clear();
        self.item_entities
            .extend(
                game.world
                    .item_entities()
                    .iter()
                    .map(|item| DroppedItemPresentation {
                        prev_pos: item.prev_pos,
                        pos: item.pos,
                        item: item.stack.item,
                        count: item.stack.count,
                        prev_spin: item.prev_spin,
                        spin: item.spin,
                        skylight: item.skylight,
                        blocklight: item.blocklight,
                    }),
            );
    }

    fn collect_particles(&mut self, game: &Game) {
        self.particles.clear();
        self.particles
            .extend(game.particles.particles().iter().map(|particle| {
                let (uv_min, uv_size) = particle.atlas_uv();
                ParticlePresentation {
                    atlas: if particle.model.is_some() {
                        ParticleAtlas::Model
                    } else {
                        ParticleAtlas::Block
                    },
                    pos: particle.pos,
                    uv_min,
                    uv_size,
                    tint: particle.tint,
                    warm: particle.warm,
                    alpha: particle.alpha(),
                    size: particle.render_size(),
                    skylight: particle.skylight,
                    blocklight: particle.blocklight,
                }
            }));
    }

    fn collect_particle_emitters(&mut self, game: &Game) {
        game.world
            .collect_particle_emitters(&mut self.particle_emitter_rows);
        self.particle_emitters.clear();
        self.particle_emitters
            .extend(self.particle_emitter_rows.iter().map(
                |&(origin, emitter, seed, skylight, blocklight)| ParticleEmitterPresentation {
                    origin,
                    emitter,
                    seed,
                    skylight,
                    blocklight,
                },
            ));
    }

    fn collect_chests(&mut self, game: &Game) {
        game.world.collect_chests(&mut self.chest_rows);
        self.chests.clear();
        self.chests.extend(
            self.chest_rows
                .iter()
                .map(|&(pos, facing, skylight, blocklight)| ChestPresentation {
                    pos,
                    facing,
                    lid_progress: game.chest_lid_angle(pos),
                    skylight,
                    blocklight,
                }),
        );
    }

    fn collect_doors(&mut self, game: &Game) {
        game.world.collect_doors(&mut self.door_rows);
        self.doors.clear();
        self.doors.extend(self.door_rows.iter().map(
            |&(pos, state, tiles, skylight, blocklight)| DoorPresentation {
                pos,
                state,
                tiles,
                swing_progress: game.door_swing_angle(pos),
                skylight,
                blocklight,
            },
        ));
    }

    fn collect_mobs(&mut self, game: &Game, tick_alpha: f32) {
        self.mobs.clear();
        self.mobs.extend(
            game.world
                .mobs()
                .instances()
                .iter()
                .map(|mob| MobPresentation {
                    id: mob.id(),
                    kind: mob.kind,
                    prev_pos: mob.prev_pos,
                    pos: mob.pos,
                    prev_yaw: mob.prev_yaw,
                    yaw: mob.yaw,
                    prev_anim_time: mob.prev_anim_time,
                    anim_time: mob.anim_time,
                    moving: mob.moving,
                    idle_anim: mob.idle_anim,
                    prev_head_yaw: mob.prev_head_yaw,
                    head_yaw: mob.head_yaw,
                    prev_head_pitch: mob.prev_head_pitch,
                    head_pitch: mob.head_pitch,
                    skylight: mob.skylight,
                    blocklight: mob.blocklight,
                    hurt_flash: mob.hurt_flash(tick_alpha),
                    dead: mob.is_dead(),
                    shorn: mob.is_shorn(),
                    ragdoll_pose: mob.ragdoll_pose(tick_alpha).map(Into::into),
                }),
        );
    }
}

/// The third-person body row, when the view is active. The body-yaw follow rule
/// keeps `yaw - body_yaw` within the head limit, so the relative head yaw needs
/// no re-wrapping here.
fn collect_player(game: &Game) -> Option<PlayerPresentation> {
    // The body draws only once the boom camera is actually placed — never on a
    // frame whose render camera is still the first-person eye (inside the head).
    if !game.third_person_enabled() || game.third_person.cam.is_none() {
        return None;
    }
    let (skylight, blocklight, _warm) = game.held_item_light();
    // The body shares the first-person camera's auto-step vertical easing (a
    // negative, settling lag) so stepping up a ledge glides instead of popping.
    let mut pos = game.player.pos;
    pos.y += game.camera_step_y_offset;
    let sleeping = game.sleep.is_some();
    if sleeping {
        // The sleeper stands at the bed-group CENTRE; the lying model's feet
        // anchor shifts back toward the foot end so the head lands on the pillow
        // (bed length 2, model ~1.85 → feet ~0.925 behind centre).
        let head_yaw = game.third_person.body_yaw;
        pos.x -= head_yaw.sin() * 0.925;
        pos.z -= head_yaw.cos() * 0.925;
    }
    Some(PlayerPresentation {
        pos,
        body_yaw: game.third_person.body_yaw,
        head_yaw: game.player.yaw - game.third_person.body_yaw,
        head_pitch: game.player.pitch,
        anim_time: game.third_person.anim_time,
        walk_weight: game.third_person.walk_weight,
        sleeping,
        skylight,
        blocklight,
    })
}

fn mining_break_overlay(game: &Game) -> Option<BreakOverlayView> {
    game.mining.overlay().map(|(block, stage)| {
        let model = match Block::from_id(game.world.chunk_block(block.x, block.y, block.z))
            .render_shape()
        {
            RenderShape::Model(kind) => Some((
                kind,
                game.world.model_offset_at(block.x, block.y, block.z),
                game.world.model_facing_at(block.x, block.y, block.z),
            )),
            _ => None,
        };
        let block_type = Block::from_id(game.world.chunk_block(block.x, block.y, block.z));
        let stair_shape = (block_type.render_shape() == RenderShape::Stair)
            .then(|| game.world.stair_shape_at(block.x, block.y, block.z));
        let slab_state = game.world.slab_state_if_slab(block);
        BreakOverlayView {
            block,
            visual_box: if model.is_some() || stair_shape.is_some() || slab_state.is_some() {
                None
            } else {
                game.world.selection_box_at(block.x, block.y, block.z)
            },
            stair_shape,
            slab_state,
            model,
            stage,
        }
    })
}
