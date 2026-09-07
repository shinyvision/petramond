//! Raster pipelines whose sample count follows the scene targets.
//!
//! A scene pipeline is compiled for a sample count the first time that count
//! is drawn, never ahead of it: the world's ~25 pipelines would otherwise each
//! compile a 1x, 4x and 8x variant at start-up when a session usually draws
//! one. Screen pipelines (UI, crosshair, the scene resolve) always draw at one
//! sample and are plain `wgpu::RenderPipeline`s built by
//! [`super::builders::single_pipeline`].

use std::sync::{Arc, OnceLock};

/// Sample counts a scene target can have — the ladder
/// `renderer::construct::max_scene_samples` picks from.
pub(crate) const SAMPLE_COUNTS: [u32; 3] = [1, 4, 8];

/// An owned `wgpu::VertexBufferLayout`, so a pipeline description outlives the
/// constructor's stack-allocated attribute arrays.
pub(super) struct VertexLayout {
    array_stride: wgpu::BufferAddress,
    step_mode: wgpu::VertexStepMode,
    attributes: Vec<wgpu::VertexAttribute>,
}

impl VertexLayout {
    pub(super) fn from_wgpu(layout: &wgpu::VertexBufferLayout<'_>) -> Self {
        Self {
            array_stride: layout.array_stride,
            step_mode: layout.step_mode,
            attributes: layout.attributes.to_vec(),
        }
    }

    pub(super) fn as_wgpu(&self) -> wgpu::VertexBufferLayout<'_> {
        wgpu::VertexBufferLayout {
            array_stride: self.array_stride,
            step_mode: self.step_mode,
            attributes: &self.attributes,
        }
    }
}

/// Everything a render pipeline is made of except its sample count.
pub(super) struct PipelineSpec {
    pub label: String,
    pub layout: wgpu::PipelineLayout,
    pub shader: wgpu::ShaderModule,
    pub vs_entry: String,
    pub fs_entry: String,
    pub buffers: Vec<VertexLayout>,
    pub targets: Vec<Option<wgpu::ColorTargetState>>,
    pub primitive: wgpu::PrimitiveState,
    pub depth: Option<wgpu::DepthStencilState>,
}

/// One scene pipeline in every sample count the device supports, compiled on
/// first use. Clones share the compiled variants: the pass constructors hand
/// the same pipeline to several dynamic draws.
#[derive(Clone)]
pub(crate) struct SampledPipeline {
    device: wgpu::Device,
    spec: Arc<PipelineSpec>,
    /// The device's ceiling; a request above it is a renderer bug, not a
    /// fallback case (the scene mode is clamped before it reaches a draw).
    max_samples: u32,
    variants: Arc<[OnceLock<wgpu::RenderPipeline>; SAMPLE_COUNTS.len()]>,
}

impl SampledPipeline {
    pub(super) fn new(device: &wgpu::Device, spec: PipelineSpec, max_samples: u32) -> Self {
        Self {
            device: device.clone(),
            spec: Arc::new(spec),
            max_samples,
            variants: Arc::new(std::array::from_fn(|_| OnceLock::new())),
        }
    }

    /// The variant rasterizing `samples` per pixel, compiled now if this is
    /// the first draw at that count.
    pub(crate) fn get(&self, samples: u32) -> &wgpu::RenderPipeline {
        let slot = SAMPLE_COUNTS
            .iter()
            .position(|n| *n == samples)
            .unwrap_or_else(|| panic!("{samples}x is not a scene sample count"));
        assert!(
            samples <= self.max_samples,
            "{samples}x scene pipeline on a device capped at {}x",
            self.max_samples
        );
        self.variants[slot].get_or_init(|| {
            let buffers: Vec<_> = self
                .spec
                .buffers
                .iter()
                .map(VertexLayout::as_wgpu)
                .collect();
            super::builders::render_pipeline(
                &self.device,
                &self.spec.label,
                &self.spec.layout,
                &self.spec.shader,
                &self.spec.vs_entry,
                &self.spec.fs_entry,
                &buffers,
                &self.spec.targets,
                self.spec.primitive,
                self.spec.depth.clone(),
                samples,
            )
        })
    }
}
