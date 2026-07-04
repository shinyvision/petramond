//! Neutral per-frame presentation data read from [`Game`].
//!
//! `Game` owns simulation state and transient client animation state. The app builds
//! this snapshot once per draw and passes it to presentation consumers, keeping render
//! wire structs out of `Game` while avoiding direct `Game` reads from those consumers.

use std::sync::Arc;

use glam::{IVec3, Quat, Vec3};

use crate::atlas::Tile;
use crate::block::{Block, RenderShape};
use crate::block_model::BlockModelKind;
use crate::door::DoorState;
use crate::furnace::Facing;
use crate::item::ItemType;
use crate::mob::Mob;

use super::Game;

pub const MAX_VISUAL_BOXES: usize = 3;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LocalVisualBoxes {
    pub boxes: [([f32; 3], [f32; 3]); MAX_VISUAL_BOXES],
    pub len: u8,
}

impl LocalVisualBoxes {
    #[inline]
    pub fn iter(self) -> impl Iterator<Item = ([f32; 3], [f32; 3])> {
        self.boxes.into_iter().take(self.len as usize)
    }
}

/// The block-break overlay to draw this frame: a cracked-texture overlay over
/// `block` at crack `stage` (0..=9, where 9 is fully cracked / about to break).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BreakOverlayView {
    pub block: IVec3,
    /// The cell-local visual box the crack hugs. `None` means an ordinary full cube.
    pub visual_box: Option<([f32; 3], [f32; 3])>,
    /// Cell-local visual boxes for partial blocks such as stairs.
    pub visual_boxes: Option<LocalVisualBoxes>,
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
    pub(crate) shorn: bool,
    pub(crate) ragdoll_pose: Option<Arc<[(Vec3, Quat)]>>,
}

pub(crate) struct GamePresentation<'a> {
    pub(crate) tick_alpha: f32,
    pub(crate) item_entities: &'a [DroppedItemPresentation],
    pub(crate) particles: &'a [ParticlePresentation],
    pub(crate) chests: &'a [ChestPresentation],
    pub(crate) doors: &'a [DoorPresentation],
    pub(crate) mobs: &'a [MobPresentation],
    pub(crate) held_item_light: (u8, u8, u8),
    pub(crate) break_overlay: Option<BreakOverlayView>,
}

#[derive(Default)]
pub(crate) struct GamePresentationScratch {
    item_entities: Vec<DroppedItemPresentation>,
    particles: Vec<ParticlePresentation>,
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
        self.collect_chests(game);
        self.collect_doors(game);
        self.collect_mobs(game, tick_alpha);

        GamePresentation {
            tick_alpha,
            item_entities: &self.item_entities,
            particles: &self.particles,
            chests: &self.chests,
            doors: &self.doors,
            mobs: &self.mobs,
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
                    shorn: mob.is_shorn(),
                    ragdoll_pose: mob.ragdoll_pose(tick_alpha).map(Into::into),
                }),
        );
    }
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
        let visual_boxes = if block_type.render_shape() == RenderShape::Stair {
            let (boxes, len) =
                crate::stair::local_boxes(game.world.stair_boxes_at(block.x, block.y, block.z));
            Some(LocalVisualBoxes { boxes, len })
        } else {
            None
        };
        BreakOverlayView {
            block,
            visual_box: if model.is_some() {
                None
            } else if visual_boxes.is_some() {
                None
            } else {
                game.world.selection_box_at(block.x, block.y, block.z)
            },
            visual_boxes,
            model,
            stage,
        }
    })
}
