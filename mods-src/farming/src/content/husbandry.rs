use mod_sdk::*;

const KEY: &str = "farming:husbandry";

#[derive(Copy, Clone, PartialEq, Eq)]
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

pub fn resolve() -> Vec<HusbandryDef> {
    let rows = mobs_with_data(KEY);
    let names = mob_names(rows.iter().map(|(kind, _)| *kind).collect());
    rows.into_iter()
        .zip(names)
        .filter_map(|((kind, text), key)| {
            let key = key?;
            let def = parse(kind, &key, &text);
            if def.is_none() {
                log(&format!("farming: invalid husbandry profile on '{key}'"));
            }
            def
        })
        .collect()
}

fn parse(kind: MobId, key: &str, text: &str) -> Option<HusbandryDef> {
    let value = json::Value::parse(text)?;
    let restore = value.get("restore")?.as_i32()?;
    if !(1..=10).contains(&restore) {
        return None;
    }
    let offspring = match value.get("offspring") {
        Some(json::Value::Null) | None => None,
        Some(value) => {
            let key = value.as_str()?;
            Some((key.to_owned(), resolve_mob_logged(key)?))
        }
    };
    let groups = value.get("food")?.as_array()?;
    if groups.is_empty() || groups.len() > 16 {
        return None;
    }
    let mut food = Vec::with_capacity(groups.len());
    for group in groups {
        let eaten = match group.get("eaten")?.as_str()? {
            "clear" => Eaten::Clear,
            "regress" => Eaten::Regress,
            "keep" => Eaten::Keep,
            _ => return None,
        };
        let names = group.get("blocks")?.as_array()?;
        if names.is_empty() || names.len() > 32 {
            return None;
        }
        let blocks = names
            .iter()
            .map(|name| resolve_block_logged(name.as_str()?))
            .collect::<Option<Vec<_>>>()?;
        food.push(FoodGroup { blocks, eaten });
    }
    Some(HusbandryDef {
        key: key.into(),
        kind,
        offspring,
        food,
        restore: i64::from(restore),
    })
}
