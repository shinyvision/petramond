use super::*;
use crate::rng::FeatureRng;
use crate::surface::rule::SurfaceCtx;
use petramond_world::chunk::SEA_LEVEL;

/// The generation rules as they were compiled into worldgen before they
/// moved to the biome rows. The shipped rows must reproduce them exactly —
/// same surface material for every context, same ground-cover picks from the
/// same random draws, same snow and flags — so moving the definition to data
/// changes no generated block.
mod legacy {
    use crate::rng::FeatureRng;
    use crate::surface::rule::{SurfaceCond, SurfaceRule};
    use petramond_world::block::Block;

    const REDWOOD_GRASS_SALT: u64 = 0x0000_5245_4457_0047;
    pub(super) const FERN_PATCH_SALT: u64 = 0x0000_FE12_4E12_0001;
    pub(super) const MOUNTAIN_SNOW_LINE: i32 = 143;
    pub(super) const COLD_HEMP: f32 = 0.01;

    const fn depth(n: u32, then: &'static SurfaceRule) -> SurfaceRule {
        SurfaceRule::Condition {
            when: SurfaceCond::DepthFromTop(n),
            then,
        }
    }

    const GRASS: SurfaceRule = SurfaceRule::Block(Block::Grass);
    const DIRT: SurfaceRule = SurfaceRule::Block(Block::Dirt);
    const STONE: SurfaceRule = SurfaceRule::Block(Block::Stone);
    const SAND: SurfaceRule = SurfaceRule::Block(Block::Sand);

    pub(super) static PLAINS_TOP: SurfaceRule =
        SurfaceRule::Sequence(&[depth(0, &GRASS), depth(3, &DIRT), STONE]);
    pub(super) static FOOTHILLS_TOP: SurfaceRule =
        SurfaceRule::Sequence(&[depth(0, &GRASS), depth(2, &DIRT), STONE]);
    static SNOW_CAP: SurfaceRule = SurfaceRule::Sequence(&[depth(0, &GRASS), STONE]);
    pub(super) static MOUNTAIN_TOP: SurfaceRule = SurfaceRule::Sequence(&[
        SurfaceRule::Condition {
            when: SurfaceCond::SurfaceAboveY(MOUNTAIN_SNOW_LINE),
            then: &SNOW_CAP,
        },
        SurfaceRule::Condition {
            when: SurfaceCond::SurfaceAboveY(135),
            then: &STONE,
        },
        depth(0, &GRASS),
        depth(2, &DIRT),
        STONE,
    ]);
    pub(super) static SAND_DEEP: SurfaceRule = SurfaceRule::Sequence(&[depth(4, &SAND), STONE]);
    pub(super) static OCEAN_FLOOR: SurfaceRule =
        SurfaceRule::Sequence(&[depth(2, &SAND), depth(4, &DIRT), STONE]);
    pub(super) static DEEP_OCEAN_FLOOR: SurfaceRule =
        SurfaceRule::Sequence(&[depth(2, &DIRT), STONE]);
    static UNDERWATER_DIRT_BAND: SurfaceRule = depth(3, &DIRT);
    static UNDERWATER_SAND_BAND: SurfaceRule = depth(2, &SAND);
    pub(super) static PODZOL_TOP: SurfaceRule = SurfaceRule::Sequence(&[
        SurfaceRule::Condition {
            when: SurfaceCond::Underwater,
            then: &UNDERWATER_DIRT_BAND,
        },
        depth(0, &SurfaceRule::Block(Block::Podzol)),
        depth(3, &DIRT),
        STONE,
    ]);
    static STONY_CALCITE_CAP: SurfaceRule =
        SurfaceRule::Sequence(&[depth(0, &SurfaceRule::Block(Block::Calcite)), STONE]);
    pub(super) static STONY_TOP: SurfaceRule = SurfaceRule::Sequence(&[
        SurfaceRule::Condition {
            when: SurfaceCond::SurfaceAboveY(150),
            then: &STONY_CALCITE_CAP,
        },
        STONE,
    ]);
    pub(super) static WETLAND_TOP: SurfaceRule = SurfaceRule::Sequence(&[
        SurfaceRule::Condition {
            when: SurfaceCond::Underwater,
            then: &UNDERWATER_SAND_BAND,
        },
        depth(0, &GRASS),
        depth(3, &DIRT),
        STONE,
    ]);
    static REDWOOD_CAP: SurfaceRule = SurfaceRule::Sequence(&[
        SurfaceRule::Condition {
            when: SurfaceCond::ClusterNoiseBelow {
                salt: REDWOOD_GRASS_SALT,
                threshold: 0.30,
                period: 7.0,
            },
            then: &GRASS,
        },
        SurfaceRule::Block(Block::Podzol),
    ]);
    pub(super) static REDWOOD_TOP: SurfaceRule = SurfaceRule::Sequence(&[
        SurfaceRule::Condition {
            when: SurfaceCond::Underwater,
            then: &UNDERWATER_DIRT_BAND,
        },
        depth(0, &REDWOOD_CAP),
        depth(3, &DIRT),
        STONE,
    ]);

    pub(super) fn sand_cover(rng: &mut FeatureRng) -> Option<Block> {
        if !rng.chance(0.007) {
            return None;
        }
        Some(if rng.next_i32(0, 99) < 45 {
            Block::DeadBush
        } else {
            Block::Cactus
        })
    }

    pub(super) fn old_growth_podzol(rng: &mut FeatureRng) -> Option<Block> {
        if !rng.chance(0.10) {
            return None;
        }
        let r = rng.next_i32(0, 99);
        Some(if r < 58 {
            Block::Fern
        } else if r < 80 {
            Block::ShortGrass
        } else if r < 90 {
            Block::RedMushroom
        } else {
            Block::BrownMushroom
        })
    }

    pub(super) fn redwood_cover(rng: &mut FeatureRng) -> Option<Block> {
        if !rng.chance(0.55) {
            return None;
        }
        Some(match rng.next_i32(0, 99) {
            0..=69 => Block::Fern,
            70..=89 => Block::ShortGrass,
            90..=94 => Block::RedMushroom,
            _ => Block::BrownMushroom,
        })
    }
}

type Picker = fn(&mut FeatureRng) -> Option<Block>;

/// One biome's pre-migration rules.
struct Legacy {
    surface: &'static SurfaceRule,
    tuft: Block,
    grass_density: f32,
    flowers: &'static [Block],
    flower_coverage: f32,
    flower_density: f32,
    hemp: f32,
    sand: Option<Picker>,
    podzol: Option<Picker>,
    grass_cover: Option<Picker>,
    cluster: Option<CoverCluster>,
    snow: SnowCover,
    /// `(ocean, beach_base, wet, mountain)`
    flags: (bool, bool, bool, bool),
}

const PLAINS_FLOWERS: &[Block] = &[
    Block::Dandelion,
    Block::Poppy,
    Block::OxeyeDaisy,
    Block::Cornflower,
    Block::AzureBluet,
];

fn legacy(biome: Biome) -> Legacy {
    use legacy::*;
    let base = |surface| Legacy {
        surface,
        tuft: Block::ShortGrass,
        grass_density: 0.0,
        flowers: &[],
        flower_coverage: 0.0,
        flower_density: 0.0,
        hemp: 0.0,
        sand: None,
        podzol: None,
        grass_cover: None,
        cluster: None,
        snow: SnowCover::None,
        flags: (false, true, false, false),
    };
    let grass = |surface, tuft, density| Legacy {
        tuft,
        grass_density: density,
        ..base(surface)
    };
    match biome {
        Biome::OCEAN | Biome::DEEP_OCEAN => Legacy {
            flags: (true, false, true, false),
            ..base(if biome == Biome::OCEAN {
                &OCEAN_FLOOR
            } else {
                &DEEP_OCEAN_FLOOR
            })
        },
        Biome::BEACH => Legacy {
            flags: (false, false, true, false),
            ..base(&SAND_DEEP)
        },
        Biome::RIVER => Legacy {
            flags: (false, true, true, false),
            ..base(&OCEAN_FLOOR)
        },
        Biome::DESERT | Biome::DESERT_LAKES => Legacy {
            sand: Some(sand_cover),
            ..base(&SAND_DEEP)
        },
        Biome::PLAINS => Legacy {
            flowers: PLAINS_FLOWERS,
            flower_coverage: 0.1,
            flower_density: 0.15,
            hemp: 0.0045,
            ..grass(&PLAINS_TOP, Block::ShortGrass, 0.14)
        },
        Biome::SAVANNA => Legacy {
            hemp: 0.0023,
            ..grass(&PLAINS_TOP, Block::ShortGrass, 0.14)
        },
        Biome::FOREST => Legacy {
            flowers: &[Block::Poppy, Block::Dandelion, Block::OxeyeDaisy],
            flower_coverage: 0.16,
            flower_density: 0.22,
            hemp: 0.0037,
            ..grass(&PLAINS_TOP, Block::ShortGrass, 0.11)
        },
        Biome::SWAMP | Biome::WETLAND => Legacy {
            hemp: 0.016,
            ..grass(&WETLAND_TOP, Block::ShortGrass, 0.10)
        },
        Biome::TAIGA => Legacy {
            hemp: 0.0023,
            ..grass(&PLAINS_TOP, Block::Fern, 0.12)
        },
        Biome::SNOWY_TUNDRA | Biome::SNOWY_PLAINS => Legacy {
            hemp: COLD_HEMP,
            snow: SnowCover::Always,
            ..base(&PLAINS_TOP)
        },
        Biome::SNOWY_TAIGA => Legacy {
            hemp: COLD_HEMP,
            snow: SnowCover::Always,
            ..grass(&PLAINS_TOP, Block::Fern, 0.12)
        },
        Biome::MOUNTAINS => Legacy {
            snow: SnowCover::AboveSurfaceY(MOUNTAIN_SNOW_LINE),
            flags: (false, false, false, true),
            ..grass(&MOUNTAIN_TOP, Block::ShortGrass, 0.05)
        },
        Biome::SNOWY_PEAKS => Legacy {
            snow: SnowCover::Always,
            flags: (false, false, false, true),
            ..base(&PLAINS_TOP)
        },
        Biome::FOOTHILLS => Legacy {
            flags: (false, true, false, true),
            ..grass(&FOOTHILLS_TOP, Block::ShortGrass, 0.06)
        },
        Biome::REDWOOD_FOREST => Legacy {
            flowers: &[Block::OxeyeDaisy, Block::Poppy],
            flower_coverage: 0.05,
            flower_density: 0.14,
            podzol: Some(redwood_cover),
            grass_cover: Some(redwood_cover),
            cluster: Some(CoverCluster {
                salt: FERN_PATCH_SALT,
                period: 9.0,
                coverage: 0.5,
            }),
            ..grass(&REDWOOD_TOP, Block::ShortGrass, 0.0)
        },
        Biome::OLD_GROWTH_TAIGA => Legacy {
            podzol: Some(old_growth_podzol),
            ..grass(&PODZOL_TOP, Block::Fern, 0.12)
        },
        Biome::MEADOW => Legacy {
            flowers: PLAINS_FLOWERS,
            flower_coverage: 0.36,
            flower_density: 0.34,
            hemp: 0.0045,
            ..grass(&PLAINS_TOP, Block::ShortGrass, 0.16)
        },
        Biome::GROVE => Legacy {
            hemp: COLD_HEMP,
            snow: SnowCover::Always,
            flags: (false, true, false, true),
            ..grass(&PLAINS_TOP, Block::Fern, 0.08)
        },
        Biome::SNOWY_SLOPES => Legacy {
            hemp: COLD_HEMP,
            snow: SnowCover::Always,
            flags: (false, false, false, true),
            ..base(&PLAINS_TOP)
        },
        Biome::WINDSWEPT_HILLS | Biome::MOUNTAIN_EDGE => Legacy {
            flags: (false, false, false, true),
            ..grass(&FOOTHILLS_TOP, Block::ShortGrass, 0.05)
        },
        Biome::STONY_PEAKS => Legacy {
            flags: (false, false, false, true),
            ..base(&STONY_TOP)
        },
        Biome::WOODED_HILLS => Legacy {
            hemp: 0.0037,
            flags: (false, true, false, true),
            ..grass(&PLAINS_TOP, Block::ShortGrass, 0.09)
        },
        other => unreachable!("{other:?} is not an engine biome"),
    }
}

/// The engine biomes, the ones the compiled definitions covered.
fn all_biomes() -> impl Iterator<Item = Biome> {
    Biome::all().take(petramond_world::biome::ENGINE_BIOME_COUNT)
}

#[test]
fn every_biome_has_its_rules_in_id_order() {
    for (i, spec) in specs().iter().enumerate() {
        assert_eq!(spec.biome.id() as usize, i + 1);
    }
    assert_eq!(specs().len(), petramond_world::biome::count());
}

/// Every surface stack resolves exactly as its compiled predecessor for
/// every depth band, altitude band, the underwater split and the redwood
/// grass-cluster field.
#[test]
fn surface_rules_match_the_compiled_stacks() {
    let surf_ys = [
        SEA_LEVEL - 30,
        SEA_LEVEL - 1,
        SEA_LEVEL,
        SEA_LEVEL + 10,
        100,
        134,
        135,
        136,
        143,
        144,
        150,
        151,
        190,
    ];
    for biome in all_biomes() {
        let (data, compiled) = (spec(biome).surface, legacy(biome).surface);
        for wz in (-40..40).step_by(3) {
            for wx in (-40..40).step_by(5) {
                for &surf_y in &surf_ys {
                    for depth in 0..12u32 {
                        let ctx = SurfaceCtx {
                            seed: 0x5EED_B10E,
                            wx,
                            wz,
                            y: surf_y - depth as i32,
                            surf_y,
                            depth_from_top: depth,
                        };
                        assert_eq!(
                            data.resolve(&ctx),
                            compiled.resolve(&ctx),
                            "{biome:?} at ({wx},{wz}) surface {surf_y} depth {depth}"
                        );
                    }
                }
            }
        }
    }
}

/// A data cover roll draws exactly what the compiled picker drew — the same
/// plant from the same stream, leaving the stream in the same state.
fn assert_same_picks(biome: Biome, what: &str, data: Option<&CoverRoll>, compiled: Option<Picker>) {
    assert_eq!(data.is_some(), compiled.is_some(), "{biome:?} {what}");
    let (Some(data), Some(compiled)) = (data, compiled) else {
        return;
    };
    let mut plants = 0;
    for z in -60..60 {
        for x in -60..60 {
            let mut a = FeatureRng::positional(7, 0xC0FE, x, 0, z);
            let mut b = a;
            let picked = data.pick(&mut a);
            assert_eq!(picked, compiled(&mut b), "{biome:?} {what} at ({x},{z})");
            assert_eq!(a.next_u64(), b.next_u64(), "{biome:?} {what} draw count");
            plants += usize::from(picked.is_some());
        }
    }
    assert!(plants > 0, "{biome:?} {what} never planted");
}

#[test]
fn vegetation_snow_and_flags_match_the_compiled_profiles() {
    for biome in all_biomes() {
        let (data, compiled) = (spec(biome), legacy(biome));
        let v = &data.vegetation;
        assert_eq!(v.grass_tuft, compiled.tuft, "{biome:?} tuft");
        assert_eq!(v.grass_density, compiled.grass_density, "{biome:?} grass");
        assert_eq!(v.flower_palette, compiled.flowers, "{biome:?} flowers");
        assert_eq!(v.flower_coverage, compiled.flower_coverage, "{biome:?} coverage");
        assert_eq!(v.flower_density, compiled.flower_density, "{biome:?} flower density");
        assert_eq!(v.hemp_anchor_chance, compiled.hemp, "{biome:?} hemp");
        assert_eq!(v.cover_cluster, compiled.cluster, "{biome:?} cluster");
        assert_same_picks(biome, "sand", v.sand_cover, compiled.sand);
        assert_same_picks(biome, "podzol", v.podzol_cover, compiled.podzol);
        assert_same_picks(biome, "grass cover", v.grass_cover, compiled.grass_cover);
        assert_eq!(data.snow_cover, compiled.snow, "{biome:?} snow");
        let flags = data.flags;
        assert_eq!(
            (flags.ocean, flags.beach_base, flags.wet, flags.mountain),
            compiled.flags,
            "{biome:?} flags"
        );
    }
}

/// The vocabulary refuses what would generate wrongly instead of guessing: a
/// row without generation rules, a cover roll that can come up empty, an
/// unknown flag or field.
#[test]
fn malformed_generation_rules_are_refused() {
    let biome = Biome::PLAINS;
    assert!(parse(biome, None).is_err());
    let with = |extra: &str| {
        format!(r#"{{"surface": "petramond:stone"{extra}}}"#)
    };
    assert!(parse(biome, Some(&with(""))).is_ok());
    for bad in [
        r#", "vegetation": {"sand_cover": {"chance": 0.5, "roll": [[50, "petramond:cactus"]]}}"#,
        r#", "vegetation": {"sand_cover": {"chance": 1.5, "roll": [[100, "petramond:cactus"]]}}"#,
        r#", "vegetation": {"flowers": {"palette": [], "coverage": 0.1, "density": 0.1}}"#,
        r#", "vegetation": {"moss": 1}"#,
        r#", "flags": ["floating"]"#,
        r#", "snow": "sometimes""#,
    ] {
        assert!(parse(biome, Some(&with(bad))).is_err(), "accepted {bad}");
    }
    let spec = parse(
        biome,
        Some(&with(r#", "snow": {"above_surface_y": 90}, "flags": ["wet", "no_beach"]"#)),
    )
    .expect("valid rules load");
    assert_eq!(spec.snow_cover, SnowCover::AboveSurfaceY(90));
    assert!(spec.flags.wet && !spec.flags.beach_base && !spec.flags.ocean);
}

/// Ground the fixed slots do not name gets its cover from the row's `covers`
/// table: the mycelium mushroom roll that used to be compiled into the
/// vegetation pass is one entry, drawing the same plants from the same
/// stream.
#[test]
fn a_covers_entry_reproduces_the_compiled_mycelium_roll() {
    let spec = parse(
        Biome::PLAINS,
        Some(
            r#"{"surface": "petramond:mycelium", "vegetation": {"covers": [
                {"on": ["petramond:mycelium"], "roll": {"chance": 0.1, "roll": [
                    [55, "petramond:red_mushroom"], [100, "petramond:brown_mushroom"]]}}]}}"#,
        ),
    )
    .expect("a covers entry loads");
    let covers = spec.vegetation.covers;
    assert_eq!(covers.len(), 1);
    assert_eq!(covers[0].on, vec![Block::Mycelium]);
    assert!(!covers[0].clustered);
    let compiled = |rng: &mut FeatureRng| {
        if !rng.chance(0.10) {
            return None;
        }
        Some(if rng.next_i32(0, 99) < 55 {
            Block::RedMushroom
        } else {
            Block::BrownMushroom
        })
    };
    let mut planted = 0;
    for i in 0..4000 {
        let mut data = FeatureRng::positional(7, 0x5EED, i, 0, -i);
        let mut code = data;
        let picked = covers[0].roll.pick(&mut data);
        assert_eq!(picked, compiled(&mut code), "column {i}");
        assert_eq!(data.next_u64(), code.next_u64(), "column {i}: same draw count");
        planted += usize::from(picked.is_some());
    }
    assert!(planted > 0);

    let empty_on = r#"{"surface": "petramond:stone", "vegetation": {"covers": [
        {"on": [], "roll": {"chance": 0.1, "roll": [[100, "petramond:red_mushroom"]]}}]}}"#;
    assert!(parse(Biome::PLAINS, Some(empty_on)).is_err());
}
