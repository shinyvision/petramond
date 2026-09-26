//! The actor pass's state: mob species and player body GPU resources.

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
    /// The model's hand bones, coat cubes and self-AO, resolved once.
    pub(super) rig: crate::mob_model::MobRig,
    pub(super) bind: wgpu::BindGroup,
    /// The species' bind-space mesh, skinned per instance on the GPU.
    pub(super) mesh: SkinnedModel,
    /// This frame's instances of the species in the skin batch.
    pub(super) drawn: std::ops::Range<u32>,
    /// Live-mob frustum cull volume around the instance position, derived from
    /// the REST-POSED model bounds × scale plus animation slack (see
    /// `construct`): horizontal radius (yaw-independent — the farthest posed
    /// corner can point any way) and the vertical extent relative to the feet.
    /// A hardcoded pad was the old bug: it topped out at 1.2 m and clipped any
    /// taller species (the hushjaw is ~1.9 m) out of the frustum early.
    pub(super) cull_r: f32,
    pub(super) cull_y0: f32,
    pub(super) cull_y1: f32,
    /// Frustum-visible subset of this species' instances this frame, as
    /// indices into `ActorPass::mobs`.
    pub(super) visible: Vec<u32>,
    /// The species' resolved animation clips and layer scratch, kept across
    /// frames.
    pub(super) pose: crate::mob_model::MobPoseCache<'static>,
}

/// GPU resources for player bodies — the local third-person body AND every
/// remote player, all sharing the body rig's skinned mesh + the player skin
/// texture bind (per-remote skins are out of scope). Every visible body is one
/// instance of one instanced draw.
pub(super) struct PlayerGpu {
    pub(super) bind: wgpu::BindGroup,
    /// The body rig's bind-space mesh, skinned per instance on the GPU.
    pub(super) mesh: SkinnedModel,
    /// This frame's bodies in the skin batch.
    pub(super) drawn: std::ops::Range<u32>,
}

/// The actor pass: every animated body (mobs, the local third-person player,
/// remote players) and the held-item streams attached to their hands.
pub(super) struct ActorPass {
    /// Per-species mob render resources, indexed by `Mob as usize` (registry id
    /// order). Built once from `mob::defs()`; each frame the visible mobs are
    /// grouped here by species, posed, and drawn in the mob pass.
    pub(super) mob_gpu: Vec<MobGpu>,
    /// The frame's skinned bodies — every mob and player body's bone palette
    /// and instance row — and the buffers they upload to.
    pub(super) skin: SkinFrame,
    /// Mobs to draw in the world this frame (the scene adapter fills this by
    /// interpolating the sim's live mob instances).
    pub(super) mobs: Vec<MobRenderInstance>,
    /// The arena `mobs`' ranges address, and the session's animation-name
    /// table their layers' ids index.
    pub(super) mob_arena: crate::MobArena,
    pub(super) anim_names: crate::AnimNames,
    /// Player-body resources (local third-person + remote players, one
    /// instanced draw in the mob pass).
    pub(super) player_gpu: PlayerGpu,
    /// The posed player bodies to draw this frame (the local third-person
    /// body first when drawn, then remotes) with their held-item views, and
    /// the pose arena their `PlayerRenderInstance::pose` ranges index into.
    pub(super) bodies: Vec<PlayerBodyRender>,
    pub(super) body_poses: Vec<glam::Mat4>,
    /// Frustum-visible bodies this frame.
    pub(super) player_visible: Vec<PlayerBodyRender>,
    /// Held EXTRUDED-SPRITE items across all bodies (explicit-UV stream, 2D
    /// atlas), attached to each posed right hand.
    pub(super) item_draw: DynamicDraw,
    pub(super) item_verts: Vec<crate::item_model::ItemVertex>,
    pub(super) item_indices: Vec<u32>,
    /// Per-item staging for one extruded-sprite build (the builder clears).
    pub(super) sprite_verts: Vec<crate::item_model::ItemVertex>,
    /// Held BBMODEL items across all bodies (explicit-UV stream, MODEL atlas) —
    /// split from the sprite stream so mixed hands draw with the right texture.
    pub(super) model_item_draw: DynamicDraw,
    pub(super) model_item_verts: Vec<crate::item_model::ItemVertex>,
    pub(super) model_item_indices: Vec<u32>,
    /// Held BLOCK mini-cubes across all bodies (packed block vertices, opaque
    /// pipeline + terrain atlas array), CPU-transformed to each hand.
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
