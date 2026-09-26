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

/// Bytes of one uv-rect table row: the `(u0, v0, u1, v1)` of one atlas tile.
///
/// The table is a read-only STORAGE buffer sized from the loaded tile
/// catalogue (see `pipeline::create_shared_bindings`), so it grows with the
/// content instead of pinning a length every shader has to spell: the WGSL side
/// is the runtime-sized `array<vec4<f32>>` the `petramond::uv_rects` import
/// declares ([`UV_RECTS_WGSL`]).
pub const UV_RECT_BYTES: u64 = 16;

/// The `petramond::uv_rects` shader module: binding 1 of the block group 0
/// (and of the model3d MVP group), selected by tile id and never recomputed.
pub(crate) const UV_RECTS_WGSL: &str =
    "@group(0) @binding(1) var<storage, read> uv_rects: array<vec4<f32>>;\n";

/// Layout entry for the uv-rect table at `binding`, vertex-stage.
pub(crate) fn uv_rects_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(UV_RECT_BYTES),
        },
        count: None,
    }
}

/// A one-row stand-in for the uv-rect table, for binds whose pipelines share
/// the block group-0 layout but never read the table (terrain, world models,
/// ghosts, schematic thumbnails).
pub(crate) fn uv_rects_placeholder(device: &wgpu::Device, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: UV_RECT_BYTES,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

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
    /// The integer world origin `view_proj` is relative to (`xyz`; `w` = 0).
    /// Integer so a far-out position reaches a shader only as the difference
    /// of two integers plus a small float: world shaders offset each draw's
    /// anchor by it before applying `view_proj`.
    pub render_origin: [i32; 4],
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
    /// `w` is reserved (0).
    pub volume_tint: [f32; 4],
}

/// Every [`Uniforms`] field as the `petramond::frame` shader module declares
/// it: `(name, WGSL type, byte offset)`, in declaration order. Shaders never
/// spell the struct themselves — they import it — so a field may go anywhere;
/// [`frame_wgsl`] emits this table and the tests pin it to the Rust layout
/// (offsets contiguous, sizes matching, nothing left out).
pub(crate) const UNIFORM_FIELDS: [(&str, &str, usize); 10] = [
    (
        "view_proj",
        "mat4x4<f32>",
        std::mem::offset_of!(Uniforms, view_proj),
    ),
    (
        "cam_pos",
        "vec4<f32>",
        std::mem::offset_of!(Uniforms, cam_pos),
    ),
    ("fog", "vec4<f32>", std::mem::offset_of!(Uniforms, fog)),
    (
        "fog_color",
        "vec4<f32>",
        std::mem::offset_of!(Uniforms, fog_color),
    ),
    (
        "inv_view_proj",
        "mat4x4<f32>",
        std::mem::offset_of!(Uniforms, inv_view_proj),
    ),
    (
        "render_origin",
        "vec4<i32>",
        std::mem::offset_of!(Uniforms, render_origin),
    ),
    (
        "atlas_layout",
        "vec4<u32>",
        std::mem::offset_of!(Uniforms, atlas_layout),
    ),
    (
        "sky_color",
        "vec4<f32>",
        std::mem::offset_of!(Uniforms, sky_color),
    ),
    (
        "sun_dir",
        "vec4<f32>",
        std::mem::offset_of!(Uniforms, sun_dir),
    ),
    (
        "volume_tint",
        "vec4<f32>",
        std::mem::offset_of!(Uniforms, volume_tint),
    ),
];

/// Version of the `petramond::frame` module a pack shader imports
/// (`PETRAMOND_FRAME_ABI` in WGSL). Bumped whenever a field changes meaning
/// or is removed, so a pack can tell which frame it was written against.
pub(crate) const FRAME_ABI_VERSION: u32 = 1;

/// The `petramond::frame` shader module: the [`Uniforms`] struct (from
/// [`UNIFORM_FIELDS`]) and the ABI version constant.
pub(crate) fn frame_wgsl() -> String {
    let mut text = format!(
        "// petramond::frame — generated from render::uniforms::Uniforms; do not copy.\n\
         const PETRAMOND_FRAME_ABI: u32 = {FRAME_ABI_VERSION}u;\n\
         struct Uniforms {{\n"
    );
    for (name, ty, _) in UNIFORM_FIELDS {
        text.push_str(&format!("    {name}: {ty},\n"));
    }
    text.push_str("};\n");
    text
}

/// The `petramond::shader_params` module: the named-parameter slots a pack
/// sky or environment shader reads at group 0 binding 1.
pub(crate) fn shader_params_wgsl() -> String {
    format!("struct ShaderParams {{\n    values: array<vec4<f32>, {SHADER_PARAM_SLOTS}>,\n}};\n")
}

#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShaderParams {
    pub values: [[f32; 4]; SHADER_PARAM_SLOTS],
}

#[cfg(test)]
mod tests;
