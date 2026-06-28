//! Neutral per-frame presentation data read from [`Game`].
//!
//! `Game` owns simulation state and transient client animation state. The app builds
//! this snapshot once per draw and passes it to presentation consumers, keeping render
//! wire structs out of `Game` while avoiding direct `Game` reads from those consumers.

use glam::IVec3;

use crate::atlas::Tile;
use crate::block::{Block, RenderShape};
use crate::block_model::BlockModelKind;
use crate::door::DoorState;
use crate::entity::{DroppedItem, ParticleSystem};
use crate::furnace::Facing;

use super::Game;

/// The block-break overlay to draw this frame: a cracked-texture overlay over
/// `block` at crack `stage` (0..=9, where 9 is fully cracked / about to break).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BreakOverlayView {
    pub block: IVec3,
    /// The cell-local visual box the crack hugs. `None` means an ordinary full cube.
    pub visual_box: Option<([f32; 3], [f32; 3])>,
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
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DoorPresentation {
    pub(crate) pos: IVec3,
    pub(crate) state: DoorState,
    pub(crate) tiles: [Tile; 3],
    pub(crate) swing_progress: f32,
    pub(crate) skylight: u8,
}

pub(crate) struct GamePresentation<'a> {
    pub(crate) tick_alpha: f32,
    pub(crate) item_entities: &'a [DroppedItem],
    pub(crate) particles: &'a ParticleSystem,
    pub(crate) chests: &'a [ChestPresentation],
    pub(crate) doors: &'a [DoorPresentation],
    pub(crate) mobs: &'a [crate::mob::Instance],
    pub(crate) held_item_light: (u8, u8),
    pub(crate) break_overlay: Option<BreakOverlayView>,
}

#[derive(Default)]
pub(crate) struct GamePresentationScratch {
    chest_rows: Vec<(IVec3, Facing, u8)>,
    door_rows: Vec<(IVec3, DoorState, [Tile; 3], u8)>,
    chests: Vec<ChestPresentation>,
    doors: Vec<DoorPresentation>,
}

impl GamePresentationScratch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn snapshot<'a>(&'a mut self, game: &'a Game) -> GamePresentation<'a> {
        self.collect_chests(game);
        self.collect_doors(game);

        GamePresentation {
            tick_alpha: game.tick_alpha(),
            item_entities: game.world.item_entities(),
            particles: &game.particles,
            chests: &self.chests,
            doors: &self.doors,
            mobs: game.world.mobs().instances(),
            held_item_light: game.held_item_light(),
            break_overlay: mining_break_overlay(game),
        }
    }

    fn collect_chests(&mut self, game: &Game) {
        game.world.collect_chests(&mut self.chest_rows);
        self.chests.clear();
        self.chests
            .extend(
                self.chest_rows
                    .iter()
                    .map(|&(pos, facing, skylight)| ChestPresentation {
                        pos,
                        facing,
                        lid_progress: game.chest_lid_angle(pos),
                        skylight,
                    }),
            );
    }

    fn collect_doors(&mut self, game: &Game) {
        game.world.collect_doors(&mut self.door_rows);
        self.doors.clear();
        self.doors
            .extend(
                self.door_rows
                    .iter()
                    .map(|&(pos, state, tiles, skylight)| DoorPresentation {
                        pos,
                        state,
                        tiles,
                        swing_progress: game.door_swing_angle(pos),
                        skylight,
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
        BreakOverlayView {
            block,
            visual_box: if model.is_some() {
                None
            } else {
                game.world.selection_box_at(block.x, block.y, block.z)
            },
            model,
            stage,
        }
    })
}
