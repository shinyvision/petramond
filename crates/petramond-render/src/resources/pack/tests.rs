use super::*;
use bytemuck::Zeroable;
use petramond_mesh::{ContactShadowVertex, ModelVertex, Vertex};

fn mesh(cy: i32, quads: usize, value: u32) -> ChunkMesh {
    let mut mesh = ChunkMesh::empty();
    let vertices: Vec<_> = (0..quads * 4)
        .map(|i| Vertex {
            pos: [(i % 4) as f32, (cy * 16) as f32, (i / 4) as f32],
            tint: value,
            packed: value,
            packed2: value,
        })
        .collect();
    mesh.opaque = vertices.clone();
    mesh.far_opaque_len = ((quads.max(1) - quads / 2) * 4) as u32;
    mesh.transparent = vertices.clone();
    mesh.transparent_two_sided = vertices.clone();
    mesh.translucent = vertices;
    mesh.model = vec![
        ModelVertex {
            tint: value,
            ..ModelVertex::zeroed()
        };
        quads * 4
    ];
    mesh.model_idx = (0..quads as u32)
        .flat_map(|q| [q * 4, q * 4 + 1, q * 4 + 2])
        .collect();
    mesh.model_blend_idx = (0..quads as u32)
        .flat_map(|q| [q * 4 + 2, q * 4 + 3, q * 4])
        .collect();
    mesh.contact = vec![ContactShadowVertex::zeroed(); quads * 6];
    mesh.mesh_dirty = true;
    mesh.into_sealed()
}

fn bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    arenas: &TerrainArenas,
    column: &GpuColumnMesh,
    buffer: ColumnBuffer,
) -> Vec<u8> {
    column.buffer(buffer).map_or_else(Vec::new, |l| {
        arenas.get(buffer).readback(device, queue, &l.alloc, l.len)
    })
}

/// Reads the repack back from the GPU. Once a sibling is released (CPU buffers gone, GPU bytes
/// kept), every buffer of the column must equal a fresh upload of the same meshes. The iterations
/// hit every upload path: same-size remesh, growth and shrink, and a section added in between.
#[test]
fn updates_and_repacking_preserve_released_siblings_in_every_buffer() {
    let instance = wgpu::Instance::new(&crate::renderer::instance_descriptor());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("[skip] no wgpu adapter; repack readback not run");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let mut arenas = TerrainArenas::default();
    let mut origins = ColumnOrigins::new(&device);
    let mut quads = QuadIndexBuffer::new(&device, &queue);
    let a = SectionPos::new(0, 0, 0);
    let b = SectionPos::new(0, 1, 0);
    let c = SectionPos::new(0, 2, 0);
    let mut first = mesh(0, 1, 3);
    let mut sibling = mesh(1, 2, 7);
    let mut batch = TerrainUploadBatch::default();
    let mut actual = upload_column_mesh(
        &device,
        &queue,
        &[(a, &first), (b, &sibling)],
        None,
        &mut origins,
        &mut arenas,
        &mut quads,
        &mut batch,
    );
    batch.submit(&queue);
    sibling.mesh_dirty = false;
    sibling.release_cpu_buffers();
    for (size, add) in [(1, false), (3, false), (2, true), (1, false), (1, false)] {
        let mut batch = TerrainUploadBatch::default();
        first = mesh(0, size, 11);
        let added = mesh(2, 1, 19);
        let mut inputs = vec![(a, &first), (b, &sibling)];
        if add {
            inputs.push((c, &added));
        }
        actual = upload_column_mesh(
            &device,
            &queue,
            &inputs,
            Some(actual),
            &mut origins,
            &mut arenas,
            &mut quads,
            &mut batch,
        );
        let reference_sibling = mesh(1, 2, 7);
        let mut reference_inputs = vec![(a, &first), (b, &reference_sibling)];
        if add {
            reference_inputs.push((c, &added));
        }
        let expected = upload_column_mesh(
            &device,
            &queue,
            &reference_inputs,
            None,
            &mut origins,
            &mut arenas,
            &mut quads,
            &mut batch,
        );
        batch.submit(&queue);
        for buffer in ColumnBuffer::ALL {
            assert_eq!(
                bytes(&device, &queue, &arenas, &actual, buffer),
                bytes(&device, &queue, &arenas, &expected, buffer),
                "{buffer:?}"
            );
        }
        assert_eq!(actual.opaque_quads(), expected.opaque_quads());
        assert_eq!(actual.opaque_far_quads(), expected.opaque_far_quads());
        for stream in SectionStream::ALL {
            assert_eq!(actual.region(stream), expected.region(stream), "{stream:?}");
        }
        for ((ap, a), (bp, b)) in actual.sections.iter().zip(&expected.sections) {
            assert_eq!(ap, bp);
            assert_eq!(a.spans, b.spans, "section {ap:?}");
        }
    }
}

#[test]
fn a_packed_column_tiles_every_region_in_section_order() {
    let meshes = [mesh(0, 2, 1), mesh(1, 3, 2), mesh(2, 1, 3)];
    let positions = [
        SectionPos::new(0, 0, 0),
        SectionPos::new(0, 1, 0),
        SectionPos::new(0, 2, 0),
    ];
    let inputs: Vec<_> = positions.iter().copied().zip(meshes.iter()).collect();
    let mut entries = section_entries(&inputs, &[]);
    let (regions, len) = place_spans(&mut entries);
    let far_total = regions[SectionStream::OpaqueFar.index()];
    for entry in &entries {
        let far = entry.record.span(SectionStream::OpaqueFar);
        let tail = entry.record.span(SectionStream::OpaqueTail);
        assert!(far.end() <= far_total, "far range inside the far region");
        assert!(tail.start >= far_total, "tail range after the far region");
        assert_eq!(far.count, mesh_count(entry.mesh, SectionStream::OpaqueFar));
    }
    let plans = plan_buffers(&entries);
    for (buffer, plan) in ColumnBuffer::ALL.into_iter().zip(&plans) {
        let mut next = 0;
        for p in plan {
            assert_eq!(p.destination, next, "{buffer:?} spans tile the buffer");
            next += p.count;
        }
        assert_eq!(
            next,
            len[buffer.index()],
            "{buffer:?} spans cover the buffer"
        );
    }
    let index_plan = &plans[ColumnBuffer::ModelIndices.index()];
    let blend_start = regions[SectionStream::ModelIndices.index()];
    for (entry, p) in entries
        .iter()
        .zip(index_plan.iter().filter(|p| p.destination >= blend_start))
    {
        let Source::Indices(_, base) = p.source else {
            panic!("an index span not rebuilt from local indices");
        };
        assert_eq!(base, entry.record.span(SectionStream::ModelVertices).start);
    }
}
