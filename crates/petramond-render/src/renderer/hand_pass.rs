//! The first-person hand pass's state.

use super::*;

/// The first-person hand pass: the held item's own pipelines and buffers,
/// the per-frame hand geometry, and the break-crack decal drawn with it.
pub(super) struct HandPass {
    /// Depth-enabled model3d variant for the first-person held block in the hand
    /// pass (same shader; the hand pass clears depth so the held block self-sorts).
    /// (The depthless `model3d_pipe` is now used only to bake the icon atlas at init,
    /// so it isn't stored here.)
    pub(super) model3d_pipe: crate::pipeline::SampledPipeline,
    /// Dynamic-offset MVP uniform buffer (256-byte slots); slot 0 is the hand.
    pub(super) model3d_mvp_buf: wgpu::Buffer,
    /// group(0) bind for model3d (MVP at binding 0 + uv_rects at binding 1).
    pub(super) model3d_mvp_bind: wgpu::BindGroup,
    /// Reusable dynamic vertex/index buffers for model3d draws (rewritten in place).
    pub(super) model3d_vbuf: wgpu::Buffer,
    pub(super) model3d_ibuf: wgpu::Buffer,
    /// item3d pipeline (extruded first-person held item) + its group0 MVP bind
    /// (over the shared `model3d_mvp_buf`, slot 0) and reusable dynamic vbuf.
    pub(super) item3d_pipe: crate::pipeline::SampledPipeline,
    pub(super) item3d_mvp_bind: wgpu::BindGroup,
    pub(super) item3d_vbuf: wgpu::Buffer,
    /// Reusable CPU staging for the extruded held-item geometry (cleared +
    /// refilled by `item_model::build_extruded_item`, capacity retained).
    pub(super) item3d_verts: Vec<crate::item_model::ItemVertex>,
    /// Vertex count of the extruded held item uploaded this frame (0 = none).
    pub(super) item3d_vertex_count: u32,
    /// True when this frame's item3d geometry is a held bbmodel block (drawn with the
    /// MODEL atlas) rather than an extruded sprite (the block atlas).
    pub(super) held_is_model: bool,
    /// Index count of the hand geometry uploaded for this frame (0 = nothing).
    pub(super) index_count: u32,
    /// Vertex count of the hand geometry — the OFF-hand geometry appends
    /// after it in the shared model3d vbuf, so its `base_vertex` starts here.
    pub(super) vertex_count: u32,
    // --- The OFF (left) hand: its own view/animator, its geometry appended
    // --- into the SAME buffers after the main hand's, MVP slot 1. Drawn only
    // --- while the off-hand slot holds an item (no bare left arm).
    /// Off-hand held item state (`item == None` = empty, nothing drawn).
    pub(super) off_item: HeldItemView,
    /// The off-hand item3d stream's `[start, start + count)` vertex range in
    /// the shared item3d vbuf (appended after the main hand's stream).
    pub(super) off_item3d_start: u32,
    pub(super) off_item3d_count: u32,
    /// The off item3d stream draws with the MODEL atlas (bbmodel) rather than
    /// the block atlas (extruded sprite) — per-stream twin of `held_is_model`.
    pub(super) off_is_model: bool,
    /// Reusable CPU staging for the per-frame hand geometry (cleared +
    /// refilled by `prepare_held_item`, capacity retained — no per-frame
    /// allocation), and the bbmodel / sprite scratch every item3d expansion
    /// bakes through.
    pub(super) verts: Vec<petramond_mesh::Vertex>,
    pub(super) indices: Vec<u32>,
    pub(super) model_scratch_verts: Vec<crate::item_model::ItemVertex>,
    pub(super) model_scratch_indices: Vec<u32>,
    pub(super) off_item3d_scratch: Vec<crate::item_model::ItemVertex>,
    /// Break-overlay (destroy crack): its own pipeline + dynamic vbuf/ibuf + the
    /// index count baked this frame (0 = no overlay), as one [`DynamicDraw`].
    pub(super) break_draw: DynamicDraw,
    // --- Per-frame view state handed off by the App, drawn in `render`. ---
    /// Block-break overlays to draw this frame (own + capped remotes; empty =
    /// none).
    pub(super) break_overlays: Vec<BreakOverlayView>,
    /// The main hand's held item state.
    pub(super) held_item: HeldItemView,
    /// Each hand's eased claimed pose (`[main, off]`).
    pub(super) held_ease: [crate::HeldItemEase; 2],
    pub(super) visible: bool,
    /// Screen-space (NDC) offset applied to the whole hand/held-item draw this
    /// frame — the hurt-shake jitter. Zero when calm.
    pub(super) shake: [f32; 2],
    /// Whether the view may shake (Options → Graphics): off draws the world
    /// and the hands without the camera bone's offset or the hurt jitter.
    pub(super) screen_shake: bool,
    pub(super) held_item_skylight: u8,
    pub(super) held_item_blocklight: petramond_world::light::BlockLight6,
    /// The first-person rig and its animator. `None` (either asset missing)
    /// draws no hand at all.
    pub(super) first_person: Option<crate::first_person::FirstPersonHand>,
    /// This frame's two hand frames (`[main, off]`), kept for the local
    /// player's animators, and the seconds since the last frame's.
    pub(super) frames: Option<[crate::HeldItemFrame; 2]>,
    pub(super) frame_dt: f32,
    /// The local player's resolved animator claims and the graph events
    /// fired on it this frame, for both of its rigs. The body bake consumes
    /// the events, so a second bake before the next claims fires nothing.
    pub(super) local_params: Vec<crate::views::AnimatorParamRow>,
    pub(super) local_plays: Vec<petramond::player::AnimatorPlay>,
    pub(super) local_events: Vec<(petramond::player::RigId, u16)>,
    /// Name-valued claims, interned once per distinct name.
    pub(super) names: crate::views::NameCache,
    /// The rig's arms in the item3d stream, `[arm_start, arm_start +
    /// arm_count)`, drawn with the player's skin.
    pub(super) arm_start: u32,
    pub(super) arm_count: u32,
}

impl HandPass {
    /// Drop the world-scoped hand state. The held item and its animator
    /// are world state too — a stale pose must not survive into the next.
    pub(super) fn clear_world(&mut self) {
        self.visible = false;
        self.index_count = 0;
        self.vertex_count = 0;
        self.item3d_vertex_count = 0;
        self.held_is_model = false;
        self.held_item = HeldItemView::default();
        self.off_item = HeldItemView::default();
        self.held_ease = Default::default();
        self.off_item3d_start = 0;
        self.off_item3d_count = 0;
        self.off_is_model = false;
        self.shake = [0.0; 2];
        self.break_overlays.clear();
        self.break_draw.index_count = 0;
        if let Some(first_person) = &mut self.first_person {
            first_person.reset();
        }
        self.frames = None;
        self.frame_dt = 0.0;
        self.local_params.clear();
        self.local_plays.clear();
        self.local_events.clear();
        self.arm_start = 0;
        self.arm_count = 0;
    }
}
