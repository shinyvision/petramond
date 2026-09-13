//! Synthetic fluid and body rows for the behaviour tests that must not depend
//! on authored water, lava or species tuning. The pack is staged for a child
//! test process, where its rows join the real registries.

use std::path::PathBuf;

use petramond_math::math::Vec3;
use petramond_world::block::Block;
use petramond_world::chunk::{Chunk, ChunkPos, SectionPos};
use petramond_world::section::Section;

use crate::world::World;

/// A jump-climb fluid with a current.
pub const BRINE: &str = "bodyfluid:brine";
/// A viscous step-climb fluid that is a navigation hazard, deals contact
/// damage and applies burning.
pub const SYRUP: &str = "bodyfluid:syrup";
/// A solid, walkable floor block that is a navigation hazard but no fluid.
pub const CINDER: &str = "bodyfluid:cinder";
/// Top of the pool floor in [`pool`]; fluid fills the cells above it.
pub const FLOOR_Y: i32 = 64;

/// Stage the `bodyfluid` pack (the two fluids, the hazardous floor, one
/// humanoid-sized body per buoyancy mode plus two tolerant species) in a
/// fresh mods root.
pub fn stage(tag: &str) -> PathBuf {
    let root = crate::modding::tests::stage_mods_fixture(tag, &[]).expect("fixture root");
    let pack = root.join("mods/bodyfluid");
    std::fs::create_dir_all(&pack).unwrap();
    let write = |file: &str, value: serde_json::Value| {
        std::fs::write(pack.join(file), value.to_string()).unwrap();
    };
    write(
        "pack.json",
        serde_json::json!({"id": "bodyfluid", "name": "Body Fluid Fixture", "version": "0.0.1"}),
    );
    write(
        "blocks.json",
        serde_json::json!({"blocks": [
            fluid_row(BRINE, "jump", 1.0, 1.0 / 3.0, 0.0, serde_json::json!({}), &[], 0.75),
            fluid_row(
                SYRUP,
                "step",
                0.4,
                0.0,
                0.05,
                serde_json::json!({
                    "damage": {"amount": 1, "interval": 10},
                    "applies": {"condition": "petramond:burning", "stage": "light", "ticks": 40}
                }),
                &["nav_hazard"],
                0.0,
            ),
            hazard_floor_row(CINDER),
        ]}),
    );
    write(
        "mobs.json",
        serde_json::json!({"mobs": [
            body_row("swim", "swim", serde_json::json!({})),
            body_row("surface", "surface", serde_json::json!({})),
            body_row("neutral", "neutral", serde_json::json!({})),
            body_row("dweller", "swim", serde_json::json!({"blocks": [SYRUP]})),
            body_row("fireproof", "swim", serde_json::json!({"conditions": ["petramond:burning"]})),
        ]}),
    );
    root
}

/// A complete synthetic fluid block row with the given motion knobs and
/// `contact` object.
#[allow(clippy::too_many_arguments)]
pub fn fluid_row(
    name: &str,
    climb: &str,
    speed_scale: f32,
    probe_fraction: f32,
    probe_offset: f32,
    contact: serde_json::Value,
    tags: &[&str],
    current: f32,
) -> serde_json::Value {
    let mut tags: Vec<&str> = tags.to_vec();
    tags.push("replaceable");
    serde_json::json!({
        "block": name,
        "fluid": {
            "delay": 5, "drop_off": 1, "renewable": false,
            "motion": {
                "speed_scale": speed_scale, "accel": 8.0, "friction": 0.4,
                "rise": 1.5, "sink": 0.5, "vertical_accel": 6.0,
                "entry_friction": 0.5, "probe_fraction": probe_fraction,
                "probe_offset": probe_offset, "climb": climb
            },
            "current": {"speed": current, "accel": 9.0},
            "contact": contact,
            "medium": {
                "fog_color": [0.2, 0.2, 0.2], "fog_start": 0.5, "fog_end": 8.0,
                "volume_tint": [1.0, 1.0, 1.0], "surface_tint": [1.0, 1.0, 1.0],
                "surface_alpha": 1.0
            }
        },
        "shape": "cube", "flags": ["transparent", "fluid"], "tags": tags,
        "behavior": "fluid", "interaction": "none", "collision": [], "emission": 0,
        "tiles": ["water_still", "water_still", "water_still"], "flow_tile": "water_flow",
        "material": "none", "hardness": -1, "drops": []
    })
}

fn hazard_floor_row(name: &str) -> serde_json::Value {
    serde_json::json!({
        "block": name,
        "shape": "cube", "flags": ["solid", "opaque"], "tags": ["nav_hazard"],
        "behavior": "inert", "interaction": "none",
        "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0,
        "tiles": ["stone", "stone", "stone"],
        "material": "stone", "hardness": 1.5, "drops": []
    })
}

fn body_row(name: &str, buoyancy: &str, tolerates: serde_json::Value) -> serde_json::Value {
    let key = format!("bodyfluid:{name}");
    serde_json::json!({
        "mob": key, "key": key, "model": "models/owl.bbmodel", "scale": 1.0,
        "size": {"half_width": 0.3, "height": 1.8},
        "tags": {"petramond:health": 20.0},
        "walk_speed": 3.0, "jump_speed": 8.0, "turn_rate": 8.0, "walk_anim_rate": 1.0,
        "category": "passive", "cap": 1,
        "spawn": {"biomes": [], "ground": []},
        "spawn_group": {"min": 1, "max": 1},
        "wander": {"chance_per_tick": 0.0, "radius": 1},
        "habitat": {"avoid": [], "prefer": []},
        "avoid_fluids": false, "buoyancy": buoyancy, "tolerates": tolerates,
        "brain": []
    })
}

/// A registered fixture block by name.
pub fn block(name: &str) -> Block {
    serde_json::from_value(serde_json::json!(name)).expect("fixture block registered")
}

/// A 16×16 pool of `fluid` from [`FLOOR_Y`] up to and including `top`.
pub fn pool(fluid: Block, top: i32) -> World {
    let mut chunk = Chunk::new(0, 0);
    for z in 0..16 {
        for x in 0..16 {
            chunk.set_block(x, (FLOOR_Y - 1) as usize, z, Block::Stone);
            for y in FLOOR_Y..=top {
                chunk.set_block(x, y as usize, z, fluid);
            }
        }
    }
    let mut world = World::new(0, 1);
    world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
    world
}

/// Raise stone from the floor up to `top` (inclusive) for every `x >= from_x`.
pub fn bank(world: &mut World, from_x: i32, top: i32) {
    for z in 0..16 {
        for x in from_x..16 {
            for y in FLOOR_Y..=top {
                world.set_block_world(x, y, z, Block::Stone);
            }
        }
    }
}

/// Where a body starts beside the bank in a pool whose top fluid cell is `top`.
pub fn beside_bank(from_x: i32, top: i32) -> Vec3 {
    Vec3::new(from_x as f32 - 1.0, top as f32 - 0.5, 8.5)
}

/// Write fluid `meta` for `fluid` into the cell at [`FLOOR_Y`].
pub fn set_flow(world: &mut World, x: i32, z: i32, fluid: Block, meta: u8) {
    if world.section_at_world_mut_for_test(x, FLOOR_Y, z).is_none() {
        let (cx, cy, cz) = (x.div_euclid(16), FLOOR_Y.div_euclid(16), z.div_euclid(16));
        world.insert_section_for_test(SectionPos::new(cx, cy, cz), Section::new(cx, cy, cz));
    }
    world
        .section_at_world_mut_for_test(x, FLOOR_Y, z)
        .unwrap()
        .set_fluid(
            x.rem_euclid(16) as usize,
            FLOOR_Y.rem_euclid(16) as usize,
            z.rem_euclid(16) as usize,
            fluid,
            meta,
        );
}

/// A one-deep brine sheet flowing toward +X: a source at `x = 4`, one level
/// thinner per cell out to `x = 10`.
pub fn flowing_brine() -> World {
    let brine = block(BRINE);
    let mut world = pool(Block::Air, FLOOR_Y - 1);
    for x in 4..=10 {
        for z in 0..16 {
            set_flow(&mut world, x, z, brine, (x - 4) as u8);
        }
    }
    world
}
