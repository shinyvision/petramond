use std::path::PathBuf;

use super::*;
use crate::block_model::placement_transform;
use crate::chunk::ChunkPos;
use crate::facing::Facing;
use crate::world::test_world::TestWorld;

struct Pack(PathBuf);

impl Drop for Pack {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One cell of body with a lip hanging half a cell past its +X side.
fn shelf_pack() -> Pack {
    let root = std::env::temp_dir().join(format!("petramond-overhang-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("mods/fx");
    std::fs::create_dir_all(dir.join("models")).unwrap();
    std::fs::create_dir_all(dir.join("textures")).unwrap();
    let tex = crate::bbmodel::tests::one_pixel_texture([255, 255, 255, 255]);
    let faces = r#"{"north": {"uv": [0,0,16,16], "texture": 0}, "east": {"uv": [0,0,16,16], "texture": 0},
        "south": {"uv": [0,0,16,16], "texture": 0}, "west": {"uv": [0,0,16,16], "texture": 0},
        "up": {"uv": [0,0,16,16], "texture": 0}, "down": {"uv": [0,0,16,16], "texture": 0}}"#;
    let files = [
        ("pack.json", r#"{"id": "fx", "name": "fx"}"#.to_string()),
        (
            "models.json",
            r#"{"models": [{"key": "fx:shelf", "model_file": "models/shelf.bbmodel",
                "cells": [1, 1, 1], "fit": "native"}]}"#
                .to_string(),
        ),
        (
            "blocks.json",
            r#"{"blocks": [{"block": "fx:shelf", "shape": {"model": "fx:shelf"}, "flags": ["solid"],
                "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0,
                "tiles": ["oak_planks", "oak_planks", "oak_planks"], "material": "wood",
                "hardness": 1.0, "drops": []}]}"#
                .to_string(),
        ),
        (
            "models/shelf.bbmodel",
            format!(
                r#"{{"meta": {{"format_version": "5.0", "model_format": "java_block", "box_uv": false}},
                "resolution": {{"width": 16, "height": 16}},
                "textures": [{{"uv_width": 16, "uv_height": 16, "source": "{tex}"}}],
                "elements": [
                    {{"uuid": "body", "name": "body", "type": "cube", "from": [0,0,0], "to": [16,8,16], "faces": {faces}}},
                    {{"uuid": "lip", "name": "lip", "type": "cube", "from": [16,4,4], "to": [24,8,12], "faces": {faces}}}
                ],
                "outliner": ["body", "lip"]}}"#
            ),
        ),
    ];
    for (file, text) in files {
        std::fs::write(dir.join(file), text).unwrap();
    }
    Pack(root)
}

/// Overhang is geometry in a cell the model doesn't own, so the DDA only
/// meets it from that cell. Each case is checked at every facing, because the
/// neighbour has to map the asking cell back through its placement.
#[test]
fn a_models_overhang_aims_at_the_model_but_never_collides() {
    let pack = shelf_pack();
    let content = crate::content::test_support::with_mods(&pack.0.join("mods"));
    let _pin = crate::content::pin(content);
    let shelf = Block(
        content
            .names()
            .blocks
            .id("fx:shelf")
            .expect("fixture block loads"),
    );
    let kind = shelf.model_kind().expect("fixture block is a model");

    let mut world = TestWorld::new(1);
    world.insert_empty_column(ChunkPos::new(0, 0));
    let at = IVec3::new(8, 64, 8);
    for x in 5..12 {
        for z in 5..12 {
            world.set_block_world(x, 63, z, Block::Stone);
        }
    }
    let down = Vec3::NEG_Y;
    for facing in [Facing::North, Facing::East, Facing::South, Facing::West] {
        world.set_block_world(at.x, at.y, at.z, shelf);
        let (section, lx, ly, lz) = world.data.chunk_at_world_mut(at.x, at.y, at.z).unwrap();
        section.set_model_facing(lx, ly, lz, facing);

        let place = placement_transform(kind, facing);
        let world_point = |p: Vec3| {
            let w = place.transform_point3(p) + at.as_vec3();
            WorldPos::new(w.x as f64, w.y as f64, w.z as f64)
        };
        let lip = world_point(Vec3::new(1.25, 0.375, 0.5));
        let bare = world_point(Vec3::new(-0.25, 0.375, 0.5));
        let above = |p: WorldPos| WorldPos::new(p.x, p.y + 2.0, p.z);

        let (hit, _) = with_dist(above(lip), down, &world.data).expect("the lip is aimed at");
        assert_eq!(
            hit.block, at,
            "{facing:?}: aiming down at the lip selects the model"
        );

        let (hit, _) = with_dist(above(bare), down, &world.data).expect("the floor is aimed at");
        assert_eq!(
            hit.block.y, 63,
            "{facing:?}: the bare side still reaches the floor"
        );

        let (hit, _) = filtered(above(lip), down, REACH, RayFilter::Collidable, &world.data)
            .expect("the floor collides");
        assert_eq!(hit.block.y, 63, "{facing:?}: the lip never collides");

        let out = place.transform_vector3(Vec3::X);
        let eye = WorldPos::new(
            lip.x + 3.0 * out.x as f64,
            lip.y,
            lip.z + 3.0 * out.z as f64,
        );
        let (hit, _) = with_dist(eye, -out, &world.data).expect("the lip end is aimed at");
        assert_eq!(
            (hit.block, hit.normal),
            (at, out.round().as_ivec3()),
            "{facing:?}: the lip's end reads as the model's face on that side"
        );

        world.set_block_world(at.x, at.y, at.z, Block::Air);
    }
}
