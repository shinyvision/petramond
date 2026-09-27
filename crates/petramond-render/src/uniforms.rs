pub fn fog_range(render_dist_chunks: i32) -> (f32, f32) {
    let end = (render_dist_chunks.max(1) * petramond_world::chunk::SECTION_SIZE as i32) as f32;
    (end * 0.75, end)
}

pub const UV_RECT_BYTES: u64 = 16;

pub(crate) const UV_RECTS_WGSL: &str =
    "@group(0) @binding(1) var<storage, read> uv_rects: array<vec4<f32>>;\n";

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
    pub cam_pos: [f32; 4],
    pub fog: [f32; 4],
    pub fog_color: [f32; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub render_origin: [i32; 4],
    pub atlas_layout: [u32; 4],
    pub sky_color: [f32; 4],
    pub sun_dir: [f32; 4],
    pub volume_tint: [f32; 4],
}

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

pub(crate) const FRAME_ABI_VERSION: u32 = 1;

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
