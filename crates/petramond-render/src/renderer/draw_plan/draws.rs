//! The terrain quad passes' draw lists: every draw a pass submits this plan,
//! as the `DrawIndexedIndirectArgs` records an indirect draw reads, grouped
//! into BATCHES that share one quad-arena block and one pipeline.
//!
//! The quad arena allocates in whole [`TerrainVertex`] units, so binding one
//! block's buffer reaches every column in it by `base_vertex`; each column's
//! origin row rides `first_instance`. A batch is therefore ONE
//! `multi_draw_indexed_indirect` however many columns and sections it covers.
//! Devices without indirect `first_instance` take the fallback: the same
//! records, issued one `draw_indexed` each — identical draws, CPU-submitted.
//!
//! The lists are built once per plan (the planner reruns only when the view
//! or the column set changes) and uploaded then; a frame that reuses the plan
//! reuses the uploaded records.
//!
//! [`TerrainVertex`]: petramond_mesh::TerrainVertex

use crate::resources::{ColumnBuffer, GpuColumnMesh, Span, TerrainArenas};
use petramond_mesh::QuadLayer;

/// One indexed draw in the layout indirect draws read
/// (`wgpu::util::DrawIndexedIndirectArgs`): 20 bytes, tightly packed.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct IndirectDraw {
    pub index_count: u32,
    pub instance_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
}

const DRAW_BYTES: u64 = std::mem::size_of::<IndirectDraw>() as u64;

/// The three terrain passes that draw quad layers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum QuadPass {
    /// Opaque terrain, near→far. Depth-tested and written with `Less`, so the
    /// image does not depend on draw order: batches group by block.
    Opaque,
    /// Translucent blocks (ice), near→far. Blended, so draw order is kept.
    Translucent,
    /// Translucent fluids, far→near, sides and two-sided tops interleaved.
    /// Blended, so draw order is kept.
    Transparent,
}

impl QuadPass {
    const COUNT: usize = 3;
    const ALL: [QuadPass; Self::COUNT] = [
        QuadPass::Opaque,
        QuadPass::Translucent,
        QuadPass::Transparent,
    ];

    /// Whether the pass's result depends on draw order (blending). Such a
    /// pass batches only CONSECUTIVE draws that share a block and pipeline.
    fn order_matters(self) -> bool {
        !matches!(self, QuadPass::Opaque)
    }
}

/// A run of a list's draws sharing one quad-arena block and quad layer (the
/// layer picks the pipeline).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Batch {
    layer: QuadLayer,
    block: u32,
    first: u32,
    count: u32,
}

/// One pass's draws, in submission order once [`DrawList::finish`] ran.
#[derive(Default)]
pub(crate) struct DrawList {
    draws: Vec<IndirectDraw>,
    /// `(layer, block)` of each draw while the list is built.
    keys: Vec<(QuadLayer, u32)>,
    batches: Vec<Batch>,
    /// This list's first record in the uploaded indirect buffer.
    base: u32,
    indices: u64,
}

impl DrawList {
    fn clear(&mut self) {
        self.draws.clear();
        self.keys.clear();
        self.batches.clear();
        self.indices = 0;
    }

    /// Draws in the list.
    pub fn len(&self) -> u32 {
        self.draws.len() as u32
    }

    /// Indices the list submits.
    pub fn indices(&self) -> u64 {
        self.indices
    }

    /// Queue `quads` quads of `column`'s `layer` buffer from element `start`
    /// (a section's span, or a whole-column region). Nothing is queued for an
    /// empty range or a column without that buffer.
    pub fn push(
        &mut self,
        arenas: &TerrainArenas,
        column: &GpuColumnMesh,
        layer: QuadLayer,
        start: u32,
        quads: u32,
    ) {
        if quads == 0 {
            return;
        }
        let Some(buffer) = column.buffer(ColumnBuffer::Quads(layer)) else {
            return;
        };
        let (block, first) = arenas.quads().first_element(&buffer.alloc);
        let slot = column.origin_slot.index();
        self.draws.push(IndirectDraw {
            index_count: quads * 6,
            instance_count: 1,
            first_index: 0,
            base_vertex: (first + start) as i32,
            first_instance: slot,
        });
        self.keys.push((layer, block));
        self.indices += u64::from(quads) * 6;
    }

    /// Queue a section's span of `layer` (vertices, whole quads).
    pub fn push_span(
        &mut self,
        arenas: &TerrainArenas,
        column: &GpuColumnMesh,
        layer: QuadLayer,
        span: Span,
    ) {
        self.push(arenas, column, layer, span.start, span.count / 4);
    }

    /// Batch the queued draws. An order-free pass groups them by block first
    /// (stably, so near→far survives inside each block).
    fn finish(&mut self, order_matters: bool) {
        if !order_matters {
            let mut keyed: Vec<_> = self.keys.drain(..).zip(self.draws.drain(..)).collect();
            keyed.sort_by_key(|&((_, block), _)| block);
            for (key, draw) in keyed {
                self.keys.push(key);
                self.draws.push(draw);
            }
        }
        self.batches.clear();
        for (i, &(layer, block)) in self.keys.iter().enumerate() {
            match self.batches.last_mut() {
                Some(b) if b.layer == layer && b.block == block => b.count += 1,
                _ => self.batches.push(Batch {
                    layer,
                    block,
                    first: i as u32,
                    count: 1,
                }),
            }
        }
        self.keys.clear();
    }
}

/// How a device submits the lists.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Submission {
    /// One `multi_draw_indexed_indirect` per batch.
    Indirect,
    /// One `draw_indexed` per record: devices without indirect
    /// `first_instance` (the column origin rides it).
    Direct,
}

/// The device features the indirect path needs, as far as `adapter` has them
/// (request them at device creation; without them the lists draw directly).
/// An adapter that cannot execute indirect draws at all gets none, so its
/// device takes the direct path.
pub(crate) fn wanted_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let indirect = adapter
        .get_downlevel_capabilities()
        .flags
        .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION);
    if indirect {
        adapter.features() & wgpu::Features::INDIRECT_FIRST_INSTANCE
    } else {
        wgpu::Features::empty()
    }
}

/// Every terrain quad pass's draw list and the GPU records they upload to.
pub(crate) struct TerrainDraws {
    lists: [DrawList; QuadPass::COUNT],
    indirect: Option<wgpu::Buffer>,
    submission: Submission,
}

impl TerrainDraws {
    pub fn new(device: &wgpu::Device) -> Self {
        let submission = if device
            .features()
            .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
        {
            Submission::Indirect
        } else {
            Submission::Direct
        };
        Self {
            lists: Default::default(),
            indirect: None,
            submission,
        }
    }

    /// Start a new plan: every list empty.
    pub fn clear(&mut self) {
        for list in &mut self.lists {
            list.clear();
        }
    }

    /// Whether every record costs a CPU draw call on this device (no indirect
    /// `first_instance`). There, one whole-column draw beats per-section draws
    /// even when it also covers culled sections.
    pub fn draws_directly(&self) -> bool {
        self.submission == Submission::Direct
    }

    pub fn list(&self, pass: QuadPass) -> &DrawList {
        &self.lists[pass as usize]
    }

    pub fn list_mut(&mut self, pass: QuadPass) -> &mut DrawList {
        &mut self.lists[pass as usize]
    }

    /// Batch every list and upload the records the indirect path reads.
    pub fn finish(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut base = 0u32;
        for pass in QuadPass::ALL {
            let list = &mut self.lists[pass as usize];
            list.finish(pass.order_matters());
            list.base = base;
            base += list.len();
        }
        if self.submission == Submission::Direct || base == 0 {
            return;
        }
        let bytes = u64::from(base) * DRAW_BYTES;
        if self.indirect.as_ref().is_none_or(|b| b.size() < bytes) {
            self.indirect = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("terrain indirect draws"),
                size: bytes.next_power_of_two(),
                usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let buffer = self.indirect.as_ref().expect("sized above");
        let size = wgpu::BufferSize::new(bytes).expect("non-empty");
        if let Some(mut view) = queue.write_buffer_with(buffer, 0, size) {
            let mut at = 0;
            for list in &self.lists {
                let src: &[u8] = bytemuck::cast_slice(&list.draws);
                view[at..at + src.len()].copy_from_slice(src);
                at += src.len();
            }
        }
    }

    /// Record `pass`'s draws. `pipeline` names the pipeline each quad layer
    /// draws with; it is bound whenever the layer changes (a render pass
    /// starts with none bound). The caller binds everything else: the frame
    /// and atlas groups, the column-origin table and the shared quad index.
    pub fn encode<'p>(
        &self,
        render: &mut wgpu::RenderPass<'_>,
        pass: QuadPass,
        arenas: &TerrainArenas,
        pipeline: &dyn Fn(QuadLayer) -> &'p wgpu::RenderPipeline,
    ) {
        let list = self.list(pass);
        let mut bound_layer = None;
        let mut bound_block = None;
        for batch in &list.batches {
            if bound_layer != Some(batch.layer) {
                render.set_pipeline(pipeline(batch.layer));
                bound_layer = Some(batch.layer);
            }
            if bound_block != Some(batch.block) {
                render.set_vertex_buffer(0, arenas.quads().block_buffer(batch.block).slice(..));
                bound_block = Some(batch.block);
            }
            match (&self.indirect, self.submission) {
                (Some(indirect), Submission::Indirect) => render.multi_draw_indexed_indirect(
                    indirect,
                    u64::from(list.base + batch.first) * DRAW_BYTES,
                    batch.count,
                ),
                _ => {
                    let first = batch.first as usize;
                    for d in &list.draws[first..first + batch.count as usize] {
                        render.draw_indexed(
                            d.first_index..d.first_index + d.index_count,
                            d.base_vertex,
                            d.first_instance..d.first_instance + d.instance_count,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
