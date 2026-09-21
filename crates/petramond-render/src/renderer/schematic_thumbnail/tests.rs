use super::*;
use glam::IVec3;
use petramond::schematic::{Scene, Selection};
use petramond::{
    schematic::{CellData, ResolvedCell, Schematic, SchematicCell},
    world::World,
};
use petramond_math::facing::Facing;
use petramond_world::{
    block::{Block, CellCodec, ShapeFamily, ShapeState},
    block_state::{EntityFront, StairHalf, StairState},
};
use std::path::PathBuf;
use std::sync::Arc;

#[test]
#[ignore = "manual offscreen schematic screenshot; creates a GPU renderer"]
fn schematic_thumbnail_smoke() {
    let mut cells = Vec::new();
    let mut put = |p: [i32; 3], block, state| {
        cells.push(SchematicCell {
            pos: p,
            data: CellData::capture(&ResolvedCell {
                block,
                state,
                fluid: 0,
                kv: Default::default(),
                container: None,
                furnace: None,
            }),
        })
    };
    for x in 0..5 {
        for z in 0..5 {
            put([x, 0, z], Block::OakPlanks, ShapeState::NONE);
        }
    }
    for y in 1..4 {
        for p in [[0, y, 0], [4, y, 0], [0, y, 4], [4, y, 4]] {
            put(p, Block::OakLog, ShapeState::NONE);
        }
    }
    let stair = *Block::all()
        .iter()
        .find(|b| b.shape_family() == ShapeFamily::Stair)
        .unwrap();
    for x in 0..5 {
        for z in [0, 4] {
            put(
                [x, 4, z],
                stair,
                StairState::new(
                    if z == 0 { Facing::North } else { Facing::South },
                    StairHalf::Bottom,
                )
                .to_cell(),
            );
        }
    }
    put(
        [3, 1, 2],
        Block::Chest,
        EntityFront(Facing::South).to_cell(),
    );
    let bed = Block::Bed;
    for (pos, offset) in petramond_world::block_model::oriented_footprint_cells(
        IVec3::new(1, 1, 1),
        bed.model_kind().unwrap(),
        Facing::South,
    ) {
        put(
            pos.to_array(),
            bed,
            petramond_world::block_model::ModelCellState {
                offset,
                facing: Facing::South,
            }
            .to_cell(),
        );
    }
    let schematic = Schematic::from_cells("Thumbnail fixture".into(), [5, 5, 5], cells).unwrap();
    let scene = Scene::prepare(&schematic, 0, |_: &mut World| {}).unwrap();
    let mut renderer = pollster::block_on(crate::new_offscreen_renderer(
        960,
        640,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ));
    let path = std::env::var_os("PETRAMOND_SCHEMATIC_CAPTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("petramond-schematic-preview.png"));
    let geometry = Geometry::build(&scene);
    assert!(
        !geometry.model_indices.is_empty(),
        "the preview includes placed model geometry"
    );
    let png = renderer
        .schematic_thumbnailer()
        .render_geometry(&geometry)
        .unwrap();
    std::fs::write(&path, &png).unwrap();
    let archive = path.with_extension(petramond::schematic::archive::EXTENSION);
    if archive.exists() {
        std::fs::remove_file(&archive).unwrap();
    }
    petramond::schematic::library::save_as(&archive, &schematic, &png).unwrap();
    assert_eq!(
        petramond::schematic::library::thumbnail_bytes(
            &petramond::schematic::library::inspect(&archive).unwrap()
        )
        .unwrap(),
        png
    );
    let image = image::open(&path).unwrap();
    assert_eq!((image.width(), image.height()), (384, 256));
    eprintln!("Schematic screenshot: {}", path.display());
    let scene = Arc::new(scene);
    let mut selection = Selection::default();
    selection.region([0, 0, 0], [4, 4, 4], false).unwrap();
    selection.region([5, 0, 0], [5, 0, 0], false).unwrap();
    // An inline pool meshes the preview inside the call that asks for it.
    let jobs = petramond::worker::JobPool::inline();
    renderer.set_selection_overlay(Some(&selection), None, None);
    renderer.set_schematic_preview(&jobs, Some(scene.clone()), Some([0; 3]));
    renderer.set_schematic_preview(&jobs, Some(scene), Some([0; 3]));
    let mut cam = crate::camera::Camera::new(
        petramond_math::world_pos::WorldPos::new(10.0, 7.5, 12.0),
        1.5,
    );
    let forward = (glam::Vec3::new(2.5, 2.0, 2.5) - cam.pos.relative_to(IVec3::ZERO)).normalize();
    cam.fov_y = 45f32.to_radians();
    cam.yaw = forward.x.atan2(forward.z);
    cam.pitch = forward.y.asin();
    renderer.update_uniforms(&cam, [0.3, 0.4, 0.5], 0.0, None, None);
    renderer.set_hand_visible(false);
    renderer.set_crosshair_visible(false);
    let frame = renderer.capture_frame();
    image::save_buffer(
        path.with_file_name("schematic-ghost.png"),
        &frame.rgba,
        frame.width,
        frame.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
    renderer.set_schematic_preview(&jobs, None, None);
    let frame = renderer.capture_frame();
    image::save_buffer(
        path.with_file_name("schematic-outline.png"),
        &frame.rgba,
        frame.width,
        frame.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
}
