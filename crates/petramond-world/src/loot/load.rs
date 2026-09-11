use super::*;
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    loot_tables: Vec<Row>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    loot: String,
    pools: Vec<RawPool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPool {
    rolls: [u8; 2],
    entries: Vec<RawEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    weight: u32,
    result: RawOutcome,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RawOutcome {
    Item { item: String, count: [u8; 2] },
    Table { table: String },
    Empty,
}

static LOOT: LazyLock<Loot> = LazyLock::new(|| {
    crate::registry::read_catalog("loot_tables.json", "loot", |layers| {
        parse_layers(layers, |key| {
            crate::registry::names().items.id(key).map(ItemType)
        })
    })
});

/// Load and validate the installed reward catalog once.
pub fn catalog() -> &'static Loot {
    &LOOT
}

/// Shared parser for runtime catalogs, asset validation, and authoring tools.
pub fn parse_layers(
    layers: &[&str],
    resolve: impl Fn(&str) -> Option<ItemType>,
) -> Result<Loot, String> {
    let tables = crate::registry::load_catalog(
        layers,
        |text| serde_json::from_str::<File>(text).map(|file| file.loot_tables),
        |row| &row.loot,
        &["petramond:owl_drops"],
        "loot",
        |row, _, names| {
            if row.pools.len() > 16 {
                return Err(format!("loot '{}': at most 16 pools", row.loot));
            }
            let mut pools = Vec::new();
            for pool in row.pools {
                if pool.rolls[0] > pool.rolls[1]
                    || pool.rolls[1] > 32
                    || pool.entries.is_empty()
                    || pool.entries.len() > 128
                {
                    return Err(format!("loot '{}': invalid rolls or entries", row.loot));
                }
                let mut entries = Vec::new();
                let mut weight = 0u32;
                for entry in pool.entries {
                    weight = weight
                        .checked_add(entry.weight)
                        .filter(|_| entry.weight > 0)
                        .ok_or_else(|| format!("loot '{}': invalid total weight", row.loot))?;
                    let outcome = match entry.result {
                        RawOutcome::Item { item, count } => {
                            let item = resolve(&item).ok_or_else(|| {
                                format!("loot '{}': unknown item '{item}'", row.loot)
                            })?;
                            if count[0] == 0
                                || count[0] > count[1]
                                || count[1] > item.max_stack_size()
                            {
                                return Err(format!("loot '{}': invalid stack count", row.loot));
                            }
                            Outcome::Item(item, count)
                        }
                        RawOutcome::Table { table } => {
                            Outcome::Table(names.id(&table).ok_or_else(|| {
                                format!("loot '{}': unknown table '{table}'", row.loot)
                            })?)
                        }
                        RawOutcome::Empty => Outcome::Empty,
                    };
                    entries.push(Entry {
                        weight: entry.weight,
                        outcome,
                    });
                }
                pools.push(Pool {
                    rolls: pool.rolls,
                    weight,
                    entries,
                });
            }
            Ok(Table { pools })
        },
    )?;
    let mut memo = vec![None; tables.rows().len()];
    let mut visiting = vec![false; memo.len()];
    for id in 0..memo.len() {
        budget(id, &tables, &mut memo, &mut visiting, 0)?;
    }
    Ok(Loot { tables })
}

fn budget(
    id: usize,
    tables: &Catalog<Table>,
    memo: &mut [Option<(usize, usize)>],
    visiting: &mut [bool],
    depth: usize,
) -> Result<(usize, usize), String> {
    if depth >= MAX_DEPTH || visiting[id] {
        return Err("loot tables contain a cycle or exceed 16 nested tables".into());
    }
    if let Some(value) = memo[id] {
        if depth + value.1 >= MAX_DEPTH {
            return Err("loot tables exceed 16 nested tables".into());
        }
        return Ok(value);
    }
    visiting[id] = true;
    let (mut total, mut height) = (0, 0);
    for pool in &tables.rows()[id].pools {
        let mut worst = 0;
        for entry in &pool.entries {
            let (cost, below) = match entry.outcome {
                Outcome::Table(child) => {
                    let (cost, height) = budget(child as usize, tables, memo, visiting, depth + 1)?;
                    (cost, height + 1)
                }
                Outcome::Item(..) => (1, 0),
                Outcome::Empty => (0, 0),
            };
            // Count visits as well as output: nested empty tables still cost work.
            worst = worst.max(cost + 1);
            height = height.max(below);
        }
        total += worst * pool.rolls[1] as usize;
        if total > MAX_EXPANSION {
            return Err("loot table expansion exceeds 256 entries".into());
        }
    }
    visiting[id] = false;
    memo[id] = Some((total, height));
    Ok((total, height))
}
