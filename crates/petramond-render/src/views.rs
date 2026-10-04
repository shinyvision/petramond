use std::sync::Arc;

use glam::{IVec3, Quat, Vec3};

use petramond::mob::Mob;
use petramond::world::PlacedEmitter;
use petramond_math::math::Tilt;
use petramond_world::item::ItemType;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BreakOverlayView {
    pub block: IVec3,
    pub visual_box: Option<([f32; 3], [f32; 3])>,
    pub shape_boxes: Option<CrackBoxes>,
    pub model: Option<ModelCrack>,
    pub stage: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ModelCrack {
    pub base: IVec3,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

pub const MAX_CRACK_BOXES: usize = 16;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CrackBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub faces: [bool; 6],
    pub pose: Option<petramond_world::block::BoxPose>,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CrackBoxes {
    pub boxes: [CrackBox; MAX_CRACK_BOXES],
    pub len: u8,
}

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
    pub prev_flight: Option<[f32; 3]>,
    pub flight: Option<[f32; 3]>,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParticleAtlas {
    Block,
    Model,
    /// No atlas, just a solid-color cube, e.g. a water splash from an emitter burst. `tint` is the
    /// color. Drawn alpha-blended with the looping emitter cubes, not through the cutout fleck
    /// pipeline.
    Solid,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ParticlePresentation {
    pub quad_axes: Option<[Vec3; 2]>,
    pub atlas: ParticleAtlas,
    pub pos: petramond_math::world_pos::WorldPos,
    pub uv_min: [f32; 2],
    pub uv_size: [f32; 2],
    pub tint: [f32; 3],
    pub alpha: f32,
    pub size: f32,
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
    pub prev_tilt: Tilt,
    pub tilt: Tilt,
    pub prev_anim_time: f32,
    pub anim_time: f32,
    pub moving: bool,
    pub idle_anim: Option<u8>,
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
    pub anims: crate::ArenaRange,
    pub emitter_tint: [f32; 3],
    pub emitter_self_lit: f32,
    pub ragdoll_pose: Option<crate::ArenaRange>,
    pub held: [Option<petramond_world::item::ItemType>; 2],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnimId(pub u32);

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

    pub fn adopt(&mut self, other: &AnimNames) {
        if !Arc::ptr_eq(&self.0, &other.0) {
            self.0 = Arc::clone(&other.0);
        }
    }
}

#[derive(Debug, Default)]
pub struct AnimInterner {
    names: AnimNames,
    ids: rustc_hash::FxHashMap<Arc<str>, AnimId>,
}

impl AnimInterner {
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

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GaitFade {
    pub clip: crate::GaitClip,
    pub phase: f32,
    pub weight: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct AnimLayer {
    pub anim: AnimId,
    pub phase: f32,
    pub weight: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MobArena {
    pub gait_fades: Vec<GaitFade>,
    pub anims: Vec<AnimLayer>,
    pub ragdoll: Vec<(Vec3, Quat)>,
}

impl MobArena {
    pub fn clear(&mut self) {
        self.gait_fades.clear();
        self.anims.clear();
        self.ragdoll.clear();
    }

    pub fn copy_from(&mut self, other: &MobArena) {
        self.clear();
        self.gait_fades.extend_from_slice(&other.gait_fades);
        self.anims.extend_from_slice(&other.anims);
        self.ragdoll.extend_from_slice(&other.ragdoll);
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EntityShadow {
    pub center: petramond_math::world_pos::WorldPos,
    pub radius: f32,
    pub strength: f32,
}

/// One cloth to draw: a `cols × rows` point grid stored row-major in the frame's
/// shared point list from `first`, positioned relative to the owning `cell`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClothPresentation {
    pub cell: IVec3,
    pub tile: petramond_world::tile::Tile,
    pub uv: [f32; 4],
    pub cols: u16,
    pub rows: u16,
    pub first: u32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClothPoint {
    pub pos: Vec3,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

pub struct GamePresentation<'a> {
    pub tick_alpha: f32,
    pub item_entities: &'a [DroppedItemPresentation],
    pub particles: &'a [ParticlePresentation],
    pub particle_emitters: &'a [PlacedEmitter],
    pub block_entities: &'a [BlockEntityPresentation],
    pub block_draws: &'a [petramond::world::draw::BlockDrawInstance],
    pub cloths: &'a [ClothPresentation],
    pub cloth_points: &'a [ClothPoint],
    pub mobs: &'a [MobPresentation],
    pub mob_arena: &'a MobArena,
    pub anim_names: &'a AnimNames,
    pub bodies: &'a [crate::PlayerBodyRender],
    pub body_poses: &'a [glam::Mat4],
    pub held_item_light: (u8, petramond_world::light::BlockLight6),
    pub break_overlays: &'a [BreakOverlayView],
    pub shadows: &'a [EntityShadow],
}
