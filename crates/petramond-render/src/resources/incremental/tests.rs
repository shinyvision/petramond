use super::*;
use bytemuck::Zeroable;

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
    // A real leaf tail: the opaque stream is packed as two per-column regions
    // (every section's far LOD, then every section's tail), and a fixture with
    // an empty tail would never exercise the second one.
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
    mesh
}

fn bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    arena: &GeometryArena,
    layer: &Option<Layer>,
) -> Vec<u8> {
    layer
        .as_ref()
        .map_or_else(Vec::new, |l| arena.readback(device, queue, &l.alloc, l.len))
}

/// The repack's byte-level contract, read back from the GPU: every layer of
/// a column whose sibling was released (CPU buffers gone, GPU bytes retained)
/// equals a from-scratch upload of the same meshes. Iterations cover both
/// upload paths — same-size remesh (retained spans unmoved, written in place),
/// growth and shrink (siblings shift, GPU→GPU copies into a fresh
/// allocation) and a section added between them (index rebasing).
#[test]
fn updates_and_repacking_preserve_released_siblings_in_every_layer() {
    let instance = wgpu::Instance::new(&crate::renderer::instance_descriptor());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("[skip] no wgpu adapter; repack readback not run");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let mut arena = GeometryArena::new();
    let mut origins = ColumnOrigins::new(&device);
    let mut quads = QuadIndexBuffer::new(&device, &queue);
    let mut scratch = ColumnUploadScratch::default();
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
        &mut scratch,
        &mut origins,
        &mut arena,
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
            &mut scratch,
            &mut origins,
            &mut arena,
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
            &mut scratch,
            &mut origins,
            &mut arena,
            &mut quads,
            &mut batch,
        );
        batch.submit(&queue);
        macro_rules! same {
            ($($field:ident),*) => { $(assert_eq!(
                bytes(&device, &queue, &arena, &actual.$field),
                bytes(&device, &queue, &arena, &expected.$field), stringify!($field));)* };
        }
        same!(
            opaque_vbuf,
            transparent_vbuf,
            transparent_ts_vbuf,
            translucent_vbuf,
            model_vbuf,
            model_ibuf,
            contact_vbuf
        );
        assert_eq!(actual.opaque_quads, expected.opaque_quads);
        assert_eq!(actual.opaque_far_quads, expected.opaque_far_quads);
        assert_eq!(actual.model_idx_count, expected.model_idx_count);
        assert_eq!(actual.model_blend_idx_count, expected.model_blend_idx_count);
        for ((ap, a), (bp, b)) in actual.sections.iter().zip(&expected.sections) {
            assert_eq!(ap, bp);
            assert_eq!(a.model_vertex_start, b.model_vertex_start);
            assert_eq!(a.model_blend_index_start, b.model_blend_index_start);
            assert_eq!(a.opaque_vertex_start, b.opaque_vertex_start);
            assert_eq!(a.opaque_vertex_count, b.opaque_vertex_count);
            assert_eq!(a.opaque_tail_start, b.opaque_tail_start);
            assert_eq!(a.opaque_tail_count, b.opaque_tail_count);
        }
    }
}
