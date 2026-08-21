//! Render-input view rows: the per-frame presentation snapshot contract
//! between the client game layer (which builds these from its replica and
//! animation state) and the renderer (which translates them into instance
//! buffers). Plain data — no renderer resources, no `Game` reads.

use std::sync::Arc;

use glam::{IVec3, Quat, Vec3};

use crate::RemotePlayerRender;
use petramond::mob::Mob;
use petramond::world::PlacedEmitter;
use petramond_math::facing::Facing;
use petramond_world::block_model::BlockModelKind;
use petramond_world::door::DoorState;
use petramond_world::item::ItemType;
use petramond_world::tile::Tile;

/// The block-break overlay to draw this frame: a cracked-texture overlay over
/// `block` at crack `stage` (0..=9, where 9 is fully cracked / about to break).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BreakOverlayView {
    pub block: IVec3,
    /// The cell-local visual box the crack hugs. `None` means an ordinary full cube.
    pub visual_box: Option<([f32; 3], [f32; 3])>,
    /// The cell's RESOLVED shape boxes, when its family has a box form: the
    /// crack traces THEM with cell-local UVs, so the decal hugs the real
    /// geometry (a stair's steps, a slab's occupied halves, a fence's post and
    /// rails, a chair's legs) instead of a box hanging in the cell's empty air.
    ///
    /// One field for every box family, because they all answer through the one
    /// box producer — a family is never named here.
    pub shape_boxes: Option<CrackBoxes>,
    /// A model block cracks over its cell's actual model cubes, including the targeted
    /// cell's authored footprint offset and placed facing.
    pub model: Option<(BlockModelKind, [u8; 3], Facing)>,
    /// 0..=9 crack stage.
    pub stage: u8,
}

/// The most cell-local boxes a crack traces (a chair is 7). A shape with more
/// truncates — the crack just covers fewer parts.
pub const MAX_CRACK_BOXES: usize = 16;

/// One cell-local box of a resolved shape, reduced to what a crack decal
/// needs: the box, and which of its faces the family actually emits. A face
/// the family never emits takes no destroy texture — that is what keeps a
/// ladder's crack off the wall behind it and a fence rail's end cap clean.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CrackBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Canonical face order (`+X, -X, +Y, -Y, +Z, -Z`) — `mesh::face::Face::ALL`.
    pub faces: [bool; 6],
}

/// A bounded, `Copy` snapshot of a cell's resolved boxes for its break crack
/// (the view stays `Copy`, so no per-frame allocation).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CrackBoxes {
    pub boxes: [CrackBox; MAX_CRACK_BOXES],
    pub len: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ChestPresentation {
    pub pos: IVec3,
    pub facing: Facing,
    pub lid_progress: f32,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DoorPresentation {
    pub pos: IVec3,
    pub state: DoorState,
    pub tiles: [Tile; 3],
    pub swing_progress: f32,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DroppedItemPresentation {
    pub prev_pos: Vec3,
    pub pos: Vec3,
    pub item: ItemType,
    pub variant: petramond_world::item::VariantId,
    pub count: u8,
    pub prev_spin: f32,
    pub spin: f32,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParticleAtlas {
    Block,
    Model,
    /// No atlas: a solid-color cube (an emitter-burst particle — water
    /// splash). `tint` IS the color; drawn alpha-blended with the looping
    /// emitter cubes instead of through the cutout fleck pipeline.
    Solid,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ParticlePresentation {
    pub atlas: ParticleAtlas,
    pub pos: Vec3,
    pub uv_min: [f32; 2],
    pub uv_size: [f32; 2],
    pub tint: [f32; 3],
    pub alpha: f32,
    pub size: f32,
    /// Vertical cube elongation (1 = a cube; ambient rain streaks stretch).
    /// Only the Solid atlas path honors it.
    pub stretch: f32,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MobPresentation {
    pub id: u64,
    pub kind: Mob,
    pub prev_pos: Vec3,
    pub pos: Vec3,
    pub prev_yaw: f32,
    pub yaw: f32,
    pub prev_anim_time: f32,
    pub anim_time: f32,
    pub moving: bool,
    pub idle_anim: Option<u8>,
    pub prev_head_yaw: f32,
    pub head_yaw: f32,
    pub prev_head_pitch: f32,
    pub head_pitch: f32,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
    pub hurt_flash: f32,
    pub dead: bool,
    pub shorn: bool,
    /// Replicated active particle-emitter bundle ids (client-local
    /// `particle_emitters.json` catalog ids).
    pub emitters: Vec<u8>,
    /// Named model animations as `(name, phase, weight)` — each layered by
    /// the renderer over the walk/idle/rest base pose at its own
    /// tick-interpolated PHASE (seconds into the clip; a paused oar's phase
    /// holds) and CLIENT-side blend weight (fading in toward 1, out toward 0).
    pub anims: Vec<(String, f32, f32)>,
    /// Body tint composed from the active bundles' `tint` values (white when
    /// none) — multiplied into the render tint like the hurt flash.
    pub emitter_tint: [f32; 3],
    pub ragdoll_pose: Option<Arc<[(Vec3, Quat)]>>,
}

/// The local player's third-person body for this frame, or absent in first person.
/// Player movement/look are per-frame (already smooth), so unlike mobs there are
/// no prev/current pairs to interpolate.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlayerPresentation {
    /// Feet centre (model `y=0`).
    pub pos: Vec3,
    /// Body facing yaw (engine yaw space).
    pub body_yaw: f32,
    /// Head yaw relative to the body (radians) and look pitch.
    pub head_yaw: f32,
    pub head_pitch: f32,
    /// Seconds into the walk animation.
    pub anim_time: f32,
    /// The body renders seated (legs forward): mounted on a mob seat, or
    /// pinned at a pose anchor whose pose is `sitting`. Anchor poses outside
    /// the known vocabulary render the rest pose (see `mount_renders_seated`).
    pub seated: bool,
    /// Walk-pose blend weight (`0` standing … `1` full walk cycle).
    pub walk_weight: f32,
    /// Sneak-stance blend weight (`0` upright … `1` fully crouched).
    pub sneak_weight: f32,
    /// Asleep in a bed: the body renders lying on its back, feet at `pos`,
    /// head toward `body_yaw`.
    pub sleeping: bool,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

/// One body whose FOOTSTEPS the client may sound this frame — the local player
/// and every visible remote, together, so a step is heard at whoever took it.
///
/// Built here rather than in `App` because deciding it needs the world: the
/// sound is the block UNDER the feet, and only presentation has the replica.
/// The cadence itself is `App`'s (see `tick_footstep_sounds`), like the mob
/// idle schedule.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FootstepSource {
    /// Stable cadence key: `0` is the local player, a remote is `1 + its
    /// PlayerId`. Ids are only compared, never sent.
    pub id: u64,
    /// Feet centre — where the sound plays, so a remote's steps arrive from
    /// their body and attenuate with distance like any other world sound.
    pub pos: Vec3,
    /// The block being walked on, or `None` when this body is not making
    /// footsteps at all: standing still, SNEAKING, airborne (the cell below is
    /// air), seated, asleep, or over an unloaded cell. `App` never re-decides
    /// this.
    pub ground: Option<petramond_world::block::Block>,
    /// Moving at a sprint — the gait `App` picks the step interval from.
    ///
    /// Derived from the body's ACTUAL horizontal speed on both sides rather
    /// than from a sprint key: a key held while the body is blocked, wading, or
    /// climbing must not quicken the cadence, and speed needs no new
    /// replication (a remote's velocity already ships for its walk blend).
    pub sprinting: bool,
}

/// One entity blob-shadow decal to draw this frame: a soft radial darkening
/// stamped on the ground under a mob, dropped item, or player body. The
/// gather (which owns the world) resolves the ground height, scales the
/// radius to the entity's footprint, and fades the strength with how far the
/// body sits above its ground; the renderer just stamps quads.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EntityShadow {
    /// Decal centre: the entity's x/z, `y` = the ground top surface under it.
    pub center: Vec3,
    /// World-space half-size of the square decal.
    pub radius: f32,
    /// Peak darkening at the centre (`0..=1`; 1 = fully black).
    pub strength: f32,
}

pub struct GamePresentation<'a> {
    pub tick_alpha: f32,
    pub item_entities: &'a [DroppedItemPresentation],
    pub particles: &'a [ParticlePresentation],
    /// Every emitter — block rows and mobs alike — whose particles are inside
    /// this frame's view volume, already culled by the gather.
    pub particle_emitters: &'a [PlacedEmitter],
    pub chests: &'a [ChestPresentation],
    /// Mod-submitted per-block draw sets with the light at their cell — the
    /// gather's own rows, handed on without a re-spelling copy.
    pub block_draws: &'a [petramond::world::draw::BlockDrawInstance],
    pub doors: &'a [DoorPresentation],
    pub mobs: &'a [MobPresentation],
    /// Every OTHER connected player's body + held item for this frame,
    /// already interpolated and posed — the render input rows themselves
    /// (`build_player_body` consumes `PlayerRenderInstance` directly, so no
    /// second translation buys anything).
    pub remote_players: &'a [RemotePlayerRender],
    /// Every body that could sound a footstep this frame (see
    /// [`FootstepSource`]) — INCLUDING bodies standing still, so `App` can
    /// retire the cadence state of players who left without a second list.
    pub footsteps: &'a [FootstepSource],
    pub player: Option<PlayerPresentation>,
    pub held_item_light: (u8, petramond_world::light::BlockLight6),
    /// Every break (crack) overlay to draw this frame: the LOCAL player's own
    /// mining target plus each visible remote's replicated one, capped at the
    /// `MAX_BREAK_OVERLAYS` nearest to the camera.
    pub break_overlays: &'a [BreakOverlayView],
    /// Blob shadows under entities (mobs, dropped items, bodies), already
    /// ground-resolved + view-culled by the gather.
    pub shadows: &'a [EntityShadow],
}
