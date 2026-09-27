use super::*;

pub(super) struct HandPass {
    pub(super) model3d_pipe: crate::pipeline::SampledPipeline,
    pub(super) model3d_mvp_buf: wgpu::Buffer,
    pub(super) model3d_mvp_bind: wgpu::BindGroup,
    pub(super) model3d_vbuf: wgpu::Buffer,
    pub(super) model3d_ibuf: wgpu::Buffer,
    pub(super) item3d_pipe: crate::pipeline::SampledPipeline,
    pub(super) item3d_mvp_bind: wgpu::BindGroup,
    pub(super) item3d_vbuf: wgpu::Buffer,
    pub(super) item3d_verts: Vec<crate::item_model::ItemVertex>,
    pub(super) item3d_vertex_count: u32,
    pub(super) held_is_model: bool,
    pub(super) index_count: u32,
    pub(super) vertex_count: u32,
    pub(super) off_item: HeldItemView,
    pub(super) off_item3d_start: u32,
    pub(super) off_item3d_count: u32,
    pub(super) off_is_model: bool,
    pub(super) verts: Vec<petramond_mesh::Vertex>,
    pub(super) indices: Vec<u32>,
    pub(super) model_scratch_verts: Vec<crate::item_model::ItemVertex>,
    pub(super) model_scratch_indices: Vec<u32>,
    pub(super) off_item3d_scratch: Vec<crate::item_model::ItemVertex>,
    pub(super) break_draw: DynamicDraw,
    pub(super) break_overlays: Vec<BreakOverlayView>,
    pub(super) held_item: HeldItemView,
    pub(super) visible: bool,
    pub(super) shake: [f32; 2],
    pub(super) screen_shake: bool,
    pub(super) held_item_skylight: u8,
    pub(super) held_item_blocklight: petramond_world::light::BlockLight6,
    pub(super) first_person: Option<crate::first_person::FirstPersonHand>,
    pub(super) arm_start: u32,
    pub(super) arm_count: u32,
}

impl HandPass {
    pub(super) fn clear_world(&mut self) {
        self.visible = false;
        self.index_count = 0;
        self.vertex_count = 0;
        self.item3d_vertex_count = 0;
        self.held_is_model = false;
        self.held_item = HeldItemView::default();
        self.off_item = HeldItemView::default();
        self.off_item3d_start = 0;
        self.off_item3d_count = 0;
        self.off_is_model = false;
        self.shake = [0.0; 2];
        self.break_overlays.clear();
        self.break_draw.index_count = 0;
        if let Some(first_person) = &mut self.first_person {
            first_person.reset();
        }
        self.arm_start = 0;
        self.arm_count = 0;
    }
}
