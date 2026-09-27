use std::sync::Arc;

use glam::{IVec3, Vec3};
use petramond::save::client::AntiAliasing;
use petramond::world::ReplicaWorld;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::chunk::{SectionPos, SECTION_VOLUME, SKY_FULL};
use petramond_world::section::Section;

use crate::camera::Camera;
use crate::world_marks::{WorldMark, WorldMarks};
use crate::Renderer;

const SIZE: (u32, u32) = (96, 64);

fn renderer() -> Option<Renderer> {
    let instance = wgpu::Instance::new(&crate::renderer::instance_descriptor());
    if pollster::block_on(instance.request_adapter(&Default::default())).is_err() {
        eprintln!("[skip] no wgpu adapter; world marks not rendered");
        return None;
    }
    let mut renderer = pollster::block_on(crate::new_offscreen_renderer(
        SIZE.0,
        SIZE.1,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ))
    .expect("offscreen renderer");
    renderer.set_hand_visible(false);
    renderer.set_crosshair_visible(false);
    Some(renderer)
}

fn camera(origin: IVec3) -> Camera {
    Camera::new(
        WorldPos::block_min(origin) + Vec3::new(8.0, 8.0, 4.0),
        SIZE.0 as f32 / SIZE.1 as f32,
    )
}

fn red_line(origin: IVec3, z: f32, occluded: f32) -> WorldMark {
    let at = |x: f32| WorldPos::block_min(origin) + Vec3::new(x, 8.0, z);
    WorldMark::Line {
        from: at(-40.0),
        to: at(56.0),
        color: [1.0, 0.0, 0.0, 1.0],
        width: 6.0,
        occluded,
    }
}

fn marks(items: Vec<WorldMark>) -> WorldMarks {
    WorldMarks {
        items,
        paint: Default::default(),
    }
}

fn red_pixels(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4)
        .filter(|p| p[0] > 150 && p[1] < 80 && p[2] < 80)
        .count()
}

#[test]
fn a_mark_hides_behind_the_world_unless_it_draws_through() {
    let origin = IVec3::new(160_000_000, 0, -160_000_000);
    let mut world = ReplicaWorld::with_pool(0, 1, Arc::new(petramond::worker::JobPool::inline()));
    let sp = SectionPos::new(origin.x / 16, 0, origin.z / 16);
    let mut section = Section::new(sp.cx, sp.cy, sp.cz);
    for x in 0..16 {
        for y in 0..16 {
            section.set_block(x, y, 8, Block::Stone);
        }
    }
    section.set_skylight(vec![SKY_FULL; SECTION_VOLUME].into());
    world.insert_section_for_test(sp, section);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while world.iter_meshes().next().is_none() {
        world.tick_mesh_budget(64);
        assert!(
            std::time::Instant::now() < deadline,
            "the wall did not mesh"
        );
        std::thread::yield_now();
    }
    let cam = camera(origin);
    for aa in [
        AntiAliasing::Off,
        AntiAliasing::Msaa4x,
        AntiAliasing::Ssaa4x,
    ] {
        let Some(mut renderer) = renderer() else {
            return;
        };
        renderer.set_anti_aliasing(aa);
        renderer.update_uniforms(&cam, [0.3, 0.4, 0.5], 0.0, None, None);
        world.redeliver_meshes();
        for _ in 0..8 {
            renderer.sync_meshes(&mut world.terrain_render_handoff());
        }
        let applied = renderer.anti_aliasing();
        let mut shown = |mark: WorldMark| {
            renderer.update_uniforms(&cam, [0.3, 0.4, 0.5], 0.0, None, None);
            renderer.set_world_marks(&marks(vec![mark]));
            red_pixels(&renderer.capture_frame().rgba)
        };
        assert_eq!(
            shown(red_line(origin, 12.0, 0.0)),
            0,
            "{applied:?}: seen through the wall"
        );
        assert!(
            shown(red_line(origin, 12.0, 1.0)) > 0,
            "{applied:?}: x-ray line hidden"
        );
        assert!(
            shown(red_line(origin, 6.0, 0.0)) > 0,
            "{applied:?}: line in front hidden"
        );
    }
}
