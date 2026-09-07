use super::*;

#[test]
fn supersampling_integrates_subpixel_stripes_without_blurring_aligned_edges() {
    let instance = wgpu::Instance::new(&crate::renderer::instance_descriptor());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("[skip] no wgpu adapter; supersampling rendering not run");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let (resolve, bgl) = create_grade_pipeline(&device, format);
    let fixture = shader_module(
        &device,
        "subpixel stripe fixture",
        crate::pipeline::GRADE_SHADER.to_owned()
            + "
        @fragment fn fs_fixture(in: VsOut) -> @location(0) vec4<f32> {
            var value = select(0.1, 0.9, fract(in.uv.x * 64.0) < 0.5);
            if in.uv.y < 0.5 { value = select(0.1, 0.9, in.uv.x >= 0.5); }
            return vec4<f32>(vec3<f32>(value), 1.0);
        }",
    );
    let fixture_pipe = single_pipeline(
        &device,
        "stripe fixture",
        &pipeline_layout(&device, "fixture layout", &[]),
        &fixture,
        "vs_grade",
        "fs_fixture",
        &[],
        &color_target(format, None, wgpu::ColorWrites::ALL),
        Default::default(),
        None,
    );
    let controls = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let output = target(&device, 64, format);
    let output_view = output.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 64 * 64 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut images = Vec::new();
    for (axis, grade) in [(1, 0.0), (2, 0.0), (4, 0.0), (4, 1.0)] {
        let source = target(&device, 64 * axis, format);
        let source_view = source.create_view(&Default::default());
        let bind = create_grade_bind(&device, &bgl, &source_view, &controls);
        queue.write_buffer(
            &controls,
            0,
            bytemuck::cast_slice(&[0.0_f32, 0.0, axis as f32, grade]),
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        draw(&mut encoder, &source_view, &fixture_pipe, None);
        draw(&mut encoder, &output_view, &resolve, Some(&bind));
        encoder.copy_texture_to_buffer(
            output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            output.size(),
        );
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
        images.push(readback.slice(..).get_mapped_range().to_vec());
        readback.unmap();
    }
    for image in &images[1..3] {
        for y in 0..31 {
            let row = y * 64 * 4;
            assert!(
                max_channel_diff(&image[row..row + 256], &images[0][row..row + 256]) <= 1,
                "a pixel-aligned edge and flat regions must remain sharp"
            );
        }
        for y in 33..64 {
            for x in 1..63 {
                let encoded = image[(y * 64 + x) * 4] as f32 / 255.0;
                let linear = ((encoded + 0.055) / 1.055).powf(2.4);
                assert!(
                    (linear - 0.5).abs() < 0.01,
                    "half bright/half dark stripes must average in linear light: {linear}"
                );
            }
        }
    }
    assert_ne!(
        images[0], images[1],
        "single-sample aliases must be resolved"
    );
    assert!(
        max_channel_diff(&images[1], &images[2]) <= 1,
        "both sample densities integrate the same stripe coverage"
    );
    assert_ne!(
        images[2], images[3],
        "grade must remain independently switchable"
    );
}

/// Largest per-channel difference between two RGBA8 images. The reductions
/// are compared to within one 8-bit step: bilinear taps landing on texel
/// boundaries round differently across drivers, and a one-step wobble is not
/// the aliasing or blur this test guards against.
fn max_channel_diff(a: &[u8], b: &[u8]) -> u8 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

fn target(device: &wgpu::Device, size: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn draw(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    bind: Option<&wgpu::BindGroup>,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    if let Some(bind) = bind {
        pass.set_bind_group(0, bind, &[]);
    }
    pass.draw(0..3, 0..1);
}
