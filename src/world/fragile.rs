//! The fragile-block break behaviour: plants and torches that cannot stand once the
//! support they rest on is gone.
//!
//! Lives here in `world` (not `block`) for the same reason water does — it drives the
//! world tick scheduler (`World::schedule_block_tick`) and the natural-break hand-off
//! ([`World::note_block_destroyed`]), world internals a `block`-side behaviour can't
//! reach — while still implementing the `block`-defined `BlockBehavior`. Carried by
//! every block tagged [`BlockTag::FRAGILE`](petramond_world::block::BlockTag::FRAGILE) (see
//! `block::data`): the tag is the categorisation (the water sim reads it to know which
//! cells it may flow into), this behaviour is what such a block DOES when its support
//! changes.

use petramond_math::math::IVec3;
use petramond_world::block::Block;

use super::store::World;

/// Ticks a now-unsupported fragile block waits before it breaks: the next tick. The
/// break resolves on the deterministic game tick *after* the change that undercut it,
/// never mid-frame — the same scheduled-tick model water uses, so a chain of supports
/// collapsing falls one layer per tick instead of all at once.
const FRAGILE_BREAK_DELAY: u64 = 1;

/// Break behaviour for fragile blocks (the cross-plants and the torch). A neighbour
/// change that takes away the block's support schedules its break for the next tick;
/// the scheduled tick re-checks (the support may have returned, or the cell may now hold
/// something else) and, only if the block is still fragile and still unsupported,
/// shatters it — dropping and bursting exactly as a player's hand-break would (see
/// [`World::note_block_destroyed`]).
pub struct Fragile;

impl crate::world::engine_behavior::EngineBlockBehavior for Fragile {
    fn neighbor_update(&self, world: &mut World, pos: IVec3) {
        // Dispatch already read this cell as the fragile block; re-read to learn which
        // one — the support cell is per-block (a torch sideways, a plant below, a
        // hanging block above).
        let block = Block::from_id(world.chunk_block(pos.x, pos.y, pos.z));
        if !world.fragile_supported(pos, block) {
            world.schedule_block_tick(pos, FRAGILE_BREAK_DELAY);
        }
    }

    fn scheduled_tick(&self, world: &mut World, pos: IVec3) {
        let block = Block::from_id(world.chunk_block(pos.x, pos.y, pos.z));
        // The cell may have changed since the break was scheduled (mined, replaced, or
        // re-supported); only break a still-fragile, still-unsupported block.
        if !block.is_fragile() || world.fragile_supported(pos, block) {
            return;
        }
        // Shatter it as a natural break — drops + burst, exactly as a hand-break.
        world.break_block_naturally(pos);
    }
}

/// The fragile singleton a row points at (`behavior: &behavior::FRAGILE`).
pub static FRAGILE: Fragile = Fragile;

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::facing::Facing;
    use petramond_world::block::{Block, SupportDir};
    use petramond_world::block_state::{StairHalf, StairState};
    use petramond_world::chunk::{Chunk, ChunkPos};
    use petramond_world::crafting::Recipes;
    use petramond_world::torch::TorchPlacement;

    /// A world with one empty loaded chunk at the origin.
    fn world() -> World {
        let mut w = World::new(0, 4);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    fn run_ticks(w: &mut World, n: u32) {
        let r = Recipes::default();
        for _ in 0..n {
            w.game_tick(&r);
        }
    }

    fn block(w: &World, p: IVec3) -> Block {
        Block::from_id(w.chunk_block(p.x, p.y, p.z))
    }

    /// A BOX SET's face is complete only where its matter reaches the
    /// boundary — the rule every "is there something to stand on / mount to"
    /// question resolves through for the families nobody can enumerate.
    ///
    /// The cactus is the case worth pinning: its side faces are carried by
    /// full-cell planes declared `occludes: false`, so they DRAW a complete
    /// face while being no matter at all, and its cap plate is `collides:
    /// false` while still being matter. Read either flag as the other and this
    /// flips — silently, into "a torch mounts on the side of a cactus".
    #[test]
    fn a_box_sets_face_is_complete_only_where_its_matter_reaches_the_boundary() {
        let face = |b: Block, dir: IVec3| {
            let mut w = world();
            let p = IVec3::new(8, 64, 8);
            w.set_block_world(p.x, p.y, p.z, b);
            petramond_world::block::full_face_at(&w, p, dir)
        };
        // A one-texel cover: its floor is against the boundary, its top is not.
        assert_eq!(
            face(Block::SnowLayer, -IVec3::Y),
            Some(petramond_world::block::FullFace::Shaped),
            "a cover rests on its own floor"
        );
        assert_eq!(
            face(Block::SnowLayer, IVec3::Y),
            None,
            "a cover's top is a texel up, not at the boundary — nothing stands on it"
        );
        // The cactus: cap plate = matter (though it does not collide), side
        // planes = face carriers (they draw, and are not matter).
        assert_eq!(
            face(Block::Cactus, IVec3::Y),
            Some(petramond_world::block::FullFace::Shaped),
            "the cap plate is matter"
        );
        assert_eq!(
            face(Block::Cactus, IVec3::X),
            None,
            "an `occludes: false` face carrier is not something to mount on"
        );
        // …and it is SHAPED, never CUBE: the material rules that bind a cube
        // face (opaque-only fence joins) must not start binding box sets.
        assert_eq!(
            face(Block::Stone, IVec3::Y),
            Some(petramond_world::block::FullFace::Cube)
        );
    }

    /// A row that DECLARED what its floor must be keeps that rule once placed,
    /// so the gate that let it be placed and the rule that keeps it there
    /// cannot disagree — a mushroom rooted on a stair top by anything other
    /// than a player click still sheds.
    ///
    /// It has to be checked BEFORE `rests_flat_on_floor`, which probes octant
    /// VOLUMES: anything with a foot on the floor reads as lying flat and would
    /// otherwise take the cover rule (any full collision cube) instead of its
    /// own declaration.
    #[test]
    fn a_declared_floor_requirement_is_also_the_survival_rule() {
        let mut w = world();
        // `petramond:brown_mushroom` declares `roots_face: "full_cube"`.
        // Both sites stay clear of the lone chunk's borders, like the snow
        // test above — the streaming-finality guard drops breaks near them.
        w.set_block_world(6, 64, 8, Block::Stone);
        w.set_block_world(6, 65, 8, Block::BrownMushroom);
        let stair = IVec3::new(10, 64, 8);
        assert!(w.place_stair(
            stair,
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom)
        ));
        w.set_block_world(10, 65, 8, Block::BrownMushroom);
        run_ticks(&mut w, 3);
        assert_eq!(
            block(&w, IVec3::new(6, 65, 8)),
            Block::BrownMushroom,
            "a full cube satisfies `full_cube`"
        );
        assert_eq!(
            block(&w, IVec3::new(10, 65, 8)),
            Block::Air,
            "a stair top does not, so the mushroom sheds like the snow layer"
        );
    }

    /// The compass mapping of a wall support. Getting this backwards puts every
    /// wall-mounted block's support cell on the far side of the wall, where it
    /// is usually air — and the block then breaks the tick after it is placed.
    #[test]
    fn a_wall_supports_row_reads_the_cell_its_side_names() {
        for (dir, facing) in [
            (SupportDir::North, Facing::North),
            (SupportDir::South, Facing::South),
            (SupportDir::West, Facing::West),
            (SupportDir::East, Facing::East),
        ] {
            assert_eq!(dir.support_cell(IVec3::ZERO), facing.dir(), "{dir:?}");
            assert!(dir.is_wall(), "{dir:?}");
        }
        assert!(!SupportDir::Below.is_wall());
        assert!(!SupportDir::Above.is_wall());
    }

    #[test]
    fn a_plant_breaks_the_tick_after_its_support_is_dug_away() {
        let mut w = world();
        let ground = IVec3::new(8, 64, 8);
        let plant = IVec3::new(8, 65, 8);
        w.set_block_world(ground.x, ground.y, ground.z, Block::Dirt);
        w.set_block_world(plant.x, plant.y, plant.z, Block::Poppy);
        run_ticks(&mut w, 2); // settle: supported, nothing happens
        assert_eq!(block(&w, plant), Block::Poppy);

        // Dig the support out: the flower is scheduled, then breaks on the next tick.
        w.set_block_world(ground.x, ground.y, ground.z, Block::Air);
        run_ticks(&mut w, 2);
        assert_eq!(
            block(&w, plant),
            Block::Air,
            "unsupported flower must break"
        );
        // ...and it was handed to the presentation layer as a hand-style break.
        let breaks = w.take_natural_breaks();
        assert!(
            breaks.iter().any(|&(p, b)| p == plant && b == Block::Poppy),
            "the broken flower was recorded for its drop + particle burst",
        );
    }

    #[test]
    fn a_cactus_breaks_the_tick_after_the_sand_under_it_is_dug() {
        // The cactus is fragile just like the dead bush: undermine it and it shatters.
        let mut w = world();
        let sand = IVec3::new(8, 64, 8);
        let cactus = IVec3::new(8, 65, 8);
        w.set_block_world(sand.x, sand.y, sand.z, Block::Sand);
        w.set_block_world(cactus.x, cactus.y, cactus.z, Block::Cactus);
        run_ticks(&mut w, 2); // settle: the sand holds it up, nothing happens
        assert_eq!(block(&w, cactus), Block::Cactus);

        // Dig the sand out: the cactus is scheduled, then breaks on the next tick.
        w.set_block_world(sand.x, sand.y, sand.z, Block::Air);
        run_ticks(&mut w, 2);
        assert_eq!(
            block(&w, cactus),
            Block::Air,
            "an undermined cactus must break"
        );
        let breaks = w.take_natural_breaks();
        assert!(
            breaks
                .iter()
                .any(|&(p, b)| p == cactus && b == Block::Cactus),
            "the broken cactus was recorded for its drop + particle burst",
        );
    }

    #[test]
    fn a_supported_plant_survives_a_change_beside_it() {
        let mut w = world();
        w.set_block_world(8, 64, 8, Block::Dirt);
        w.set_block_world(8, 65, 8, Block::Poppy);
        // A change next to the plant (its support untouched) must not break it.
        w.set_block_world(9, 65, 8, Block::Dirt);
        run_ticks(&mut w, 3);
        assert_eq!(block(&w, IVec3::new(8, 65, 8)), Block::Poppy);
        assert!(w.take_natural_breaks().is_empty());
    }

    #[test]
    fn a_wall_torch_breaks_when_the_wall_it_leans_on_is_removed() {
        let mut w = world();
        let torch = IVec3::new(8, 65, 8);
        // A West-leaning torch is mounted on the wall to its +X (see `TorchPlacement`):
        // its support is sideways, the one non-data-driven case.
        let wall = TorchPlacement::West.support_cell(torch);
        w.set_block_world(wall.x, wall.y, wall.z, Block::Stone);
        w.set_block_world(torch.x, torch.y, torch.z, Block::Torch);
        w.insert_torch(torch, TorchPlacement::West);
        run_ticks(&mut w, 2);
        assert_eq!(block(&w, torch), Block::Torch, "held up by its wall");

        // Mine the wall: the torch loses its sideways support and breaks next tick.
        w.set_block_world(wall.x, wall.y, wall.z, Block::Air);
        run_ticks(&mut w, 2);
        assert_eq!(
            block(&w, torch),
            Block::Air,
            "a wall torch falls with its wall"
        );
        let breaks = w.take_natural_breaks();
        assert!(breaks.iter().any(|&(p, b)| p == torch && b == Block::Torch));
    }

    #[test]
    fn a_ladder_breaks_the_tick_after_its_wall_is_mined() {
        let mut w = world();
        let ladder = IVec3::new(8, 65, 8);
        // An east-facing ladder (its own block row) hangs on the wall to its west.
        let wall = petramond_world::ladder::support_cell(ladder, Facing::East);
        w.set_block_world(wall.x, wall.y, wall.z, Block::Stone);
        w.set_block_world(ladder.x, ladder.y, ladder.z, Block::LadderEast);
        run_ticks(&mut w, 2);
        assert_eq!(block(&w, ladder), Block::LadderEast, "held up by its wall");

        // Mine the wall: the ladder loses its support and breaks on the next tick
        // (the same announce → scheduled-break cadence as the wall torch above).
        w.set_block_world(wall.x, wall.y, wall.z, Block::Air);
        run_ticks(&mut w, 2);
        assert_eq!(
            block(&w, ladder),
            Block::Air,
            "a ladder falls with its wall on the following tick"
        );
        let breaks = w.take_natural_breaks();
        assert!(
            breaks
                .iter()
                .any(|&(p, b)| p == ladder && b == Block::LadderEast),
            "the broken ladder was recorded for its drop + particle burst",
        );
    }

    #[test]
    fn a_snow_layer_rests_on_any_full_cube_but_sheds_off_partial_shapes() {
        // Both sites stay >= SIM_READ_REACH cells from the lone chunk's
        // borders, or the streaming-finality guard drops the scheduled break.
        let mut w = world();
        // Leaves are a full collision cube without being opaque: canopy snow
        // must persist (the weather mod lays it there; it used to shatter on
        // the placement's own block update).
        w.set_block_world(7, 64, 8, Block::OakLeaves);
        w.set_block_world(7, 65, 8, Block::SnowLayer);
        run_ticks(&mut w, 3);
        assert_eq!(
            block(&w, IVec3::new(7, 65, 8)),
            Block::SnowLayer,
            "canopy snow must persist on leaves"
        );

        // A stair is not a full cube: the layer sheds on the next tick.
        let stair = IVec3::new(9, 64, 8);
        assert!(w.place_stair(
            stair,
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom)
        ));
        w.set_block_world(9, 65, 8, Block::SnowLayer);
        run_ticks(&mut w, 3);
        assert_eq!(
            block(&w, IVec3::new(9, 65, 8)),
            Block::Air,
            "stair-top snow must shed"
        );
    }

    #[test]
    fn a_wall_torch_on_a_stair_flat_side_survives_support_rechecks() {
        let mut w = world();
        let stair = IVec3::new(8, 66, 8);
        assert!(w.place_stair(
            stair,
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom)
        ));
        let torch = stair - IVec3::X;
        w.set_block_world(torch.x, torch.y, torch.z, Block::Torch);
        w.insert_torch(torch, TorchPlacement::West);
        run_ticks(&mut w, 2);
        assert_eq!(block(&w, torch), Block::Torch, "stair back holds torch");
    }

    /// A block row declaring `support: "above"` hangs from its ceiling, and a
    /// run of them unzips DOWNWARD from a cut at the top while a cut at the
    /// bottom takes nothing with it.
    ///
    /// No engine row hangs, so this needs a pack row: the block registry is a
    /// process-wide `LazyLock`, so the fixture must be in place before ANY
    /// test in this binary touches it — hence the established re-spawn
    /// pattern (this test writes a content-only pack and runs the `#[ignore]`d
    /// inner test below in a child process with `PETRAMOND_MODS` set).
    /// Deterministic regardless of test order.
    #[test]
    fn a_hanging_row_breaks_downward_and_never_upward() {
        let root = std::env::temp_dir().join(format!("petramond-hangpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack = root.join("mods/hangtest");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(
            pack.join("pack.json"),
            r#"{ "name": "Hang Test", "id": "hangtest", "description": "support-direction fixture" }"#,
        )
        .unwrap();
        // Two DIFFERENT hanging rows (a curtain mixes them) plus one ordinary
        // standing row, so the direction is proven to be per-row data.
        let row = |name: &str, support: &str| {
            format!(
                r#"{{ "block": "hangtest:{name}", "shape": "cross", "flags": ["transparent"], "tags": ["fragile"], "behavior": "fragile", "interaction": "none", "collision": [], "emission": 0{support}, "tiles": ["poppy", "poppy", "poppy"], "material": "plant", "hardness": 0, "drops": [] }}"#
            )
        };
        let above = r#", "support": "above""#;
        std::fs::write(
            pack.join("blocks.json"),
            format!(
                r#"{{ "blocks": [ {}, {}, {} ] }}"#,
                row("vine", above),
                row("vine_lit", above),
                row("standing", "")
            ),
        )
        .unwrap();

        let exe = std::env::current_exe().expect("test binary path");
        let out = std::process::Command::new(exe)
            .arg("world::fragile::tests::hanging_support_inner")
            .arg("--exact")
            .arg("--ignored")
            .arg("--nocapture")
            .env("PETRAMOND_MODS", root.join("mods"))
            .output()
            .expect("spawn test binary");
        let _ = std::fs::remove_dir_all(&root);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "inner test failed\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stderr),
        );
        // A filtered-out inner test also exits 0 — the one way this whole
        // check can silently become a no-op (a rename, a moved module).
        assert!(
            stdout.contains("1 passed"),
            "the inner test did not run\n{stdout}"
        );
    }

    /// Runs ONLY in the child process spawned above (needs `PETRAMOND_MODS`
    /// pointing at the fixture pack before first registry touch).
    #[test]
    #[ignore = "spawned by a_hanging_row_breaks_downward_and_never_upward with a fixture pack env"]
    fn hanging_support_inner() {
        let by_name = |name: &str| {
            Block(
                petramond_world::registry::names()
                    .blocks
                    .id(name)
                    .unwrap_or_else(|| panic!("fixture pack row '{name}' must be registered")),
            )
        };
        let vine = by_name("hangtest:vine");
        let vine_lit = by_name("hangtest:vine_lit");
        let standing = by_name("hangtest:standing");

        let mut w = world();
        // LEAVES, not stone: a full collision cube that is NOT opaque. A pack's
        // solid-but-translucent ceiling (a glowing mushroom cap) is the common
        // real anchor, and an `is_opaque` accept rule would drop every curtain
        // hanging under one while still passing under rock.
        let ceiling = IVec3::new(8, 70, 8);
        w.set_block_world(ceiling.x, ceiling.y, ceiling.z, Block::OakLeaves);
        // A five-cell curtain of MIXED hanging rows: chaining is a property of
        // the declaration, not of block identity.
        let curtain: Vec<IVec3> = (65..=69).rev().map(|y| IVec3::new(8, y, 8)).collect();
        for (i, c) in curtain.iter().enumerate() {
            let b = if i % 2 == 0 { vine } else { vine_lit };
            w.set_block_world(c.x, c.y, c.z, b);
        }
        run_ticks(&mut w, 3);
        for c in &curtain {
            assert_ne!(block(&w, *c), Block::Air, "hung curtain must stand: {c:?}");
        }

        // An edit BESIDE the curtain announces to every cell of it; under the
        // old ground rule that shattered the whole run.
        w.set_block_world(9, 67, 8, Block::Stone);
        run_ticks(&mut w, 3);
        for c in &curtain {
            assert_ne!(
                block(&w, *c),
                Block::Air,
                "a neighbour edit must not shatter the curtain: {c:?}"
            );
        }

        // Cut the BOTTOM: nothing above it lost its own support.
        let bottom = curtain[4];
        w.set_block_world(bottom.x, bottom.y, bottom.z, Block::Air);
        run_ticks(&mut w, 6);
        for c in &curtain[..4] {
            assert_ne!(
                block(&w, *c),
                Block::Air,
                "a cut at the bottom must not propagate upward: {c:?}"
            );
        }

        // Cut the TOP: the whole remaining run unzips downward, one cell per
        // tick, and each cell is handed over as a natural break (drops + burst).
        let _ = w.take_natural_breaks();
        let top = curtain[0];
        w.set_block_world(top.x, top.y, top.z, Block::Air);
        run_ticks(&mut w, 10);
        for c in &curtain[1..4] {
            assert_eq!(
                block(&w, *c),
                Block::Air,
                "a cut at the top must cascade all the way down: {c:?}"
            );
        }
        let breaks = w.take_natural_breaks();
        assert_eq!(
            breaks.len(),
            3,
            "every cascaded cell breaks naturally, exactly once: {breaks:?}"
        );

        // PLACEMENT must accept exactly what the rule above keeps. A hanging
        // row has no substrate vocabulary — `roots_on` names GROUNDS and its
        // support is a ceiling — so without a gate it places on open air and
        // this very tick shatters it, eating the item.
        let mut never_occupied = |_: IVec3, _: &[petramond_world::block::Aabb]| false;
        let mut plan = |w: &World, p: IVec3, b: Block| {
            w.finish_single_cell_placement(
                b,
                p,
                petramond_world::block::ShapeState::NONE,
                &[],
                &mut never_occupied,
            )
            .is_some()
        };
        assert!(
            !plan(&w, IVec3::new(4, 68, 4), vine),
            "a hanging row must not place under open air"
        );
        w.set_block_world(4, 70, 4, Block::Stone);
        assert!(plan(&w, IVec3::new(4, 69, 4), vine), "a ceiling accepts it");
        w.set_block_world(4, 69, 4, vine);
        assert!(
            plan(&w, IVec3::new(4, 68, 4), vine_lit),
            "and so does another hanging row, so a curtain extends downward"
        );

        // The direction is PER ROW: the same fixture's default row is still
        // held from below and is not held by a ceiling.
        w.set_block_world(7, 64, 7, Block::Dirt);
        w.set_block_world(7, 65, 7, standing);
        w.set_block_world(9, 66, 9, Block::Stone);
        w.set_block_world(9, 65, 9, standing);
        run_ticks(&mut w, 4);
        assert_eq!(block(&w, IVec3::new(7, 65, 7)), standing, "ground holds it");
        assert_eq!(
            block(&w, IVec3::new(9, 65, 9)),
            Block::Air,
            "a ceiling holds up nothing that does not declare it"
        );
    }
}
