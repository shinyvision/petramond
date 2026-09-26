use mod_sdk::*;
use serde::Deserialize;

use crate::keys::HUSBANDRY_DATA as KEY;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Eaten {
    Clear,
    Regress,
    Keep,
}

pub struct FoodGroup {
    pub blocks: Vec<BlockId>,
    pub eaten: Eaten,
}

pub struct HusbandryDef {
    pub key: String,
    pub kind: MobId,
    pub offspring: Option<(String, MobId)>,
    pub food: Vec<FoodGroup>,
    pub restore: i64,
}

impl HusbandryDef {
    pub fn kept(&self) -> bool {
        self.offspring.is_some()
    }

    pub fn food_group(&self, block: BlockId) -> Option<&FoodGroup> {
        self.food.iter().find(|group| group.blocks.contains(&block))
    }
}

/// The `farming:husbandry` entry a mob row carries — the schema other packs
/// write to make their own species graze, restore and breed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    /// Meals one bite restores, 1..=10.
    restore: i32,
    /// The species a bred pair produces; absent = a wild, unkept species.
    #[serde(default)]
    offspring: Option<String>,
    /// 1..=16 food groups.
    food: Vec<FoodSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FoodSpec {
    /// 1..=32 block rows the species eats.
    blocks: Vec<String>,
    /// What eating does to the block.
    eaten: Eaten,
}

impl Profile {
    /// The shape rules serde cannot state: the ranges.
    fn check(&self) -> Result<(), String> {
        if !(1..=10).contains(&self.restore) {
            return Err(format!("restore {} is outside 1..=10", self.restore));
        }
        if self.food.is_empty() || self.food.len() > 16 {
            return Err(format!("{} food groups (1..=16)", self.food.len()));
        }
        if let Some(group) = self
            .food
            .iter()
            .find(|g| g.blocks.is_empty() || g.blocks.len() > 32)
        {
            return Err(format!(
                "a food group lists {} blocks (1..=32)",
                group.blocks.len()
            ));
        }
        Ok(())
    }
}

pub fn resolve() -> Vec<HusbandryDef> {
    let rows = mobs_with_data_as::<Profile>(KEY);
    let names = mob_names(rows.iter().map(|(kind, _)| *kind).collect());
    rows.into_iter()
        .zip(names)
        .filter_map(|((kind, profile), key)| {
            let key = key?;
            match build(kind, &key, profile) {
                Ok(def) => Some(def),
                Err(reason) => {
                    log(&row_error(KEY, &key, &reason));
                    None
                }
            }
        })
        .collect()
}

/// Resolve a checked profile's names into a species definition.
fn build(kind: MobId, key: &str, profile: Profile) -> Result<HusbandryDef, String> {
    profile.check()?;
    let offspring = match profile.offspring {
        Some(name) => {
            let id = resolve_mob_logged(&name).ok_or(format!("unknown offspring '{name}'"))?;
            Some((name, id))
        }
        None => None,
    };
    let mut food = Vec::with_capacity(profile.food.len());
    for group in profile.food {
        let blocks = group
            .blocks
            .iter()
            .map(|name| resolve_block_logged(name).ok_or(format!("unknown block '{name}'")))
            .collect::<Result<Vec<_>, _>>()?;
        food.push(FoodGroup {
            blocks,
            eaten: group.eaten,
        });
    }
    Ok(HusbandryDef {
        key: key.into(),
        kind,
        offspring,
        food,
        restore: i64::from(profile.restore),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every husbandry entry this pack ships matches the schema and its
    /// ranges — the check a foreign pack's rows get at load.
    #[test]
    fn every_shipped_profile_matches_the_schema() {
        let rows = pack_rows_with_data(include_str!("../../pack/mobs.json"), "mobs", KEY);
        for (mob, entry) in &rows {
            let profile: Profile = parse_row_data(entry).unwrap_or_else(|e| panic!("{mob}: {e}"));
            profile.check().unwrap_or_else(|e| panic!("{mob}: {e}"));
        }
        assert!(rows.len() >= 2, "the rabbit and the sheep carry profiles");
    }

    #[test]
    fn a_misspelt_field_or_an_unknown_eaten_rule_is_rejected() {
        assert!(parse_row_data::<Profile>(r#"{"restore":3,"food":[],"ofspring":"x"}"#).is_err());
        assert!(parse_row_data::<Profile>(
            r#"{"restore":3,"food":[{"blocks":["a:b"],"eaten":"burn"}]}"#
        )
        .is_err());
    }

    #[test]
    fn out_of_range_profiles_fail_the_check() {
        let profile = |text: &str| parse_row_data::<Profile>(text).unwrap();
        let group = r#"{"blocks":["a:b"],"eaten":"keep"}"#;
        assert!(profile(&format!(r#"{{"restore":3,"food":[{group}]}}"#))
            .check()
            .is_ok());
        assert!(profile(&format!(r#"{{"restore":11,"food":[{group}]}}"#))
            .check()
            .is_err());
        assert!(profile(r#"{"restore":3,"food":[]}"#).check().is_err());
        assert!(
            profile(r#"{"restore":3,"food":[{"blocks":[],"eaten":"keep"}]}"#)
                .check()
                .is_err()
        );
    }
}
