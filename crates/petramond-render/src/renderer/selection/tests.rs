use super::*;
use glam::Vec3;
use petramond::world::World;
use petramond::{save::client::AntiAliasing, world::WorldRole};
use petramond_math::facing::Facing;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::{Block, CellCodec, ShapeFamily};
use petramond_world::block_state::{StairHalf, StairState};
use petramond_world::{
    chunk::{SectionPos, SECTION_VOLUME, SKY_FULL},
    section::Section,
};
use std::path::PathBuf;
use std::sync::Arc;

#[test]
#[ignore = "manual native selection highlight comparison"]
fn selection_highlight_visual_check() {
    let dir =
        PathBuf::from(std::env::var_os("PETRAMOND_SELECTION_CAPTURE").expect("capture directory"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut renderer = pollster::block_on(crate::new_offscreen_renderer(
        1280,
        960,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ));
    renderer.set_hand_visible(false);
    renderer.set_crosshair_visible(false);
    let save = |name: &str, frame: &crate::renderer::RenderedFrame| {
        image::save_buffer(
            dir.join(name),
            &frame.rgba,
            frame.width,
            frame.height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    };
    for (name, origin, aa) in [
        ("near", IVec3::ZERO, AntiAliasing::Off),
        ("near-msaa", IVec3::ZERO, AntiAliasing::Msaa4x),
        (
            "far",
            IVec3::new(25_000_000, 0, -25_000_000),
            AntiAliasing::Msaa4x,
        ),
    ] {
        renderer.set_anti_aliasing(aa);
        let mut world = World::new_with_pool(
            0,
            1,
            WorldRole::ClientReplica,
            Arc::new(petramond::worker::JobPool::inline()),
        );
        let sp = SectionPos::new(origin.x / 16, 0, origin.z / 16);
        let mut section = Section::new(sp.cx, sp.cy, sp.cz);
        for x in 2..13 {
            for z in 2..13 {
                section.set_block(x, 0, z, Block::Grass);
            }
        }
        for x in 3..12 {
            for y in 1..4 {
                section.set_block(x, y, 4, Block::Stone);
            }
        }
        let stair = *Block::all()
            .iter()
            .find(|b| b.shape_family() == ShapeFamily::Stair)
            .unwrap();
        let fence = *Block::all()
            .iter()
            .find(|b| b.shape_family() == ShapeFamily::Fence)
            .unwrap();
        for x in 3..7 {
            section.set_block(x, 1, 8, stair);
            section.set_cell_state(
                x,
                1,
                8,
                StairState::new(Facing::South, StairHalf::Bottom).to_cell(),
            );
        }
        for x in 8..11 {
            section.set_block(x, 1, 8, fence);
        }
        section.set_block(5, 1, 10, Block::OakLeaves);
        section.set_block(9, 1, 10, Block::OakLeaves);
        for (p, offset) in petramond_world::block_model::oriented_footprint_cells(
            IVec3::new(7, 1, 6),
            Block::Bed.model_kind().unwrap(),
            Facing::South,
        ) {
            section.set_block(p.x as usize, p.y as usize, p.z as usize, Block::Bed);
            section.set_cell_state(
                p.x as usize,
                p.y as usize,
                p.z as usize,
                petramond_world::block_model::ModelCellState {
                    offset,
                    facing: Facing::South,
                }
                .to_cell(),
            );
        }
        section.set_skylight(vec![SKY_FULL; SECTION_VOLUME].into());
        world.insert_section_for_test(sp, section);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while world.iter_meshes().next().is_none() {
            world.tick_mesh_budget(64);
            assert!(std::time::Instant::now() < deadline, "fixture did not mesh");
            std::thread::yield_now();
        }
        let eye = Vec3::new(17.0, 13.0, 21.0);
        let mut cam = crate::camera::Camera::new(WorldPos::block_min(origin) + eye, 1280.0 / 960.0);
        let forward = (Vec3::new(7.0, 1.5, 7.0) - eye).normalize();
        cam.fov_y = 45f32.to_radians();
        cam.yaw = forward.x.atan2(forward.z);
        cam.pitch = forward.y.asin();
        renderer.set_selection_overlay(None, None, None);
        renderer.update_uniforms(&cam, [0.3, 0.4, 0.5], 0.0, None, None);
        for _ in 0..8 {
            renderer.sync_meshes(&mut world.terrain_render_handoff());
        }
        assert!(
            !renderer.terrain.columns.is_empty(),
            "fixture did not upload"
        );
        let before = renderer.capture_frame();
        save(&format!("{name}-before.png"), &before);
        let mut selection = Selection::default();
        let p = |v| (origin + IVec3::from_array(v)).to_array();
        selection
            .region(p([4, 0, 4]), p([8, 3, 10]), false)
            .unwrap();
        selection.region(p([6, 0, 6]), p([6, 3, 7]), true).unwrap();
        selection.region(p([9, 0, 8]), p([9, 1, 8]), false).unwrap();
        renderer.set_selection_overlay(Some(&selection), None, None);
        renderer.update_uniforms(&cam, [0.3, 0.4, 0.5], 0.0, None, None);
        let after = renderer.capture_frame();
        save(&format!("{name}-selected.png"), &after);
        assert_ne!(before.rgba, after.rgba);
        let pixel = |frame: &crate::renderer::RenderedFrame, local: Vec3| {
            let relative_eye = cam.pos.relative_to(renderer.view.render_origin);
            let vp =
                cam.proj() * Mat4::look_at_rh(relative_eye, relative_eye + cam.forward(), Vec3::Y);
            let clip = vp
                * (WorldPos::block_min(origin) + local)
                    .relative_to(renderer.view.render_origin)
                    .extend(1.0);
            let ndc = clip.truncate() / clip.w;
            let x = ((ndc.x * 0.5 + 0.5) * frame.width as f32) as usize;
            let y = ((0.5 - ndc.y * 0.5) * frame.height as f32) as usize;
            frame.rgba[(y * frame.width as usize + x) * 4..][..3]
                .iter()
                .map(|c| u32::from(*c))
                .sum::<u32>()
        };
        let selected = Vec3::new(5.5, 4.0, 4.5);
        let outside = Vec3::new(10.5, 4.0, 4.5);
        assert!(
            pixel(&after, selected) > pixel(&before, selected),
            "selected surface did not brighten"
        );
        assert_eq!(
            pixel(&after, outside),
            pixel(&before, outside),
            "unselected surface changed"
        );
        let face = selection
            .surface()
            .pick(
                WorldPos::block_min(origin) + Vec3::new(4.5, 12.0, 4.5),
                -Vec3::Y,
                128.0,
            )
            .unwrap()
            .0
            .clone();
        renderer.set_selection_overlay(Some(&selection), None, Some(&face));
        renderer.update_uniforms(&cam, [0.3, 0.4, 0.5], 0.0, None, None);
        save(&format!("{name}-face.png"), &renderer.capture_frame());
        selection.begin_extrusion(face);
        for (suffix, offset) in [("extruded", 2), ("contracted", -2)] {
            selection.extrude(offset).unwrap();
            let face = selection.extrusion_face().unwrap();
            renderer.set_selection_overlay(Some(&selection), None, Some(&face));
            renderer.update_uniforms(&cam, [0.3, 0.4, 0.5], 0.0, None, None);
            save(&format!("{name}-{suffix}.png"), &renderer.capture_frame());
        }
        selection.cancel_extrusion();
        selection.clear();
        renderer.set_selection_overlay(Some(&selection), None, None);
        let cleared = renderer.capture_frame();
        assert_eq!(
            before.rgba, cleared.rgba,
            "clearing selection must restore the image"
        );
    }
}
