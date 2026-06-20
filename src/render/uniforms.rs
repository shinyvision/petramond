pub const FOG_START: f32 = 14.0 * 16.0;
pub const FOG_END: f32 = 16.0 * 16.0;

/// Underwater fog band (blocks): pulled in tight so submerged visibility is short
/// and distant terrain dissolves into the murky water colour.
pub const UNDERWATER_FOG_START: f32 = 0.5;
pub const UNDERWATER_FOG_END: f32 = 22.0;

/// Fixed size of the uv-rect table shared with the vertex shader (`block.wgsl`
/// declares `array<vec4<f32>, UV_RECTS_LEN>` — keep that literal in sync). Sized
/// to the 8-bit tile-id field (`packed` bits 0..8), so the whole content catalogue
/// fits without a shader edit. 256 × vec4<f32> = 4 KiB, well under the 16 KiB
/// minimum uniform-block size guaranteed by WebGPU. The `pipeline.rs`
/// `assert!(TILE_COUNT <= UV_RECTS_LEN)` is the compile-time guard.
pub const UV_RECTS_LEN: usize = 256;

#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    pub view_proj: [[f32; 4]; 4],
    pub cam_pos: [f32; 4], // padded to 16
    pub fog: [f32; 4],     // (start, end, _, _)
    pub fog_color: [f32; 4],
    pub inv_view_proj: [[f32; 4]; 4],
}
