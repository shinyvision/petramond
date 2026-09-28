use mod_sdk::biome as b;
use mod_sdk::build::{Axis, Dir, Draw, Families, Form, Half, Material, Name, Noise2};
use mod_sdk::FxHashMap;
use mod_sdk::GenRng;

pub(super) const WOOD_FORMS: [Form; 5] = [
    Form::Block,
    Form::Log,
    Form::Slab,
    Form::Stairs,
    Form::Fence,
];
pub(super) const STONES: [&str; 3] = ["cobblestone", "stone", "stone_bricks"];
const STONE_FORMS: [Form; 3] = [Form::Block, Form::Slab, Form::Stairs];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Surface {
    Grass,
    Podzol,
    Sand,
}

/// How a biome dresses its camps: which woods it grows, how often its walls are stone, and the
/// ground they stand on.
pub(super) struct BiomeStyle {
    pub biomes: &'static [u8],
    pub woods: &'static [(&'static str, f32)],
    pub stone_bias: f32,
    pub surface: Surface,
    pub snow: bool,
    pub mud: bool,
    pub rocky: bool,
    pub tufts: f32,
    pub dead_bush: f32,
}

const fn style(
    biomes: &'static [u8],
    woods: &'static [(&'static str, f32)],
    stone_bias: f32,
    surface: Surface,
) -> BiomeStyle {
    BiomeStyle {
        biomes,
        woods,
        stone_bias,
        surface,
        snow: false,
        mud: false,
        rocky: false,
        tufts: 0.1,
        dead_bush: 0.006,
    }
}

/// Deserts have no trees, so their camps are mostly stone and any wood is carried in.
static STYLES: [BiomeStyle; 10] = [
    BiomeStyle {
        tufts: 0.14,
        ..style(
            &[b::PLAINS, b::MEADOW],
            &[("oak", 8.0), ("birch", 2.0)],
            0.45,
            Surface::Grass,
        )
    },
    style(
        &[b::FOREST, b::WOODED_HILLS],
        &[("oak", 6.0), ("birch", 4.0)],
        0.3,
        Surface::Grass,
    ),
    style(
        &[b::TAIGA, b::GROVE],
        &[("spruce", 1.0)],
        0.4,
        Surface::Grass,
    ),
    BiomeStyle {
        tufts: 0.08,
        ..style(
            &[b::OLD_GROWTH_TAIGA],
            &[("spruce", 1.0)],
            0.35,
            Surface::Podzol,
        )
    },
    BiomeStyle {
        snow: true,
        tufts: 0.03,
        ..style(
            &[
                b::SNOWY_TAIGA,
                b::SNOWY_TUNDRA,
                b::SNOWY_PLAINS,
                b::SNOWY_SLOPES,
            ],
            &[("spruce", 1.0)],
            0.45,
            Surface::Grass,
        )
    },
    BiomeStyle {
        tufts: 0.18,
        ..style(&[b::SAVANNA], &[("acacia", 1.0)], 0.4, Surface::Grass)
    },
    BiomeStyle {
        tufts: 0.08,
        ..style(
            &[b::REDWOOD_FOREST],
            &[("redwood", 8.0), ("spruce", 2.0)],
            0.3,
            Surface::Podzol,
        )
    },
    BiomeStyle {
        mud: true,
        tufts: 0.12,
        ..style(
            &[b::SWAMP, b::WETLAND],
            &[("oak", 1.0)],
            0.3,
            Surface::Grass,
        )
    },
    BiomeStyle {
        tufts: 0.0,
        dead_bush: 0.02,
        ..style(
            &[b::DESERT, b::DESERT_LAKES],
            &[("acacia", 6.0), ("oak", 4.0)],
            0.85,
            Surface::Sand,
        )
    },
    BiomeStyle {
        rocky: true,
        tufts: 0.08,
        ..style(
            &[
                b::WINDSWEPT_HILLS,
                b::MOUNTAINS,
                b::FOOTHILLS,
                b::MOUNTAIN_EDGE,
            ],
            &[("oak", 6.0), ("spruce", 4.0)],
            0.7,
            Surface::Grass,
        )
    },
];

#[cfg(test)]
pub(super) const STYLE_BIOMES: [u8; 10] = [
    b::PLAINS,
    b::FOREST,
    b::TAIGA,
    b::OLD_GROWTH_TAIGA,
    b::SNOWY_TAIGA,
    b::SAVANNA,
    b::REDWOOD_FOREST,
    b::SWAMP,
    b::DESERT,
    b::WINDSWEPT_HILLS,
];

/// The camp style for a surface biome; `None` where camps never stand (water, beaches, peaks).
pub(super) fn for_biome(biome: u8) -> Option<&'static BiomeStyle> {
    STYLES.iter().find(|s| s.biomes.contains(&biome))
}

/// Why camps cannot be built with the loaded packs, if they cannot.
pub(super) fn check(families: &Families) -> Result<(), String> {
    for stone in STONES {
        if STONE_FORMS
            .iter()
            .any(|&f| !families.has(Name::new(stone), f))
        {
            return Err(format!(
                "material family '{stone}' lacks a block, slab or stairs"
            ));
        }
    }
    if families.with_forms(&WOOD_FORMS).is_empty() {
        return Err("no material family has planks, log, slab, stairs and fence".into());
    }
    Ok(())
}

/// The camp's palette: what its walls are made of and how blocks are picked from it.
pub(super) struct Mats<'a> {
    fam: &'a Families,
    pub style: &'static BiomeStyle,
    pub stone: bool,
    pub wood: Name,
    others: Vec<Name>,
    stones: [Name; 3],
    stone_noise: Noise2,
}

impl<'a> Mats<'a> {
    pub(super) fn new(fam: &'a Families, style: &'static BiomeStyle, rng: &mut GenRng) -> Mats<'a> {
        let stone = rng.roll(style.stone_bias);
        let woods = fam.with_forms(&WOOD_FORMS);
        let wanted = Name::new(rng.weighted(style.woods));
        let wood = if woods.contains(&wanted) {
            wanted
        } else {
            woods[0]
        };
        let others = woods.into_iter().filter(|w| *w != wood).collect();
        Mats {
            fam,
            style,
            stone,
            wood,
            others,
            stones: STONES.map(Name::new),
            stone_noise: Noise2(rng.next_u64() as u32),
        }
    }

    pub(super) fn families(&self) -> &'a Families {
        self.fam
    }

    /// The family's member in `form`, shaped by `state` only when that member exists: the plain
    /// block a missing form falls back to must carry no state, or the host rejects the write.
    fn form(&self, family: Name, form: Form, state: impl FnOnce(Material) -> Material) -> Material {
        match self.fam.get(family, form) {
            Some(m) => state(m),
            None => self
                .fam
                .get(family, Form::Block)
                .unwrap_or_else(|| named!("cobblestone")),
        }
    }

    pub(super) fn block(&self, family: impl Into<Name>) -> Material {
        self.form(family.into(), Form::Block, |m| m)
    }

    pub(super) fn slab(&self, family: impl Into<Name>, half: Half) -> Material {
        self.form(family.into(), Form::Slab, |m| m.half(half))
    }

    pub(super) fn stairs(&self, family: impl Into<Name>, facing: Dir, half: Half) -> Material {
        self.form(family.into(), Form::Stairs, |m| m.facing(facing).half(half))
    }

    pub(super) fn log(&self, wood: impl Into<Name>, axis: Axis) -> Material {
        self.form(wood.into(), Form::Log, |m| m.axis(axis))
    }

    pub(super) fn fence(&self, wood: impl Into<Name>) -> Material {
        self.form(wood.into(), Form::Fence, |m| m)
    }

    pub(super) fn door(&self, wood: impl Into<Name>) -> Option<Material> {
        self.fam.get(wood.into(), Form::Door)
    }

    pub(super) fn cobblestone(&self) -> Name {
        self.stones[0]
    }

    /// The stone family at a wall cell: patches of bricks (old repairs) and bare stone in a
    /// mostly-cobble wall, coherent in space so they read as patches, not noise.
    pub(super) fn stone_at(&self, rng: &mut GenRng, [x, y, z]: [i32; 3]) -> Name {
        let [cobble, stone, bricks] = self.stones;
        // A plane of noise sheared with height: patches coherent in all three axes at a 2D
        // sample's cost, drifting a little sideways as they go up.
        let n = self.stone_noise.at(
            x as f32 * 0.21 + y as f32 * 0.19,
            z as f32 * 0.21 - y as f32 * 0.23,
        );
        if n > 0.3 {
            if rng.roll(0.85) {
                bricks
            } else {
                cobble
            }
        } else if n < -0.36 {
            if rng.roll(0.8) {
                stone
            } else {
                cobble
            }
        } else if rng.roll(0.82) {
            cobble
        } else {
            *rng.pick(&[stone, bricks])
        }
    }

    /// The primary wood, or rarely a scavenged other one.
    pub(super) fn wood_at(&self, rng: &mut GenRng, other: f32) -> Name {
        if !self.others.is_empty() && rng.roll(other) {
            *rng.pick(&self.others)
        } else {
            self.wood
        }
    }

    pub(super) fn other_wood(&self, rng: &mut GenRng) -> Name {
        if self.others.is_empty() {
            self.wood
        } else {
            *rng.pick(&self.others)
        }
    }
}

thread_local! {
    static NAMED: std::cell::RefCell<FxHashMap<(usize, usize, Form), Material>> =
        Default::default();
}

/// `petramond:<block>` as a material of `form`, built once per literal: builders call this in
/// their innermost loops.
fn cached(block: &'static str, form: Form) -> Material {
    NAMED.with(|named| {
        *named
            .borrow_mut()
            .entry((block.as_ptr() as usize, block.len(), form))
            .or_insert_with(|| Material::of(Name::new(&format!("petramond:{block}")), form))
    })
}

pub(super) fn named(block: &'static str) -> Material {
    cached(block, Form::Other)
}

pub(super) fn decor(block: &'static str) -> Material {
    cached(block, Form::Decor)
}
