//! Render-input view rows: the per-frame presentation snapshot contract
//! between the client game layer (which builds these from its replica and
//! animation state) and the renderer (which translates them into instance
//! buffers). Plain data — no renderer resources, no `Game` reads.

use std::sync::Arc;

use glam::{IVec3, Quat, Vec3};

use petramond::mob::Mob;
use petramond::world::PlacedEmitter;
use petramond_math::math::Tilt;
use petramond_world::item::ItemType;

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
    /// A model block cracks over the model's OWN drawn triangles (the decal
    /// pass re-draws them), masked to this world-space outline box — so the
    /// whole piece cracks as one object and nothing is cracked in mid-air.
    pub model: Option<ModelCrack>,
    /// 0..=9 crack stage.
    pub stage: u8,
}

/// The world outline box of a cracked bbmodel block: the rotated-footprint
/// `base` cell plus the model's tight bounds relative to it, exactly as
/// [`WorldData::model_outline_box`](petramond_world::world::WorldData::model_outline_box)
/// answers. The decal pass masks the model's own geometry to this box, so a
/// multi-cell piece cracks as ONE object however many cells it spans.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ModelCrack {
    pub base: IVec3,
    pub min: [f32; 3],
    pub max: [f32; 3],
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
    /// The box's rotation off the axis grid, so the crack hugs a tilted
    /// plate where it is drawn.
    pub pose: Option<petramond_world::block::BoxPose>,
}

/// A bounded, `Copy` snapshot of a cell's resolved boxes for its break crack
/// (the view stays `Copy`, so no per-frame allocation).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CrackBoxes {
    pub boxes: [CrackBox; MAX_CRACK_BOXES],
    pub len: u8,
}

/// One animated block this frame: the world gather's row (position, block,
/// the pose its cell's state gives it, light) plus the client's linear open
/// progress, which the scene eases.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BlockEntityPresentation {
    pub block: petramond::world::animated_block::AnimatedBlock,
    pub open_progress: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DroppedItemPresentation {
    pub prev_pos: petramond_math::world_pos::WorldPos,
    pub pos: petramond_math::world_pos::WorldPos,
    pub item: ItemType,
    pub variant: petramond_world::item::VariantId,
    pub count: u8,
    pub prev_spin: f32,
    pub spin: f32,
    /// `[yaw, pitch, speed]` of a flying or lodged item, previous and
    /// current tick; `None` for a loose stack, which spins.
    pub prev_flight: Option<[f32; 3]>,
    pub flight: Option<[f32; 3]>,
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
    /// Textured, double-sided quad half-axes; None keeps the ordinary cube.
    pub quad_axes: Option<[Vec3; 2]>,
    pub atlas: ParticleAtlas,
    pub pos: petramond_math::world_pos::WorldPos,
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

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MobPresentation {
    pub id: u64,
    pub kind: Mob,
    pub prev_pos: petramond_math::world_pos::WorldPos,
    pub pos: petramond_math::world_pos::WorldPos,
    pub prev_yaw: f32,
    pub yaw: f32,
    /// Body tilt inside the yaw, previous and current tick, blended like the
    /// position.
    pub prev_tilt: Tilt,
    pub tilt: Tilt,
    pub prev_anim_time: f32,
    pub anim_time: f32,
    pub moving: bool,
    pub idle_anim: Option<u8>,
    /// The active gait's eased-in weight, and the gaits still fading out as a
    /// range into [`MobArena::gait_fades`]: a body never snaps between gaits.
    pub gait_weight: f32,
    pub gait_fades: crate::ArenaRange,
    pub prev_head_yaw: f32,
    pub head_yaw: f32,
    pub prev_head_pitch: f32,
    pub head_pitch: f32,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
    pub hurt_flash: f32,
    pub dead: bool,
    pub shorn: bool,
    /// Named model animation layers as a range into [`MobArena::anims`] —
    /// each layered by the renderer over the walk/idle/rest base pose at its
    /// own tick-interpolated PHASE (seconds into the clip; a paused oar's
    /// phase holds) and CLIENT-side blend weight (fading in toward 1, out
    /// toward 0).
    pub anims: crate::ArenaRange,
    /// Body tint composed from the active bundles' `tint` values (white when
    /// none) — multiplied into the render tint like the hurt flash.
    pub emitter_tint: [f32; 3],
    /// Body self-lighting from the active bundles (`0..=1`, strongest wins).
    pub emitter_self_lit: f32,
    /// The dying body's per-bone ragdoll pose, already interpolated for this
    /// frame, as a range into [`MobArena::ragdoll`]; `None` for a live mob.
    pub ragdoll_pose: Option<crate::ArenaRange>,
    /// The items drawn in the species' main and off hand bones.
    pub held: [Option<petramond_world::item::ItemType>; 2],
}

/// A mob animation name, interned once per session by [`AnimInterner`] where
/// the replicated row carries it, so no per-frame path clones or compares a
/// name. Resolved through the session's [`AnimNames`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnimId(pub u32);

/// A session's interned animation names, indexed by [`AnimId`]. Append-only
/// and shared: a clone is a refcount bump, and a holder keeps its copy current
/// with [`adopt`](Self::adopt) — a pointer compare on every frame the table
/// did not grow.
#[derive(Clone, Debug, Default)]
pub struct AnimNames(Arc<Vec<Arc<str>>>);

impl AnimNames {
    pub fn get(&self, id: AnimId) -> Option<&Arc<str>> {
        self.0.get(id.0 as usize)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Take `other`'s table when it is a different one (grown, or another
    /// session's).
    pub fn adopt(&mut self, other: &AnimNames) {
        if !Arc::ptr_eq(&self.0, &other.0) {
            self.0 = Arc::clone(&other.0);
        }
    }
}

/// Interns animation names into one session's [`AnimNames`]. Owned by the
/// session's entity replica, so a new session starts a fresh table and no
/// name state is process-global.
#[derive(Debug, Default)]
pub struct AnimInterner {
    names: AnimNames,
    ids: rustc_hash::FxHashMap<Arc<str>, AnimId>,
}

impl AnimInterner {
    /// `name`'s id, adding it to the table the first time it is seen. A new
    /// name copies the table's pointer list only while a reader still holds
    /// the previous table.
    pub fn intern(&mut self, name: &str) -> AnimId {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = AnimId(self.names.len() as u32);
        let name: Arc<str> = name.into();
        Arc::make_mut(&mut self.names.0).push(Arc::clone(&name));
        self.ids.insert(name, id);
        id
    }

    pub fn names(&self) -> &AnimNames {
        &self.names
    }
}

/// One gait a mob body eased out of and is still fading.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GaitFade {
    pub clip: crate::GaitClip,
    /// The phase the gait holds while it fades (seconds into the clip).
    pub phase: f32,
    pub weight: f32,
}

/// One named animation layer on a mob body this frame.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct AnimLayer {
    pub anim: AnimId,
    /// Seconds into the clip.
    pub phase: f32,
    /// Blend weight, `0..=1`.
    pub weight: f32,
}

/// Every mob's variable-length rows for one frame, back to back: each
/// [`MobPresentation`] and [`MobRenderInstance`](crate::MobRenderInstance)
/// addresses its own by [`ArenaRange`](crate::ArenaRange), so a mob row stays
/// a plain `Copy` value with no per-entity allocation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MobArena {
    pub gait_fades: Vec<GaitFade>,
    pub anims: Vec<AnimLayer>,
    /// Ragdoll bones as `(rest-pivot position, rotation delta)` in model space.
    pub ragdoll: Vec<(Vec3, Quat)>,
}

impl MobArena {
    pub fn clear(&mut self) {
        self.gait_fades.clear();
        self.anims.clear();
        self.ragdoll.clear();
    }

    /// Replace this arena's rows with `other`'s, reusing capacity: ranges
    /// into `other` then address the same rows here.
    pub fn copy_from(&mut self, other: &MobArena) {
        self.clear();
        self.gait_fades.extend_from_slice(&other.gait_fades);
        self.anims.extend_from_slice(&other.anims);
        self.ragdoll.extend_from_slice(&other.ragdoll);
    }
}

/// One entity blob-shadow decal to draw this frame: a soft radial darkening
/// stamped on the ground under a mob, dropped item, or player body. The
/// gather (which owns the world) resolves the ground height, scales the
/// radius to the entity's footprint, and fades the strength with how far the
/// body sits above its ground; the renderer just stamps quads.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EntityShadow {
    /// Decal centre: the entity's x/z, `y` = the ground top surface under it.
    pub center: petramond_math::world_pos::WorldPos,
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
    /// Every animated block (chest, door, trapdoor, a pack's own) to draw.
    pub block_entities: &'a [BlockEntityPresentation],
    /// Mod-submitted per-block draw sets with the light at their cell — the
    /// gather's own rows, handed on without a re-spelling copy.
    pub block_draws: &'a [petramond::world::draw::BlockDrawInstance],
    pub mobs: &'a [MobPresentation],
    /// The mob rows' fading gaits, animation layers and ragdoll poses, which
    /// each [`MobPresentation`] addresses by range.
    pub mob_arena: &'a MobArena,
    /// The session's animation-name table the layers' [`AnimId`]s index.
    pub anim_names: &'a AnimNames,
    /// Every player body in view — the local third-person body first when
    /// it is drawn, then each remote — already posed by the client's
    /// animation. The render input rows themselves, so no second
    /// translation buys anything.
    pub bodies: &'a [crate::PlayerBodyRender],
    /// Every posed body's bones, back to back — each body addresses its own
    /// slice by `PlayerRenderInstance::pose`. One arena keeps every render
    /// row a plain `Copy` value and puts no ceiling on a rig's bone count.
    pub body_poses: &'a [glam::Mat4],
    pub held_item_light: (u8, petramond_world::light::BlockLight6),
    /// Every break (crack) overlay to draw this frame: the LOCAL player's own
    /// mining target plus each visible remote's replicated one, capped at the
    /// `MAX_BREAK_OVERLAYS` nearest to the camera.
    pub break_overlays: &'a [BreakOverlayView],
    /// Blob shadows under entities (mobs, dropped items, bodies), already
    /// ground-resolved + view-culled by the gather.
    pub shadows: &'a [EntityShadow],
}
