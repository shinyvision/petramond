//! Fluid meshing on SYNTHETIC rows: a pack staged for a child test process
//! adds its own fluids and tiles, so nothing here depends on authored water or
//! lava — and the fluids it adds are a third and fourth medium.

use super::fluid::{assert_meshes_from_its_row, face_of, fluid_quads, medium_slot};
use super::*;
use crate::face::Face;
use petramond_world::fluid_math::FALLING;

/// An opaque medium with untinted tiles.
const TAR: &str = "meshfluids:tar";
/// A see-through medium with biome-tinted tiles.
const MIST: &str = "meshfluids:mist";

fn fluid_row(name: &str, alpha: f32, tiles: &str) -> String {
    format!(
        r#"{{
        "block": "{name}",
        "fluid": {{
            "delay": 5, "drop_off": 1, "renewable": false,
            "motion": {{
                "speed_scale": 1.0, "accel": 8.0, "friction": 0.4, "rise": 1.5, "sink": 0.5,
                "vertical_accel": 6.0, "entry_friction": 0.5, "probe_fraction": 0.5,
                "probe_offset": 0.0, "climb": "jump"
            }},
            "medium": {{
                "fog_color": [0.2, 0.2, 0.2], "fog_start": 0.5, "fog_end": 8.0,
                "volume_tint": [1.0, 1.0, 1.0], "surface_tint": [1.0, 1.0, 1.0],
                "surface_alpha": {alpha:?}
            }}
        }},
        "shape": "cube", "flags": ["transparent", "fluid"], "tags": ["replaceable"],
        "behavior": "fluid", "interaction": "none", "collision": [], "emission": 0,
        "tiles": ["meshfluids_{tiles}_still", "meshfluids_{tiles}_still", "meshfluids_{tiles}_still"],
        "flow_tile": "meshfluids_{tiles}_flow",
        "material": "none", "hardness": -1, "drops": []
    }}"#
    )
}

/// A fresh mods root holding the `meshfluids` pack.
fn stage() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("petramond-mesh-fluids-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pack = root.join("mods/meshfluids");
    std::fs::create_dir_all(pack.join("textures")).unwrap();
    let write = |file: &str, text: &str| std::fs::write(pack.join(file), text).unwrap();
    write(
        "pack.json",
        r#"{"id": "meshfluids", "name": "Mesh Fluid Fixture", "version": "0.0.1"}"#,
    );
    write(
        "textures/atlas.json",
        r#"{"tiles": [
            {"name": "meshfluids_plain_still", "file": "stone.png"},
            {"name": "meshfluids_plain_flow", "file": "dirt.png"},
            {"name": "meshfluids_tinted_still", "file": "stone.png", "tint": "grass"},
            {"name": "meshfluids_tinted_flow", "file": "dirt.png", "tint": "grass"}
        ]}"#,
    );
    write(
        "blocks.json",
        &format!(
            r#"{{"blocks": [{}, {}]}}"#,
            fluid_row(TAR, 1.0, "plain"),
            fluid_row(MIST, 0.5, "tinted")
        ),
    );
    root
}

#[test]
fn synthetic_fluid_rows() {
    let root = stage();
    let out = std::process::Command::new(std::env::current_exe().expect("test binary path"))
        .args([
            "tests::synthetic_fluids::inner",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env("PETRAMOND_MODS", root.join("mods"))
        .output()
        .expect("spawn test binary");
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        out.status.success(),
        "inner test failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
#[ignore = "runs in a child process with the synthetic pack staged"]
fn inner() {
    let tar = block(TAR);
    let mist = block(MIST);
    assert!(
        petramond_world::fluid::medium::media().len() >= 3,
        "the fixture adds fluids beyond the shipped ones"
    );
    for fluid in [tar, mist] {
        assert_meshes_from_its_row(fluid);
    }
    an_opaque_top_under_a_lid_draws_only_when_recessed(tar, mist);
    only_a_full_opaque_medium_covers_the_faces_behind_it(tar, mist);
    biome_tinted_fluid_tiles_mark_the_section(tar, mist);
}

fn block(name: &str) -> Block {
    let id = petramond_world::registry::names()
        .blocks
        .id(name)
        .unwrap_or_else(|| panic!("fixture block {name} registered"));
    Block::from_id(id)
}

fn tops_at(m: &ChunkMesh, fluid: Block, x: f32) -> usize {
    fluid_quads(m, fluid)
        .iter()
        .filter(|q| face_of(&q[0]) == Face::PosY && q[0].pos[0].floor() == x)
        .count()
}

/// A FALLING cell fills its cell, so its opaque top is coplanar with a lid's
/// underside and both write depth: it must not draw. A recessed source top
/// sits below the lid and stays (both windings). A see-through top writes no
/// depth and keeps drawing under a lid either way.
fn an_opaque_top_under_a_lid_draws_only_when_recessed(opaque: Block, clear: Block) {
    let mut section = Section::new(0, 0, 0);
    section.set_fluid(4, 4, 4, opaque, FALLING);
    section.set_block(4, 5, 4, Block::Stone);
    section.set_fluid(8, 4, 8, opaque, 0);
    section.set_block(8, 5, 8, Block::Stone);
    section.set_fluid(12, 4, 12, clear, FALLING);
    section.set_block(12, 5, 12, Block::Stone);
    for m in [mesh(&section), mesh_via_pad(&section)] {
        assert_eq!(tops_at(&m, opaque, 4.0), 0, "full opaque top under a lid");
        assert_eq!(
            tops_at(&m, opaque, 8.0),
            2,
            "recessed opaque top under a lid"
        );
        assert_eq!(tops_at(&m, clear, 12.0), 1, "see-through top under a lid");
    }
}

/// Stone's face toward a FULL cell of an opaque medium is hidden like a face
/// toward stone, on the closure path and on the pad path's exposure masks; a
/// see-through medium hides nothing.
fn only_a_full_opaque_medium_covers_the_faces_behind_it(opaque: Block, clear: Block) {
    for (fluid, covers) in [(opaque, true), (clear, false)] {
        let mut section = Section::new(0, 0, 0);
        section.set_block(4, 4, 4, Block::Stone);
        // Capped from above, so the cell beside the stone is full.
        section.set_fluid(5, 4, 4, fluid, 0);
        section.set_fluid(5, 5, 4, fluid, 0);
        for m in [mesh(&section), mesh_via_pad(&section)] {
            let stone_east = m
                .opaque
                .chunks_exact(4)
                .filter(|q| {
                    medium_slot(&q[0]) == 0 && q.iter().all(|v| v.pos[0] == 5.0 && v.pos[1] <= 5.0)
                })
                .count();
            assert_eq!(
                stone_east,
                usize::from(!covers),
                "{fluid:?} covers: {covers}"
            );
        }
    }
}

/// Whether a section needs biome tints derives from the tiles a row draws.
fn biome_tinted_fluid_tiles_mark_the_section(plain: Block, tinted: Block) {
    for (fluid, expect) in [(plain, false), (tinted, true)] {
        let mut section = Section::new(0, 0, 0);
        section.set_fluid(1, 1, 1, fluid, 0);
        assert_eq!(section.has_biome_tint_blocks(), expect, "{fluid:?}");
    }
}
