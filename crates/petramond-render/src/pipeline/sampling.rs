use std::sync::{Arc, OnceLock};

pub(crate) const SAMPLE_COUNTS: [u32; 3] = [1, 4, 8];

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

#[derive(Clone)]
pub(crate) struct SampledPipeline {
    device: wgpu::Device,
    spec: Arc<PipelineSpec>,
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
