/// Terminal fog band: the smoothstep ramp of `atmosphere.wgsl`'s terminal term.
/// The band starts earlier than the old linear fog so terrain dissolves into the
/// haze gradually; the returned end is a hard contract — the atmosphere reaches
/// exactly 1.0 there, and terrain visibility is fog-distance culled against it.
///
/// Derived from the streaming radius so the fog fade always terminates exactly at
/// the loaded-world edge: a lower render distance pulls the fog in with it instead
/// of ending terrain before the fade.
pub fn fog_range(render_dist_chunks: i32) -> (f32, f32) {
    let end = (render_dist_chunks.max(1) * crate::chunk::SECTION_SIZE as i32) as f32;
    (end * 0.75, end)
}

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
pub const SHADER_PARAM_SLOTS: usize = 16;

#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    pub view_proj: [[f32; 4]; 4],
    pub cam_pos: [f32; 4], // padded to 16
    pub fog: [f32; 4],     // (start, end, time, underwater)
    /// `xyz` = fog colour; `w` = the sim-owned sky scale (1.0 = noon/identity),
    /// read by `block.wgsl`, `model3d.wgsl`, and `sky.wgsl` to dim skylight,
    /// the held item, and the sky-gradient zenith at draw time (Phase 2c).
    pub fog_color: [f32; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    /// World-space origin subtracted by world shaders before applying `view_proj`.
    /// Keeps GPU transform math camera-local while simulation/render data remains
    /// in absolute world coordinates.
    pub render_origin: [f32; 4],
    /// Animated-water flipbook control for the block shader:
    /// `(water_still_base_tile, water_flow_base_tile, frame_count, _)`. The
    /// shader advances `base + floor(time*fps) % frames` over these two tiles.
    pub water_anim: [u32; 4],
    /// `xyz` = the sim-owned sky light COLOUR (white `[1,1,1]` = identity; a
    /// day/night mod tints the night subtly blue), applied to the SKY lighting
    /// term only in `block.wgsl` / `model3d.wgsl` and to the `sky.wgsl` zenith.
    /// `w` is reserved (0). Appended after `water_anim` so shaders declaring a
    /// prefix of this struct stay layout-compatible.
    pub sky_color: [f32; 4],
    /// `xyz` = the unit sun direction (derived from the engine-owned
    /// `petramond:time` day fraction with the same arc formula as
    /// `daynight_sky.wgsl`); `w` = daylight in `[0,1]` (1 = full day). Read by
    /// the atmosphere haze (`atmosphere.wgsl`) so terrain fog warms toward the
    /// sun the sky shader draws. Appended last for prefix layout compatibility.
    pub sun_dir: [f32; 4],
}

#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShaderParams {
    pub values: [[f32; 4]; SHADER_PARAM_SLOTS],
}
