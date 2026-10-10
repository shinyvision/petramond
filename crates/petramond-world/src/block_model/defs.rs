use serde::{Deserialize, Serialize};

use crate::facing::Facing;

use super::MAX_MODEL_PARTS;

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct BlockModelKind(pub u16);

#[allow(non_upper_case_globals)]
impl BlockModelKind {
    pub const FurnitureWorkbench: BlockModelKind = BlockModelKind(0);
    pub const Bucket: BlockModelKind = BlockModelKind(1);
    pub const WaterBucket: BlockModelKind = BlockModelKind(2);
    pub const Bed: BlockModelKind = BlockModelKind(3);
    pub const ChiselingStation: BlockModelKind = BlockModelKind(5);
}

const ENGINE_MODEL_KEYS: &[&str] = &[
    "petramond:furniture_workbench",
    "petramond:bucket",
    "petramond:water_bucket",
    "petramond:bed",
    "petramond:lava_bucket",
    "petramond:chiseling_station",
];

impl std::fmt::Debug for BlockModelKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match ENGINE_MODEL_KEYS.get(self.0 as usize) {
            Some(key) => write!(f, "BlockModelKind({key})"),
            None => write!(f, "BlockModelKind(#{})", self.0),
        }
    }
}

impl Serialize for BlockModelKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(def(*self).key)
    }
}

impl<'de> Deserialize<'de> for BlockModelKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let key = std::borrow::Cow::<str>::deserialize(d)?;
        defs()
            .iter()
            .position(|m| m.key == key)
            .map(|i| BlockModelKind(i as u16))
            .ok_or_else(|| serde::de::Error::custom(format!("unknown block model '{key}'")))
    }
}

pub fn all() -> &'static [BlockModelKind] {
    &DEFS.current().kinds
}

#[derive(Copy, Clone)]
pub enum CollisionSpec {
    FromModel,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlacementOrientation {
    #[default]
    LeftToRight,
    BackToFront,
    FrontToBack,
    Centered,
}

impl PlacementOrientation {
    pub fn apply(self, player_facing: Facing) -> Facing {
        match self {
            PlacementOrientation::LeftToRight => player_facing,
            PlacementOrientation::BackToFront => match player_facing {
                Facing::North => Facing::South,
                Facing::South => Facing::North,
                Facing::West => Facing::East,
                Facing::East => Facing::West,
            },
            PlacementOrientation::FrontToBack => match player_facing {
                Facing::North => Facing::West,
                Facing::West => Facing::South,
                Facing::South => Facing::East,
                Facing::East => Facing::North,
            },
            PlacementOrientation::Centered => Facing::North,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PartRole {
    #[default]
    Visible,
    Hidden,
    PassThrough,
    Hitbox,
}

impl PartRole {
    pub fn draws(self) -> bool {
        matches!(self, PartRole::Visible | PartRole::PassThrough)
    }

    pub fn collides(self) -> bool {
        self == PartRole::Visible
    }

    pub fn picks_by_bounds(self) -> bool {
        self == PartRole::Hitbox
    }
}

pub fn part_role(roles: &[(&str, PartRole)], other: PartRole, name: &str) -> PartRole {
    roles
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .map_or(other, |i| roles[i].1)
}

pub struct BlockModelDef {
    pub key: &'static str,
    pub model_file: &'static str,
    pub cells: [u8; 3],
    pub collision: CollisionSpec,
    pub orientation: PlacementOrientation,
    pub fit: FitMode,
    /// This row's [`PartRole`] per authored cube NAME (name-sorted), applied
    /// after the cache load so rows sharing one `.bbmodel` show, collide with
    /// and aim at different parts of it. Empty for most rows.
    pub part_roles: &'static [(&'static str, PartRole)],
    pub other_parts: PartRole,
    /// Per-row translations of named cubes, in AUTHORED PIXELS (applied after
    /// the cache load, before the part roles) — how rows sharing one
    /// `.bbmodel` pose a part differently per variant: a fill surface rising
    /// with the composter's stages, a fluid level, a gauge needle.
    /// Name-sorted for deterministic application; empty for most rows.
    pub part_offsets: &'static [(&'static str, [f32; 3])],
    /// Authored cube names that are optional per placed instance, in a fixed order: bit `i` of a
    /// cell's parts mask shows `parts[i]`. They stay hidden until a mod sets the mask
    /// (`BlockCall::SetModelParts`), so the row looks like its base self until then.
    ///
    /// A machine with independent visual states uses this instead of a row per combination. The
    /// forge's basin holds any of five moulds, with or without metal, while the furnace is lit
    /// or not: 48 rows, or one row with a mask.
    ///
    /// Render only. Collision and selection stay the row's, so a placed machine's hitbox never
    /// changes under the player.
    pub parts: &'static [&'static str],
    pub tint_parts: &'static [&'static str],
    pub surfaces: &'static [super::SurfaceMaterial],
}

impl BlockModelDef {
    pub fn part_role(&self, name: &str) -> PartRole {
        part_role(self.part_roles, self.other_parts, name)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    #[default]
    Fill,
    /// Authored pixels map 1:1 onto the footprint grid — cell `(i,j,k)` IS
    /// authored `16i..16(i+1)`, no scaling, no centring. Geometry outside the
    /// box OVERHANGS (a hopper lip, a tray): it renders (assigned to the
    /// nearest footprint cell) and aims like the rest of the model, but never
    /// extends collision or placement beyond the footprint.
    /// Right for machines whose occupied space is smaller than their
    /// silhouette. Author the model resting at `y = 0` inside `0..16·cells`.
    Native,
    Centered,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModelDef {
    key: String,
    model_file: String,
    cells: [u8; 3],
    #[serde(default)]
    orientation: PlacementOrientation,
    #[serde(default)]
    fit: FitMode,
    #[serde(default)]
    part_roles: std::collections::BTreeMap<String, PartRole>,
    #[serde(default)]
    other_parts: PartRole,
    #[serde(default)]
    part_offsets: std::collections::BTreeMap<String, [f32; 3]>,
    #[serde(default)]
    parts: Vec<String>,
    #[serde(default)]
    tint_parts: Vec<String>,
    #[serde(default)]
    surfaces: Vec<super::SurfaceMaterial>,
}

pub(crate) struct ModelDefs {
    rows: &'static [BlockModelDef],
    kinds: Box<[BlockModelKind]>,
}

pub(crate) static DEFS: crate::content::Slot<ModelDefs> = crate::content::Slot::new(
    crate::content::stage::MODELS,
    &[crate::content::stage::TILES],
    load,
);

fn load(reg: &crate::content::ContentRegistry) -> Result<ModelDefs, String> {
    let rows = crate::registry::read_asset_catalog(
        reg.packs(),
        "models.json",
        "block model",
        parse_layers,
    )?
    .rows();
    check_shared_part_lists(rows)?;
    Ok(ModelDefs {
        rows,
        kinds: (0..rows.len())
            .map(|id| BlockModelKind(id as u16))
            .collect(),
    })
}

fn defs() -> &'static [BlockModelDef] {
    DEFS.current().rows
}

fn check_shared_part_lists(rows: &[BlockModelDef]) -> Result<(), String> {
    let mut seen: Vec<(&str, &BlockModelDef)> = Vec::new();
    for row in rows.iter().filter(|r| !r.parts.is_empty()) {
        match seen.iter().find(|(f, _)| *f == row.model_file) {
            Some((_, first)) if first.parts != row.parts => {
                return Err(format!(
                    "'{}' and '{}' share '{}' but declare different `parts` orders \
                     ({:?} vs {:?}); the mask is per placed block and survives a row swap",
                    first.key, row.key, row.model_file, first.parts, row.parts
                ));
            }
            Some(_) => {}
            None => seen.push((row.model_file, row)),
        }
    }
    Ok(())
}

fn parse_layers(texts: &[&str]) -> Result<crate::registry::Catalog<BlockModelDef>, String> {
    crate::registry::load_catalog_with_capacity(
        texts,
        |text| crate::registry::parse_rows::<RawModelDef>(text, "models", "key"),
        |r| &r.key,
        ENGINE_MODEL_KEYS,
        "block model",
        crate::registry::WIDE_ID_CAP,
        |r, id, names| {
            super::material::validate(&r.surfaces).map_err(|e| format!("{}: {e}", r.key))?;
            let part_roles: Vec<(&'static str, PartRole)> = r
                .part_roles
                .into_iter()
                .map(|(name, role)| (&*Box::leak(name.into_boxed_str()), role))
                .collect();
            let part_offsets: Vec<(&'static str, [f32; 3])> = r
                .part_offsets
                .into_iter()
                .map(|(p, off)| (&*Box::leak(p.into_boxed_str()), off))
                .collect();
            if r.parts.len() > MAX_MODEL_PARTS {
                return Err(format!(
                    "block model '{}' declares {} optional parts; the mask holds {MAX_MODEL_PARTS}",
                    r.key,
                    r.parts.len()
                ));
            }
            if let Some(dup) = r
                .parts
                .iter()
                .enumerate()
                .find_map(|(i, p)| r.parts[..i].contains(p).then_some(p))
            {
                return Err(format!(
                    "block model '{}' lists optional part '{dup}' twice; each bit needs its own \
                     cube, and the later bit would show nothing",
                    r.key
                ));
            }
            let parts: Vec<&'static str> = r
                .parts
                .into_iter()
                .map(|p| &*Box::leak(p.into_boxed_str()))
                .collect();
            let tint_parts: Vec<&'static str> = r
                .tint_parts
                .into_iter()
                .map(|p| &*Box::leak(p.into_boxed_str()))
                .collect();
            Ok(BlockModelDef {
                key: names.name(id).expect("id resolved from this table"),
                model_file: Box::leak(r.model_file.into_boxed_str()),
                cells: r.cells,
                collision: CollisionSpec::FromModel,
                orientation: r.orientation,
                fit: r.fit,
                part_roles: Box::leak(part_roles.into_boxed_slice()),
                other_parts: r.other_parts,
                part_offsets: Box::leak(part_offsets.into_boxed_slice()),
                parts: Box::leak(parts.into_boxed_slice()),
                tint_parts: Box::leak(tint_parts.into_boxed_slice()),
                surfaces: Box::leak(r.surfaces.into_boxed_slice()),
            })
        },
    )
}

#[inline]
pub fn def(kind: BlockModelKind) -> &'static BlockModelDef {
    &defs()[kind.0 as usize]
}

#[cfg(test)]
mod part_list_tests {
    use super::*;

    fn row(key: &'static str, file: &'static str, parts: &'static [&'static str]) -> BlockModelDef {
        BlockModelDef {
            key,
            model_file: file,
            parts,
            ..*def(BlockModelKind(0))
        }
    }

    #[test]
    fn rows_sharing_a_model_must_agree_on_part_order() {
        let same = [
            row("a", "m.bbmodel", &["coals", "lever"]),
            row("b", "m.bbmodel", &["coals", "lever"]),
            row("c", "m.bbmodel", &[]),
            row("d", "other.bbmodel", &["lever", "coals"]),
        ];
        assert!(check_shared_part_lists(&same).is_ok());

        let swapped = [
            row("a", "m.bbmodel", &["coals", "lever"]),
            row("b", "m.bbmodel", &["lever", "coals"]),
        ];
        assert!(check_shared_part_lists(&swapped).is_err());
    }
}
