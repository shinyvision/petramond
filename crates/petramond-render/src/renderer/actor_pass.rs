use super::*;

/// Per-species GPU resources for the skinned pipeline, built once at renderer init by
/// iterating [`petramond::mob::defs()`] (so the renderer never names a species). Borrows
/// the species' precached [`Model`] + its render scale, the species' own texture/sampler + group(1)
/// bind, its static skinned mesh (uploaded once), and per-frame state (the visible
/// subset + its instance range in the frame's skin batch). The `Vec<MobGpu>` is in
/// `Mob as usize` order.
pub(super) struct MobGpu {
    pub(super) model: &'static Model,
    pub(super) scale: f32,
    pub(super) rig: crate::mob_model::MobRig,
    pub(super) bind: wgpu::BindGroup,
    /// The species' bind-space mesh, skinned per instance on the GPU.
    pub(super) mesh: SkinnedModel,
    pub(super) drawn: std::ops::Range<u32>,
    /// Live-mob frustum cull volume around the instance position, derived from
    /// the REST-POSED model bounds × scale plus animation slack (see
    /// `construct`): horizontal radius (yaw-independent — the farthest posed
    /// corner can point any way) and the vertical extent relative to the feet.
    /// The bound scales with each species so taller bodies remain in the
    /// frustum until their posed geometry leaves it.
    pub(super) cull_r: f32,
    pub(super) cull_y0: f32,
    pub(super) cull_y1: f32,
    pub(super) visible: Vec<u32>,
    pub(super) pose: crate::mob_model::MobPoseCache<'static>,
}

pub(super) struct PlayerGpu {
    pub(super) bind: wgpu::BindGroup,
    pub(super) mesh: SkinnedModel,
    pub(super) drawn: std::ops::Range<u32>,
}

pub(super) struct ActorPass {
    pub(super) mob_gpu: Vec<MobGpu>,
    pub(super) skin: SkinFrame,
    pub(super) mobs: Vec<MobRenderInstance>,
    pub(super) mob_arena: crate::MobArena,
    pub(super) anim_names: crate::AnimNames,
    pub(super) player_gpu: PlayerGpu,
    pub(super) bodies: Vec<PlayerBodyRender>,
    pub(super) body_poses: Vec<glam::Mat4>,
    pub(super) player_visible: Vec<PlayerBodyRender>,
    pub(super) item_draw: DynamicDraw,
    pub(super) item_verts: Vec<crate::item_model::ItemVertex>,
    pub(super) item_indices: Vec<u32>,
    pub(super) sprite_verts: Vec<crate::item_model::ItemVertex>,
    pub(super) model_item_draw: DynamicDraw,
    pub(super) model_item_verts: Vec<crate::item_model::ItemVertex>,
    pub(super) model_item_indices: Vec<u32>,
    pub(super) block_item_draw: DynamicDraw,
}

impl ActorPass {
    pub(super) fn clear_world(&mut self) {
        for mob in &mut self.mob_gpu {
            mob.drawn = 0..0;
            mob.visible.clear();
        }
        self.skin.batch.clear();
        self.mobs.clear();
        self.mob_arena.clear();
        self.player_gpu.drawn = 0..0;
        self.bodies.clear();
        self.body_poses.clear();
        self.player_visible.clear();
        self.item_draw.index_count = 0;
        self.model_item_draw.index_count = 0;
        self.block_item_draw.index_count = 0;
    }
}
