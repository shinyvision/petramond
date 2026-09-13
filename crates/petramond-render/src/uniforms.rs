/// Terminal fog band: the smoothstep ramp of `atmosphere.wgsl`'s terminal term.
/// The band starts earlier than the old linear fog so terrain dissolves into the
/// haze gradually; the returned end is a hard contract — the atmosphere reaches
/// exactly 1.0 there, and terrain visibility is fog-distance culled against it.
///
/// Derived from the streaming radius so the fog fade always terminates exactly at
/// the loaded-world edge: a lower render distance pulls the fog in with it instead
/// of ending terrain before the fade.
pub fn fog_range(render_dist_chunks: i32) -> (f32, f32) {
    let end = (render_dist_chunks.max(1) * petramond_world::chunk::SECTION_SIZE as i32) as f32;
    (end * 0.75, end)
}

/// Fixed size of the uv-rect table shared with the vertex shader. Sized
/// straight from the packed vertex's tile-id field ([`petramond_mesh::MAX_TILES`])
/// so the whole content catalogue fits without a shader edit, and so widening
/// that field cannot leave the table behind. The
/// `pipeline.rs` `assert!(TILE_COUNT <= UV_RECTS_LEN)` is the runtime guard
/// that the catalogue fits the table.
///
/// The WGSL side spells the length as a LITERAL (`array<vec4<f32>, 2048>`) in
/// every shader bound to this group — WGSL cannot read a Rust constant, so
/// `uv_rect_table_length_matches_every_shader` re-reads the shader sources and
/// pins them to this value.
pub const UV_RECTS_LEN: usize = petramond_mesh::MAX_TILES;

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
    /// `(start, end, time, in fluid)`: the fog band (the eye fluid's own band
    /// while inside one), animation seconds, and `1` while the camera eye is
    /// inside a fluid, else `0` (every shader's `w > 0.5` test).
    pub fog: [f32; 4],
    /// `xyz` = fog colour; `w` = the sim-owned sky scale (1.0 = noon/identity),
    /// read by `block.wgsl`, `model3d.wgsl`, and `sky.wgsl` to dim skylight,
    /// the held item, and the sky-gradient zenith at draw time.
    pub fog_color: [f32; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    /// World-space origin subtracted by world shaders before applying `view_proj`.
    /// Keeps GPU transform math camera-local while simulation/render data remains
    /// in absolute world coordinates.
    pub render_origin: [f32; 4],
    /// Atlas layout for the block shader: `w` is the atlas TILE COUNT — the
    /// texture-array layer offset from any tile to its dye-base twin for dyed
    /// vertices (packed2 bit 19). `xyz` are reserved (0).
    pub atlas_layout: [u32; 4],
    /// `xyz` = the sim-owned sky light COLOUR (white `[1,1,1]` = identity; a
    /// day/night mod tints the night subtly blue), applied to the SKY lighting
    /// term only in `block.wgsl` / `model3d.wgsl` and to the `sky.wgsl` zenith.
    /// `w` is reserved (0).
    pub sky_color: [f32; 4],
    /// `xyz` = the unit sun direction (derived from the engine-owned
    /// `petramond:time` day fraction with the same arc formula as
    /// `daynight_sky.wgsl`); `w` = daylight in `[0,1]` (1 = full day). Read by
    /// the atmosphere haze (`atmosphere.wgsl`) so terrain fog warms toward the
    /// sun the sky shader draws.
    pub sun_dir: [f32; 4],
    /// `xyz` = the eye fluid's `volume_tint`, multiplied over every surface
    /// but a fluid's own faces while the eye is inside it (white in air);
    /// `w` is reserved (0). New fields go at the END: every other shader
    /// declares a PREFIX of this struct, so an insertion above shifts what they
    /// read (`uniform_layout_matches_every_shader`).
    pub volume_tint: [f32; 4],
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

    /// Every shader bound to the frame uniform buffer declares its own WGSL
    /// mirror of [`Uniforms`], most of them a PREFIX. WGSL lays a struct out
    /// from its own declaration, so a field inserted mid-struct on the Rust
    /// side silently shifts everything a prefix mirror reads after it (the
    /// sky colour became lava tile ids once). Parse each mirror's fields,
    /// accumulate their std140 offsets, and pin them to the Rust offsets.
    #[test]
    fn uniform_layout_matches_every_shader() {
        use super::Uniforms;
        use std::mem::offset_of;

        let sources = [
            (
                "block.wgsl",
                include_str!("../shaders/block.wgsl"),
                "Uniforms",
            ),
            ("sky.wgsl", include_str!("../shaders/sky.wgsl"), "Uniforms"),
            ("mob.wgsl", include_str!("../shaders/mob.wgsl"), "Uniforms"),
            (
                "particles.wgsl",
                include_str!("../shaders/particles.wgsl"),
                "Uniforms",
            ),
            (
                "contact.wgsl",
                include_str!("../shaders/contact.wgsl"),
                "Uniforms",
            ),
            (
                "entity_shadow.wgsl",
                include_str!("../shaders/entity_shadow.wgsl"),
                "Uniforms",
            ),
            (
                "break_overlay.wgsl",
                include_str!("../shaders/break_overlay.wgsl"),
                "Uniforms",
            ),
            (
                "outline.wgsl",
                include_str!("../shaders/outline.wgsl"),
                "Uniforms",
            ),
            (
                "model3d.wgsl",
                include_str!("../shaders/model3d.wgsl"),
                "FrameUniforms",
            ),
        ];
        let rust_offset = |field: &str| -> Option<usize> {
            Some(match field {
                "view_proj" => offset_of!(Uniforms, view_proj),
                "cam_pos" => offset_of!(Uniforms, cam_pos),
                "fog" => offset_of!(Uniforms, fog),
                "fog_color" => offset_of!(Uniforms, fog_color),
                "inv_view_proj" => offset_of!(Uniforms, inv_view_proj),
                "render_origin" => offset_of!(Uniforms, render_origin),
                "atlas_layout" => offset_of!(Uniforms, atlas_layout),
                "sky_color" => offset_of!(Uniforms, sky_color),
                "sun_dir" => offset_of!(Uniforms, sun_dir),
                "volume_tint" => offset_of!(Uniforms, volume_tint),
                _ => return None,
            })
        };
        for (name, src, struct_name) in sources {
            let header = format!("struct {struct_name} {{");
            let body = src
                .split_once(&header)
                .and_then(|(_, rest)| rest.split_once("};"))
                .map(|(body, _)| body)
                .unwrap_or_else(|| panic!("{name} declares no `{header}`"));
            let mut offset = 0usize;
            let mut fields = 0;
            for line in body.lines() {
                let decl = line.split("//").next().unwrap_or("").trim();
                let Some((field, ty)) = decl.split_once(':') else {
                    continue;
                };
                let (field, ty) = (field.trim(), ty.trim().trim_end_matches(','));
                let size = match ty {
                    "mat4x4<f32>" => 64,
                    "vec4<f32>" | "vec4<u32>" => 16,
                    other => panic!("{name}: unhandled uniform field type `{other}`"),
                };
                let want = rust_offset(field)
                    .unwrap_or_else(|| panic!("{name} declares unknown frame uniform `{field}`"));
                assert_eq!(
                    offset, want,
                    "{name}: `{field}` sits at byte {offset} in WGSL but {want} in `Uniforms` \
                     (a field inserted mid-struct? new fields go at the end)"
                );
                offset += size;
                fields += 1;
            }
            assert!(fields > 0, "{name}: no fields parsed from `{struct_name}`");
            assert!(
                offset <= std::mem::size_of::<Uniforms>(),
                "{name}: mirror ({offset} bytes) overruns `Uniforms`"
            );
        }
    }
}
