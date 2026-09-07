#[test]
fn world_cell_variation_survives_rebases_faces_and_transition_donors() {
    let instance = wgpu::Instance::new(&crate::renderer::instance_descriptor());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("[skip] no wgpu adapter; terrain variation shader check not run");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let source = super::super::lanes::declarations()
        + &super::hash_declaration()
        + "fn block_variation_count(tile: u32) -> u32 { return select(1u, 4u, tile == 7u); }\n"
        + include_str!("../../../shaders/tile_variation.wgsl")
        + include_str!("tests.wgsl");
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("actual terrain variation functions"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("check"),
        compilation_options: Default::default(),
        cache: None,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 128 * 8,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: output.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: output.as_entire_binding(),
        }],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(2, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output.size());
    queue.submit([encoder.finish()]);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(5)),
        })
        .unwrap();
    let mapped = readback.slice(..).get_mapped_range();
    let results: &[[u32; 2]] = bytemuck::cast_slice(&mapped);
    let mut chosen = std::collections::HashSet::new();
    for (i, [errors, layer]) in results.iter().copied().enumerate() {
        assert_eq!(
            errors, 0,
            "GPU variation case {i}: failure mask {errors:#x}"
        );
        chosen.insert(layer);
    }
    assert_eq!(
        chosen.len(),
        4,
        "spatial selection must exercise every alternative"
    );
}
