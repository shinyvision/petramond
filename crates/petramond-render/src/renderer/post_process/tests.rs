use super::*;

#[test]
fn supersampling_keeps_integer_footprints_through_odd_resizes_and_low_render_scale() {
    for size in [(1915, 1041), (1, 1), (3839, 2161)] {
        for mode in [AntiAliasing::Ssaa4x, AntiAliasing::Ssaa16x] {
            let axis = mode.resolution_multiplier();
            for low_scale in [0.5, 0.75, 1.0] {
                assert_eq!(
                    scene_dimensions(size, low_scale, mode),
                    (size.0 * axis, size.1 * axis)
                );
            }
        }
    }
    assert_eq!(
        scene_dimensions((100, 60), 0.5, AntiAliasing::Off),
        (50, 30)
    );
    assert_eq!(scene_dimensions((1, 1), 0.5, AntiAliasing::Off), (1, 1));
}

#[test]
fn unsupported_sample_counts_fall_back_to_a_complete_pixel_footprint() {
    for size in [(1915, 1041), (3840, 2160), (7680, 4320)] {
        let limit = 8192;
        let max = max_multiplier(size, limit);
        let mode = supported_mode(AntiAliasing::Ssaa16x, max, 4);
        let dimensions = scene_dimensions(size, 1.0, mode);
        assert!(dimensions.0 <= limit && dimensions.1 <= limit);
        assert_eq!(supported_mode(AntiAliasing::Off, max, 4), AntiAliasing::Off);
    }
}

#[test]
fn msaa_keeps_native_dimensions_and_falls_back_only_in_sample_count() {
    for mode in [AntiAliasing::Msaa4x, AntiAliasing::Msaa8x] {
        assert_eq!(scene_dimensions((1915, 1041), 0.5, mode), (1915, 1041));
        assert_eq!(supported_mode(mode, 1, 4), AntiAliasing::Msaa4x);
        assert_eq!(supported_mode(mode, 1, 1), AntiAliasing::Off);
    }
    assert_eq!(
        supported_mode(AntiAliasing::Msaa8x, 1, 8),
        AntiAliasing::Msaa8x
    );
}

#[test]
fn scene_resolves_after_mode_switches_resizes_and_grade_changes() {
    let instance = wgpu::Instance::new(&crate::renderer::instance_descriptor());
    if pollster::block_on(instance.request_adapter(&Default::default())).is_err() {
        eprintln!("[skip] no wgpu adapter; scene lifecycle validation not run");
        return;
    }
    let mut renderer = pollster::block_on(crate::new_offscreen_renderer(
        65,
        37,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ));
    for mode in [
        AntiAliasing::Off,
        AntiAliasing::Msaa4x,
        AntiAliasing::Msaa8x,
        AntiAliasing::Ssaa4x,
        AntiAliasing::Msaa4x,
    ] {
        let applied = renderer.set_anti_aliasing(mode);
        assert!(applied.sample_count() <= renderer.max_anti_aliasing_samples());
        for grade in [false, true] {
            renderer.set_grade_enabled(grade);
            renderer.resize(67, 39);
            let frame = renderer.capture_frame();
            assert_eq!((frame.width, frame.height), (67, 39));
            assert_eq!(frame.rgba.len(), 67 * 39 * 4);
            renderer.resize(65, 37);
        }
    }
}
