//! The actor pass's state: mob species and player body GPU resources.

use super::*;

/// Per-species GPU resources for the mob pipeline, built once at renderer init by
/// iterating [`petramond::mob::defs()`] (so the renderer never names a species). Borrows
/// the species' precached [`Model`] + its render scale, the species' own texture/sampler + group(1)
/// bind, its dynamic draw buffers, and reused per-frame scratch (the visible subset
/// + the baked `ItemVertex` geometry). The `Vec<MobGpu>` is in `Mob as usize` order.
pub(super) struct MobGpu {
    pub(super) model: &'static Model,
    pub(super) scale: f32,
    /// The model's hand bones, coat cubes and self-AO, resolved once.
    pub(super) rig: crate::mob_model::MobRig,
    pub(super) bind: wgpu::BindGroup,
    pub(super) draw: DynamicDraw,
    /// Live-mob frustum cull volume around the instance position, derived from
    /// the REST-POSED model bounds × scale plus animation slack (see
    /// `construct`): horizontal radius (yaw-independent — the farthest posed
    /// corner can point any way) and the vertical extent relative to the feet.
    /// A hardcoded pad was the old bug: it topped out at 1.2 m and clipped any
    /// taller species (the hushjaw is ~1.9 m) out of the frustum early.
    pub(super) cull_r: f32,
    pub(super) cull_y0: f32,
    pub(super) cull_y1: f32,
    /// Frustum-visible subset of this species' instances this frame.
    pub(super) visible: Vec<MobRenderInstance>,
    /// Reused CPU staging for this species' baked geometry.
    pub(super) verts: Vec<ItemVertex>,
    pub(super) indices: Vec<u32>,
}

/// GPU resources for player bodies — the local third-person body AND every
/// remote player, all sharing the precached player model + skin texture bind
/// (per-remote skins are out of scope). One dynamic draw over the shared mob
/// pipeline; `verts`/`indices` are the COMBINED per-frame staging every
/// visible body appends into.
pub(super) struct PlayerGpu {
    pub(super) bind: wgpu::BindGroup,
    pub(super) draw: DynamicDraw,
    pub(super) verts: Vec<ItemVertex>,
    pub(super) indices: Vec<u32>,
}

/// One body the frame draws, with what drives its animator.
#[derive(Clone, Copy)]
pub(super) struct VisibleBody {
    pub(super) inst: PlayerRenderInstance,
    pub(super) held: HeldItemView,
    pub(super) off: HeldItemView,
    pub(super) key: u32,
    pub(super) frames: Option<[HeldItemFrame; 2]>,
    /// The body's claims in the frame's arenas; `None` is the local body,
    /// whose claims the hand pass holds.
    pub(super) animator: Option<crate::AnimatorRanges>,
}

/// The actor pass: every animated body (mobs, the local third-person player,
/// remote players) and the held-item streams attached to their hands.
pub(super) struct ActorPass {
    /// Per-species mob render resources, indexed by `Mob as usize` (registry id
    /// order). Built once from `mob::defs()`; each frame the visible mobs are
    /// grouped here by species, baked, and drawn in the mob pass.
    pub(super) mob_gpu: Vec<MobGpu>,
    /// Mobs to draw in the world this frame (the scene adapter fills this by
    /// interpolating the sim's live mob instances).
    pub(super) mobs: Vec<MobRenderInstance>,
    /// Player-body resources (local third-person + remote players, one
    /// combined stream drawn in the mob pass).
    pub(super) player_gpu: PlayerGpu,
    /// The LOCAL third-person body to draw this frame (`None` in first person).
    pub(super) player_view: Option<PlayerRenderInstance>,
    /// The remote players' bodies + held-item views for this frame.
    pub(super) remote_players: Vec<RemotePlayerRender>,
    /// This frame's bone offsets for every drawn body, back to back — each
    /// body addresses its own slice by `PlayerRenderInstance::bones`.
    pub(super) bone_offsets: Vec<crate::BoneOffset>,
    /// This frame's animator claims and fired events for every remote body,
    /// back to back — each addresses its own by `RemotePlayerRender::animator`.
    pub(super) animator_params: Vec<crate::views::AnimatorParamRow>,
    pub(super) animator_plays: Vec<petramond::player::AnimatorPlay>,
    pub(super) animator_events: Vec<(petramond::player::RigId, u16)>,
    /// Frustum-visible bodies this frame (local first, then remotes), each
    /// paired with the held-item view that animates its hand.
    pub(super) player_visible: Vec<VisibleBody>,
    /// Every roster body's animator.
    pub(super) body_animators: crate::player_model::BodyAnimators,
    /// Per-body staging for one `build_player_body` bake, appended into
    /// `player_gpu`'s combined stream.
    pub(super) body_verts: Vec<crate::item_model::ItemVertex>,
    pub(super) body_indices: Vec<u32>,
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
            mob.draw.index_count = 0;
            mob.visible.clear();
        }
        self.mobs.clear();
        self.player_gpu.draw.index_count = 0;
        self.player_view = None;
        self.remote_players.clear();
        self.bone_offsets.clear();
        self.animator_params.clear();
        self.animator_plays.clear();
        self.animator_events.clear();
        self.player_visible.clear();
        self.body_animators.clear();
        self.item_draw.index_count = 0;
        self.model_item_draw.index_count = 0;
        self.block_item_draw.index_count = 0;
    }
}
