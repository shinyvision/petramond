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

/// Fixed size of the uv-rect table shared with the vertex shader. Sized
/// straight from the packed vertex's tile-id field ([`crate::mesh::MAX_TILES`])
/// so the whole content catalogue fits without a shader edit, and so widening
/// that field cannot leave the table behind. The
/// `pipeline.rs` `assert!(TILE_COUNT <= UV_RECTS_LEN)` is the runtime guard
/// that the catalogue fits the table.
///
/// The WGSL side spells the length as a LITERAL (`array<vec4<f32>, 2048>`) in
/// every shader bound to this group — WGSL cannot read a Rust constant, so
/// `uv_rect_table_length_matches_every_shader` re-reads the shader sources and
/// pins them to this value.
pub const UV_RECTS_LEN: usize = crate::mesh::MAX_TILES;

/// The binding must fit `wgpu::Limits::default().max_uniform_buffer_binding_size`
/// (64 KiB) — which is what `render::renderer::construct` requests, so a table
/// past it would fail device creation on every adapter rather than fall back.
/// At 2048 tiles the table is 32 KiB; the next widening of the tile field would
/// trip this instead of shipping.
const _: () = assert!(UV_RECTS_LEN * 16 <= 65536);

pub const SHADER_PARAM_SLOTS: usize = 16;

#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    pub view_proj: [[f32; 4]; 4],
    pub cam_pos: [f32; 4], // padded to 16
    pub fog: [f32; 4],     // (start, end, time, underwater)
    /// `xyz` = fog colour; `w` = the sim-owned sky scale (1.0 = noon/identity),
    /// read by `block.wgsl`, `model3d.wgsl`, and `sky.wgsl` to dim skylight,
    /// the held item, and the sky-gradient zenith at draw time.
    pub fog_color: [f32; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    /// World-space origin subtracted by world shaders before applying `view_proj`.
    /// Keeps GPU transform math camera-local while simulation/render data remains
    /// in absolute world coordinates.
    pub render_origin: [f32; 4],
    /// Atlas animation + dye-layout control for the block shader:
    /// `(water_still_base_tile, water_flow_base_tile, frame_count, tile_count)`.
    /// `xyz` drive the animated-water flipbook (the shader advances
    /// `base + floor(time*fps) % frames` over the two water tiles); `w` is
    /// the atlas TILE COUNT — the texture-array layer offset from any tile
    /// to its dye-base twin for dyed vertices (packed2 bit 19).
    pub atlas_anim: [u32; 4],
    /// `xyz` = the sim-owned sky light COLOUR (white `[1,1,1]` = identity; a
    /// day/night mod tints the night subtly blue), applied to the SKY lighting
    /// term only in `block.wgsl` / `model3d.wgsl` and to the `sky.wgsl` zenith.
    /// `w` is reserved (0). Appended after `atlas_anim` so shaders declaring a
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

#[cfg(test)]
mod tests {
    use super::UV_RECTS_LEN;

    /// Every shader bound to the block group-0 layout declares the uv-rect
    /// table's length as a WGSL literal, which no Rust constant can reach. A
    /// shader left behind at the old length silently reads (or fails to
    /// validate) past the table when the tile field widens, so re-read the
    /// sources and pin the literal.
    #[test]
    fn uv_rect_table_length_matches_every_shader() {
        // Every shader declaring binding 1 of the shared block group.
        let sources = [
            ("block.wgsl", include_str!("../shaders/block.wgsl")),
            ("model3d.wgsl", include_str!("../shaders/model3d.wgsl")),
            (
                "break_overlay.wgsl",
                include_str!("../shaders/break_overlay.wgsl"),
            ),
            ("particles.wgsl", include_str!("../shaders/particles.wgsl")),
        ];
        let want = format!("array<vec4<f32>, {UV_RECTS_LEN}>");
        let mut declared = 0;
        for (name, src) in sources {
            for line in src.lines() {
                if !line.contains("uv_rects") || !line.contains("array<vec4<f32>") {
                    continue;
                }
                declared += 1;
                assert!(
                    line.contains(&want),
                    "{name} declares uv_rects as `{}`, not `{want}`",
                    line.trim()
                );
            }
        }
        // block.wgsl computes its uvs from the atlas directly and declares no
        // table; the other three do. A source that stops declaring it (or a
        // new one that starts) should be reflected here deliberately.
        assert_eq!(declared, 3, "shaders declaring the uv-rect table");
    }
}
