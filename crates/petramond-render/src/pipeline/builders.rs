use std::num::NonZeroU64;

/// Polygon offset for the break-overlay decal: nudge the crack toward the camera
/// (depth is standard near=0/far=1, so negative = closer) so it reliably wins the
/// `LessEqual` depth tie against the coincident block face despite the mesher's
/// per-AO triangulation flip. The `constant` term covers head-on faces (depth slope
/// ~0); the `slope_scale` term covers glancing angles. A few ULP — far too
/// small to overcome a genuinely closer surface or to read as parallax.
const BREAK_DEPTH_BIAS: wgpu::DepthBiasState = wgpu::DepthBiasState {
    constant: -10,
    slope_scale: -1.0,
    clamp: 0.0,
};

// The break-overlay crack cube is COINCIDENT with the block faces, so it wins the
// depth `LessEqual` tie only via a polygon offset toward the camera. Depth is
// standard (near=0/far=1, closer = smaller), so both offset terms MUST be negative
// — a positive or zero bias would leave the decal at/behind the surface and the
// crack would z-fight or vanish. Guard the sign at COMPILE TIME so a future
// "cleanup" can't silently break it. (The magnitude is intentionally unchecked: the
// float-depth bias unit is implementation-defined per the WebGPU/Vulkan spec.)
const _: () = assert!(
    BREAK_DEPTH_BIAS.constant < 0,
    "constant bias must be negative (toward camera)"
);
const _: () = assert!(
    BREAK_DEPTH_BIAS.slope_scale < 0.0,
    "slope-scaled bias must be negative (toward camera)"
);

/// Polygon offset for the model→terrain contact-shadow stamp, which is
/// coincident with the top face of the supporting block. Its OWN named bias —
/// initially equal to the break overlay's, but tuned independently: the stamp is
/// a large mostly-horizontal decal seen at flatter angles than a crack cube, so
/// its slope term may need to drift without touching the crack's.
const CONTACT_DEPTH_BIAS: wgpu::DepthBiasState = wgpu::DepthBiasState {
    constant: -10,
    slope_scale: -1.0,
    clamp: 0.0,
};

const _: () = assert!(
    CONTACT_DEPTH_BIAS.constant < 0,
    "constant bias must be negative (toward camera)"
);
const _: () = assert!(
    CONTACT_DEPTH_BIAS.slope_scale < 0.0,
    "slope-scaled bias must be negative (toward camera)"
);

/// The render target's depth format. Every depth-tested pass shares one
/// `Depth32Float` attachment, so the presets below all use this.
pub(super) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// A named depth-stencil configuration for [`world_pipeline`]. The world passes
/// only ever vary along three axes — whether depth is written, the compare
/// function, and the polygon-offset bias — so each variant captures one real
/// combination instead of re-spelling the `DepthStencilState` block per pipeline.
#[derive(Copy, Clone)]
pub(super) enum DepthPreset {
    /// Depth test `Less` + WRITE. Opaque geometry, particles, and the hand
    /// variants that self-sort against a cleared depth buffer.
    WriteLess,
    /// Depth test `Less`, NO write. Transparent water and emitter particles:
    /// sort behind solid geometry without occluding the surfaces drawn after.
    ReadLess,
    /// Depth test `LessEqual`, NO write, with the break-overlay polygon offset.
    /// The crack decal is coincident with the block faces; the bias wins the tie.
    ReadLessEqualBiased,
    /// Depth test `LessEqual`, NO write, with the contact-shadow polygon offset.
    /// The stamp is coincident with the supporting block's top face.
    ReadLessEqualContactBiased,
    /// Depth test `LessEqual`, NO write, no bias. The selection outline: hidden
    /// behind terrain but its slightly-inflated front edges win the equal test.
    ReadLessEqual,
}

impl DepthPreset {
    fn state(self) -> wgpu::DepthStencilState {
        let (write, compare, bias) = match self {
            DepthPreset::WriteLess => (
                true,
                wgpu::CompareFunction::Less,
                wgpu::DepthBiasState::default(),
            ),
            DepthPreset::ReadLess => (
                false,
                wgpu::CompareFunction::Less,
                wgpu::DepthBiasState::default(),
            ),
            DepthPreset::ReadLessEqualBiased => {
                (false, wgpu::CompareFunction::LessEqual, BREAK_DEPTH_BIAS)
            }
            DepthPreset::ReadLessEqualContactBiased => {
                (false, wgpu::CompareFunction::LessEqual, CONTACT_DEPTH_BIAS)
            }
            DepthPreset::ReadLessEqual => (
                false,
                wgpu::CompareFunction::LessEqual,
                wgpu::DepthBiasState::default(),
            ),
        };
        wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: write,
            depth_compare: compare,
            stencil: wgpu::StencilState::default(),
            bias,
        }
    }
}

/// One color target with the given blend. `write_mask` is `ALL` for every pass
/// except the crosshair (which writes COLOR only); pass that explicitly.
pub(super) fn color_target(
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    write_mask: wgpu::ColorWrites,
) -> [Option<wgpu::ColorTargetState>; 1] {
    [Some(wgpu::ColorTargetState {
        format,
        blend,
        write_mask,
    })]
}

/// One labelled WGSL shader module (source from `include_str!`/`concat!`, or
/// the shader pack's owned string).
pub(super) fn shader_module(
    device: &wgpu::Device,
    label: &str,
    wgsl: impl Into<std::borrow::Cow<'static, str>>,
) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    })
}

/// A pipeline layout over `bind_group_layouts`; no pass in this module uses
/// push constants.
pub(super) fn pipeline_layout(
    device: &wgpu::Device,
    label: &str,
    bind_group_layouts: &[&wgpu::BindGroupLayout],
) -> wgpu::PipelineLayout {
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts,
        push_constant_ranges: &[],
    })
}

/// A whole-buffer uniform layout entry (no dynamic offset).
pub(super) fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    min_size: u64,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(min_size),
        },
        count: None,
    }
}

/// A bind group binding each buffer whole, at bindings `0..buffers.len()` in
/// order.
pub(super) fn buffer_bind_group(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    buffers: &[&wgpu::Buffer],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry> = buffers
        .iter()
        .enumerate()
        .map(|(i, buf)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: buf.as_entire_binding(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &entries,
    })
}

/// The layout-entry pair of a fragment-sampled float texture (at `binding`)
/// plus its filtering sampler (at `binding + 1`).
pub(super) fn texture_sampler_layout_entries(
    binding: u32,
    dim: wgpu::TextureViewDimension,
) -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: dim,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: binding + 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

/// The bind-group entry pair matching [`texture_sampler_layout_entries`].
pub(super) fn texture_sampler_bind_entries<'a>(
    binding: u32,
    view: &'a wgpu::TextureView,
    sampler: &'a wgpu::Sampler,
) -> [wgpu::BindGroupEntry<'a>; 2] {
    [
        wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        },
        wgpu::BindGroupEntry {
            binding: binding + 1,
            resource: wgpu::BindingResource::Sampler(sampler),
        },
    ]
}

/// A single texture+sampler bind-group layout (bindings 0/1).
pub(super) fn texture_sampler_bgl(
    device: &wgpu::Device,
    label: &str,
    dim: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &texture_sampler_layout_entries(0, dim),
    })
}

/// Layout + bind group over one texture view + sampler; labels derive from
/// `label` (`"<label> bgl"` / `"<label> bg"`).
pub(super) fn texture_sampler_bgl_bind(
    device: &wgpu::Device,
    label: &str,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    dim: wgpu::TextureViewDimension,
) -> (wgpu::BindGroupLayout, wgpu::BindGroup) {
    let bgl = texture_sampler_bgl(device, &format!("{label} bgl"), dim);
    let bg_label = format!("{label} bg");
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&bg_label),
        layout: &bgl,
        entries: &texture_sampler_bind_entries(0, view, sampler),
    });
    (bgl, bind)
}

/// Build a render pipeline, filling the fields that are constant across every
/// pass in this module (`compilation_options`, the shared `sample_count`
/// multisample state, `multiview: None`, `cache: None`) exactly once. Callers
/// supply only what actually varies per pass: label, layout, shader + entry
/// points, vertex buffer layouts, the color targets, the primitive state, and an
/// optional [`DepthPreset`] (`None` = no depth attachment).
///
/// Vertex and fragment stages share one `shader` module — every pass in this
/// file does. The depth-less UI / icon passes pass `depth: None`; that is the
/// ONLY difference between e.g. `model3d_pipe` and `model3d_hand_pipe`.
#[allow(clippy::too_many_arguments)]
pub(super) fn world_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vs_entry: &str,
    fs_entry: &str,
    buffers: &[wgpu::VertexBufferLayout],
    targets: &[Option<wgpu::ColorTargetState>],
    primitive: wgpu::PrimitiveState,
    depth: Option<DepthPreset>,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs_entry),
            compilation_options: Default::default(),
            buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs_entry),
            compilation_options: Default::default(),
            targets,
        }),
        primitive,
        depth_stencil: depth.map(DepthPreset::state),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    })
}

/// Back-face-culled primitive state shared by the block-vertex passes
/// (opaque/transparent terrain, model3d, break overlay).
pub(super) fn cull_back() -> wgpu::PrimitiveState {
    wgpu::PrimitiveState {
        cull_mode: Some(wgpu::Face::Back),
        ..Default::default()
    }
}
