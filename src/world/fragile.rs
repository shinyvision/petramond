use crate::world::ServerWorld;
use petramond_math::math::IVec3;
use petramond_world::block::Block;

pub struct Fragile;

impl crate::world::engine_behavior::EngineBlockBehavior for Fragile {
    fn neighbor_update(&self, world: &mut ServerWorld, pos: IVec3) {
        let block = Block::from_id(world.data.chunk_block(pos.x, pos.y, pos.z));
        if !block.is_fragile() || world.data.fragile_supported(pos, block) {
            return;
        }
        world.break_block_naturally(pos);
    }
}

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

    fn world() -> ServerWorld {
        let mut w = ServerWorld::new(0, 4);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    fn run_ticks(w: &mut ServerWorld, n: u32) {
        let r = Recipes::default();
        for _ in 0..n {
            w.game_tick(&r);
        }
    }

    fn block(w: &ServerWorld, p: IVec3) -> Block {
        Block::from_id(w.data.chunk_block(p.x, p.y, p.z))
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
            petramond_world::block::full_face_at(w.data(), p, dir)
        };
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
        assert_eq!(
            face(Block::Stone, IVec3::Y),
            Some(petramond_world::block::FullFace::Cube)
        );
    }

    /// A row that declared its floor requirement keeps that rule once placed, so the gate that
    /// allowed placement and the rule that keeps it standing can't disagree. A mushroom rooted on
    /// a stair top by anything other than a player click still sheds.
    ///
    /// Must run before `rests_flat_on_floor`. That check probes octant volumes, so anything with a
    /// foot on the floor looks flat and would fall through to the generic cover rule instead of
    /// using its own declaration.
    #[test]
    fn a_declared_floor_requirement_is_also_the_survival_rule() {
        let mut w = world();
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
    fn a_plant_breaks_with_the_update_that_undermines_it() {
        let mut w = world();
        let ground = IVec3::new(8, 64, 8);
        let plant = IVec3::new(8, 65, 8);
        w.set_block_world(ground.x, ground.y, ground.z, Block::Dirt);
        w.set_block_world(plant.x, plant.y, plant.z, Block::Poppy);
        run_ticks(&mut w, 2);
        assert_eq!(block(&w, plant), Block::Poppy);

        w.set_block_world(ground.x, ground.y, ground.z, Block::Air);
        run_ticks(&mut w, 1);
        assert_eq!(
            block(&w, plant),
            Block::Air,
            "unsupported flower must break at the undermining update"
        );
        let breaks = w.take_natural_breaks();
        assert!(
            breaks.iter().any(|&(p, b)| p == plant && b == Block::Poppy),
            "the broken flower was recorded for its drop + particle burst",
        );
    }

    #[test]
    fn a_cactus_breaks_with_the_update_that_undermines_it() {
        let mut w = world();
        let sand = IVec3::new(8, 64, 8);
        let cactus = IVec3::new(8, 65, 8);
        w.set_block_world(sand.x, sand.y, sand.z, Block::Sand);
        w.set_block_world(cactus.x, cactus.y, cactus.z, Block::Cactus);
        run_ticks(&mut w, 2);
        assert_eq!(block(&w, cactus), Block::Cactus);

        w.set_block_world(sand.x, sand.y, sand.z, Block::Air);
        run_ticks(&mut w, 1);
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
        w.set_block_world(9, 65, 8, Block::Dirt);
        run_ticks(&mut w, 3);
        assert_eq!(block(&w, IVec3::new(8, 65, 8)), Block::Poppy);
        assert!(w.take_natural_breaks().is_empty());
    }

    #[test]
    fn a_wall_torch_breaks_when_the_wall_it_leans_on_is_removed() {
        let mut w = world();
        let torch = IVec3::new(8, 65, 8);
        let wall = TorchPlacement::West.support_cell(torch);
        w.set_block_world(wall.x, wall.y, wall.z, Block::Stone);
        w.set_block_world(torch.x, torch.y, torch.z, Block::Torch);
        w.data.insert_torch(torch, TorchPlacement::West);
        run_ticks(&mut w, 2);
        assert_eq!(block(&w, torch), Block::Torch, "held up by its wall");

        w.set_block_world(wall.x, wall.y, wall.z, Block::Air);
        run_ticks(&mut w, 1);
        assert_eq!(
            block(&w, torch),
            Block::Air,
            "a wall torch falls with its wall"
        );
        let breaks = w.take_natural_breaks();
        assert!(breaks.iter().any(|&(p, b)| p == torch && b == Block::Torch));
    }

    #[test]
    fn a_ladder_breaks_with_the_update_that_mines_its_wall() {
        let mut w = world();
        let ladder = IVec3::new(8, 65, 8);
        let wall = petramond_world::ladder::support_cell(ladder, Facing::East);
        w.set_block_world(wall.x, wall.y, wall.z, Block::Stone);
        w.set_block_world(ladder.x, ladder.y, ladder.z, Block::LadderEast);
        run_ticks(&mut w, 2);
        assert_eq!(block(&w, ladder), Block::LadderEast, "held up by its wall");

        w.set_block_world(wall.x, wall.y, wall.z, Block::Air);
        run_ticks(&mut w, 1);
        assert_eq!(
            block(&w, ladder),
            Block::Air,
            "a ladder falls with its wall"
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
        let mut w = world();
        w.set_block_world(7, 64, 8, Block::OakLeaves);
        w.set_block_world(7, 65, 8, Block::SnowLayer);
        run_ticks(&mut w, 3);
        assert_eq!(
            block(&w, IVec3::new(7, 65, 8)),
            Block::SnowLayer,
            "canopy snow must persist on leaves"
        );

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
    fn a_wall_torch_on_a_stair_flat_side_survives_its_support_checks() {
        let mut w = world();
        let stair = IVec3::new(8, 66, 8);
        assert!(w.place_stair(
            stair,
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom)
        ));
        let torch = stair - IVec3::X;
        w.set_block_world(torch.x, torch.y, torch.z, Block::Torch);
        w.data.insert_torch(torch, TorchPlacement::West);
        run_ticks(&mut w, 2);
        assert_eq!(block(&w, torch), Block::Torch, "stair back holds torch");
    }

    /// A row with `support: "above"` hangs from its ceiling. A run of them unzips downward from a
    /// cut at the top; a cut at the bottom takes nothing with it.
    ///
    /// No engine block hangs, so this needs a pack fixture. The block registry is a process-wide
    /// `LazyLock` and must be seeded before any test touches it, hence the re-spawn pattern: write
    /// a content-only pack, then run the `#[ignore]`d inner test in a child process with
    /// `PETRAMOND_MODS` set.
    #[test]
    fn a_hanging_row_breaks_downward_and_never_upward() {
        let root = petramond_util::test_dirs::TestScratchDir::new("hangpack");
        let pack = root.join("mods/hangtest");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(
            pack.join("pack.json"),
            r#"{ "name": "Hang Test", "id": "hangtest", "description": "support-direction fixture" }"#,
        )
        .unwrap();
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

        let run = petramond_world::test_child::run_ignored(
            "world::fragile::tests::hanging_support_inner",
            [("PETRAMOND_MODS", root.join("mods"))],
        );
        run.assert_passed();
    }

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
        let ceiling = IVec3::new(8, 70, 8);
        w.set_block_world(ceiling.x, ceiling.y, ceiling.z, Block::OakLeaves);
        let curtain: Vec<IVec3> = (65..=69).rev().map(|y| IVec3::new(8, y, 8)).collect();
        for (i, c) in curtain.iter().enumerate() {
            let b = if i % 2 == 0 { vine } else { vine_lit };
            w.set_block_world(c.x, c.y, c.z, b);
        }
        run_ticks(&mut w, 3);
        for c in &curtain {
            assert_ne!(block(&w, *c), Block::Air, "hung curtain must stand: {c:?}");
        }

        w.set_block_world(9, 67, 8, Block::Stone);
        run_ticks(&mut w, 3);
        for c in &curtain {
            assert_ne!(
                block(&w, *c),
                Block::Air,
                "a neighbour edit must not shatter the curtain: {c:?}"
            );
        }

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

        let mut never_occupied = |_: IVec3, _: &[petramond_world::block::Aabb]| false;
        let mut plan = |w: &ServerWorld, p: IVec3, b: Block| {
            w.data
                .finish_single_cell_placement(
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
