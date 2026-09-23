//! The bbmodel-block break crack: a decal over the model's OWN triangles.
//!
//! A cell-shaped block's crack is geometry built on the CPU
//! ([`crate::break_overlay`]). A model block's is not: nothing here re-derives
//! the model's form. The pass re-draws the column's model index stream — the
//! exact triangles the model pass drew — with a decal pipeline that
//!
//! * claims only the fragments of the cracked model's own surface, so the
//!   neighbouring furniture in the same column stays clean and a multi-cell
//!   piece cracks as ONE object. The claim is SIDED — a fragment is pushed a
//!   hair back along its own normal before the outline-box test, because two
//!   models placed against each other share a boundary plane exactly and a
//!   plain point test claims both models' faces on it;
//! * discards every fragment the model's own texture leaves transparent, so a
//!   cutout texel takes no crack (no crack hanging in mid-air);
//! * projects ONE destroy tile across the whole outline box, so the piece wears
//!   a single continuous crack pattern instead of one complete crack per cube.
//!
//! Because the decal is the same geometry, its depth is the model's own: the
//! crack cannot misalign, at any angle, for any model, ever.

use glam::IVec3;
use petramond_world::chunk::{ChunkPos, SECTION_SIZE};

use crate::views::BreakOverlayView;

/// Cracked models drawn at once. The overlay list is the local miner plus
/// visible remotes; four concurrently-cracked models in view is already
/// generous, and the mask is a fixed-size uniform the vertex stage walks.
pub(crate) const MAX_MODEL_CRACKS: usize = 4;

/// One cracked model's mask: its world outline box relative to the render
/// origin, plus the atlas uv rect of its stage's destroy tile.
#[repr(C)]
#[derive(Copy, Clone, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct CrackEntry {
    lo: [f32; 4],
    hi: [f32; 4],
    rect: [f32; 4],
}

/// group(2) binding 0 of the model-break pipeline.
#[repr(C, align(16))]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct CrackUniform {
    entries: [CrackEntry; MAX_MODEL_CRACKS],
    /// `x` = live entries; the rest pads to 16 bytes.
    count: [u32; 4],
}

impl Default for CrackUniform {
    fn default() -> Self {
        Self {
            entries: [CrackEntry::default(); MAX_MODEL_CRACKS],
            count: [0; 4],
        }
    }
}

/// The group(2) layout: the crack masks (read in the VERTEX stage, which
/// classifies each vertex into its model's box) plus the BLOCK atlas, where the
/// destroy tiles live — group(1) is the model atlas the geometry samples.
pub(crate) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    let tex = crate::pipeline::texture_sampler_layout_entries(1, wgpu::TextureViewDimension::D2);
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<CrackUniform>() as u64),
            },
            count: None,
        },
        tex[0],
        tex[1],
    ]
}

/// The model-break decal pass's resources: its pipeline, the per-frame mask
/// uniform, and the columns whose model streams this frame must re-draw.
pub(crate) struct ModelBreak {
    pub(crate) pipe: crate::pipeline::SampledPipeline,
    buf: wgpu::Buffer,
    pub(crate) bind: wgpu::BindGroup,
    /// Columns holding a cracked model this frame (deduplicated; nearly always
    /// one, at most four per crack when a model straddles a column corner).
    pub(crate) columns: Vec<ChunkPos>,
}

impl ModelBreak {
    pub(crate) fn new(
        device: &wgpu::Device,
        pipe: crate::pipeline::SampledPipeline,
        layout: &wgpu::BindGroupLayout,
        atlas_view: &wgpu::TextureView,
        atlas_sampler: &wgpu::Sampler,
    ) -> Self {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("model break cracks"),
            size: std::mem::size_of::<CrackUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tex = crate::pipeline::texture_sampler_bind_entries(1, atlas_view, atlas_sampler);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model break bind"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                },
                tex[0].clone(),
                tex[1].clone(),
            ],
        });
        Self {
            pipe,
            buf,
            bind,
            columns: Vec::new(),
        }
    }

    /// Take this frame's model cracks from the overlay views: upload their
    /// masks and collect the columns to re-draw. Non-model overlays are the CPU
    /// geometry path's and are ignored here.
    pub(crate) fn upload(
        &mut self,
        queue: &wgpu::Queue,
        views: &[BreakOverlayView],
        render_origin: IVec3,
    ) {
        self.columns.clear();
        let mut uniform = CrackUniform::default();
        for view in views {
            let Some(model) = view.model else { continue };
            let n = uniform.count[0] as usize;
            if n == MAX_MODEL_CRACKS {
                break;
            }
            let base = (model.base - render_origin).as_vec3();
            let lo = base + glam::Vec3::from(model.min);
            let hi = base + glam::Vec3::from(model.max);
            uniform.entries[n] = CrackEntry {
                lo: [lo.x, lo.y, lo.z, 0.0],
                hi: [hi.x, hi.y, hi.z, 0.0],
                rect: crate::atlas::tile_uv(crate::break_overlay::destroy_tile(view.stage)),
            };
            uniform.count[0] += 1;
            push_columns(&mut self.columns, model.base, model.min, model.max);
        }
        queue.write_buffer(&self.buf, 0, bytemuck::bytes_of(&uniform));
    }

    /// Whether anything is cracked this frame.
    pub(crate) fn active(&self) -> bool {
        !self.columns.is_empty()
    }
}

/// Record every column the model's outline reaches into — its geometry can be
/// baked into any of them, and a model wider than a cell may straddle a column
/// edge.
fn push_columns(columns: &mut Vec<ChunkPos>, base: IVec3, min: [f32; 3], max: [f32; 3]) {
    let s = SECTION_SIZE as i32;
    let (x0, x1) = (
        base.x + min[0].floor() as i32,
        base.x + max[0].ceil() as i32,
    );
    let (z0, z1) = (
        base.z + min[2].floor() as i32,
        base.z + max[2].ceil() as i32,
    );
    for cx in x0.div_euclid(s)..=x1.div_euclid(s) {
        for cz in z0.div_euclid(s)..=z1.div_euclid(s) {
            let pos = ChunkPos::new(cx, cz);
            if !columns.contains(&pos) {
                columns.push(pos);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mask array's length is a WGSL LITERAL, which no Rust constant can
    /// reach. A cap raised on this side alone would leave the shader reading a
    /// shorter array — silently dropping the cracks past its end.
    #[test]
    fn shader_declares_the_mask_array_length() {
        let src = include_str!("../shaders/model_break.wgsl");
        assert!(
            src.contains(&format!("array<ModelCrack, {MAX_MODEL_CRACKS}>")),
            "model_break.wgsl must declare the mask array as \
             `array<ModelCrack, {MAX_MODEL_CRACKS}>`"
        );
    }

    /// A model wider than a cell straddles column edges; every column it
    /// reaches into must draw, or half the piece keeps an uncracked face.
    #[test]
    fn a_straddling_model_draws_every_column_it_reaches() {
        let mut columns = Vec::new();
        push_columns(
            &mut columns,
            IVec3::new(15, 64, 15),
            [0.0; 3],
            [2.0, 1.0, 2.0],
        );
        assert_eq!(columns.len(), 4);
    }
}
