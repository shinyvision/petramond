//! Registry-driven flipbooks; static tiles pay no second texture sample.

use petramond_world::tile;
use std::fmt::Write;

pub(super) fn declarations() -> String {
    let mut text = String::from("fn tile_animation(tile: u32) -> vec3<f32> {\n switch tile {\n");
    let water = tile::engine();
    for (id, cell) in tile::cells().iter().enumerate() {
        if cell.anim_frames == 0 {
            continue;
        }
        let speed = if id == water.water_still.index() {
            "WATER_STILL_FPS".to_owned()
        } else if id == water.water_flow.index() {
            "WATER_FLOW_FPS".to_owned()
        } else {
            format!("{:.8}", 20.0 / f64::from(cell.frame_ticks))
        };
        writeln!(
            text,
            "case {id}u: {{ return vec3<f32>({}.0, {speed}, {}.0); }}",
            cell.anim_frames,
            u8::from(cell.interpolate)
        )
        .unwrap();
    }
    text.push_str("default: { return vec3<f32>(1.0, 0.0, 0.0); }\n }\n}\n");
    text
}

pub(super) fn model_declarations() -> String {
    let mut text = String::from("fn model_animation(id: u32) -> vec4<f32> {\n switch id {\n");
    for (id, a) in petramond_world::block_model::atlas()
        .animations()
        .iter()
        .enumerate()
        .skip(1)
    {
        writeln!(
            text,
            "case {id}u: {{ return vec4<f32>({:.12}, {}.0, {:.8}, {}.0); }}",
            a.stride,
            a.frames,
            20.0 / f64::from(a.frame_ticks),
            u8::from(a.interpolate)
        )
        .unwrap();
    }
    text.push_str("default: { return vec4<f32>(0.0, 1.0, 0.0, 0.0); }\n }\n}\n");
    text
}
