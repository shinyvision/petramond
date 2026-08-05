//! The pack's item-row DATA, read once at init.
//!
//! Neither table names a single item in code. Both are enumerations of a data
//! key (`items_with_data`), so a mould or a meltable shipped by ANOTHER pack
//! joins them at load with nothing to change here — which is the whole point of
//! putting the vocabulary on the row instead of in the machine.

use std::collections::HashMap;

use mod_sdk::*;

/// Melt time for a metal with no `forge:metal` entry.
const MELT_TICKS_DEFAULT: u32 = 240;
/// And its colour: plain hot iron.
const MOLTEN_DEFAULT: [u8; 3] = [168, 96, 54];
const SOLID_DEFAULT: [u8; 3] = [96, 62, 44];

/// One meltable metal, as its item row describes it.
#[derive(Clone)]
pub struct Metal {
    pub melt_ticks: u32,
    /// What a crucible of it, and the metal running out of the spout, look
    /// like: vibrant while liquid, the same hue gone dull once it has set.
    pub molten: [u8; 3],
    pub solid: [u8; 3],
    /// What the crucible tooltip calls it ("Iron", not "Raw Iron" — the
    /// crucible holds a metal, not an item form). Optional: a metal with no
    /// `name` gets no tooltip.
    pub name: Option<String>,
    /// The item this one BECOMES once molten, when it is not itself the
    /// canonical form of its metal — a plate melted back down is iron again,
    /// not "iron plate that happens to be liquid".
    ///
    /// Every cast route is then keyed on the canonical metal alone, so a pack
    /// adding another source of iron declares one field and gets every mould
    /// it can pour into for free. Without it each new source needs a full row
    /// per class, and the row nobody writes is a pour that silently produces
    /// nothing.
    pub melts_to: Option<String>,
}

impl Default for Metal {
    fn default() -> Self {
        Metal {
            melt_ticks: MELT_TICKS_DEFAULT,
            molten: MOLTEN_DEFAULT,
            solid: SOLID_DEFAULT,
            name: None,
            melts_to: None,
        }
    }
}

/// What the forging furnace knows about the items it is handed.
#[derive(Default)]
pub struct Casting {
    /// Item registry name → the machine-recipe CLASS that mould casts.
    moulds: HashMap<String, String>,
    /// Item registry name → how it melts and what colour it is.
    metals: HashMap<String, Metal>,
}

/// One `forge:metal` row exactly as written — every field OPTIONAL, so
/// [`Casting::resolve`] can tell "the row said 240" from "the row said
/// nothing" and inherit the second case.
struct RawMetal {
    melt_ticks: Option<u32>,
    molten: Option<[u8; 3]>,
    solid: Option<[u8; 3]>,
    name: Option<String>,
    melts_to: Option<String>,
}

/// Fill a derived metal's gaps from the CANONICAL metal it melts to.
///
/// A plate and an ingot are iron; saying so twice is two numbers that drift,
/// and the drift is silent — a re-tuned melt time that half the sources of a
/// metal ignore, or a colour that only the raw ore is drawn in. One row states
/// a metal's numbers and everything that melts INTO it names it.
///
/// One level only, and deliberately: a derived form points at a raw metal, and
/// a chain would let two rows point at each other.
fn resolve_metals(raw: HashMap<String, RawMetal>) -> HashMap<String, Metal> {
    let canonical = |name: &String, m: &RawMetal| -> Option<&RawMetal> {
        m.melts_to
            .as_ref()
            .filter(|to| *to != name)
            .and_then(|to| raw.get(to))
    };
    raw.iter()
        .map(|(name, m)| {
            let from = canonical(name, m);
            (
                name.clone(),
                Metal {
                    melt_ticks: m
                        .melt_ticks
                        .or_else(|| from?.melt_ticks)
                        .unwrap_or(MELT_TICKS_DEFAULT),
                    molten: m.molten.or_else(|| from?.molten).unwrap_or(MOLTEN_DEFAULT),
                    solid: m.solid.or_else(|| from?.solid).unwrap_or(SOLID_DEFAULT),
                    name: m.name.clone().or_else(|| from?.name.clone()),
                    melts_to: m.melts_to.clone(),
                },
            )
        })
        .collect()
}

impl Casting {
    pub fn resolve() -> Casting {
        let moulds = read_rows("forge:mould", |value| {
            value.get("class")?.as_str().map(str::to_owned)
        });
        let metals = resolve_metals(read_rows("forge:metal", |value| {
            Some(RawMetal {
                melt_ticks: value
                    .get("melt_ticks")
                    .and_then(|t| t.as_f64())
                    .map(|t| t.max(1.0) as u32),
                molten: rgb(value.get("molten")),
                solid: rgb(value.get("solid")),
                name: value.get("name").and_then(|t| t.as_str()).map(str::to_owned),
                melts_to: value
                    .get("melts_to")
                    .and_then(|t| t.as_str())
                    .map(str::to_owned),
            })
        }));
        // The member counts are the only cheap signal that the pack's data keys
        // and this module's constants are the same strings — a typo on either
        // side is silent everywhere else.
        log(&format!(
            "forge: {} mould rows, {} metal rows",
            moulds.len(),
            metals.len()
        ));
        Casting { moulds, metals }
    }

    /// The recipe class the mould in the basin casts, or `None` for anything
    /// that is not a mould.
    pub fn mould_class(&self, item: &str) -> Option<&str> {
        self.moulds.get(item).map(String::as_str)
    }

    /// How `item` melts. The defaults cover a row that OMITS fields, not an
    /// item with no row at all: the table is the whitelist — [`is_metal`] —
    /// and an item outside it never reaches the crucible, which is what stops
    /// the metal slot from melting whatever you shovel into it. A pack adds a
    /// meltable by shipping the row, not by leaving it out.
    ///
    /// [`is_metal`]: Self::is_metal
    pub fn metal(&self, item: &str) -> Metal {
        self.metals.get(item).cloned().unwrap_or_default()
    }

    pub fn is_metal(&self, item: &str) -> bool {
        self.metals.contains_key(item)
    }

    /// What the crucible REMEMBERS after melting `item`: the canonical form of
    /// its metal. This is what every cast route is keyed on.
    pub fn molten_form(&self, item: &str) -> String {
        self.metals
            .get(item)
            .and_then(|m| m.melts_to.clone())
            .unwrap_or_else(|| item.to_owned())
    }
}

#[cfg(test)]
impl Casting {
    /// Build a table straight from `(item, class)` / `(item, melts_to)` pairs.
    /// The real one comes off item rows through the host, which a mod crate's
    /// tests cannot reach — but the machine's RULES (what melts, what mixes,
    /// what a mould names) are the part worth pinning, and they are pure.
    pub fn for_test(moulds: &[(&str, &str)], metals: &[(&str, Option<&str>)]) -> Casting {
        Casting {
            moulds: moulds
                .iter()
                .map(|(i, c)| ((*i).to_owned(), (*c).to_owned()))
                .collect(),
            metals: metals
                .iter()
                .map(|(i, to)| {
                    (
                        (*i).to_owned(),
                        Metal {
                            melts_to: to.map(str::to_owned),
                            ..Metal::default()
                        },
                    )
                })
                .collect(),
        }
    }
}

fn rgb(value: Option<&json::Value>) -> Option<[u8; 3]> {
    let a = value?.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([a[0].as_u8()?, a[1].as_u8()?, a[2].as_u8()?])
}

/// Enumerate every item carrying `key` and parse its value, resolving the ids
/// back to registry names in ONE batched call.
pub fn read_rows<T>(key: &str, parse: impl Fn(&json::Value) -> Option<T>) -> HashMap<String, T> {
    let rows = items_with_data(key);
    if rows.is_empty() {
        return HashMap::new();
    }
    let ids: Vec<ItemId> = rows.iter().map(|(id, _)| *id).collect();
    let names = item_names(ids);
    let mut out = HashMap::with_capacity(rows.len());
    for ((_, text), name) in rows.iter().zip(names) {
        let (Some(name), Some(value)) = (name, json::Value::parse(text)) else {
            continue;
        };
        if let Some(parsed) = parse(&value) {
            out.insert(name, parsed);
        } else {
            log(&format!(
                "forge: item '{name}' has a malformed '{key}' entry"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn casting() -> Casting {
        let mut metals = HashMap::new();
        metals.insert(
            "petramond:raw_iron".into(),
            Metal {
                melts_to: None,
                ..Metal::default()
            },
        );
        metals.insert(
            "forge:iron_plate".into(),
            Metal {
                melts_to: Some("petramond:raw_iron".into()),
                ..Metal::default()
            },
        );
        let mut moulds = HashMap::new();
        moulds.insert("forge:axe_head_mould".into(), "forge:cast_axe_head".into());
        Casting { moulds, metals }
    }

    /// Every cast route is keyed on the CANONICAL metal, so anything melted
    /// down has to arrive as that. Without it a plate poured back into an
    /// empty basin matches no recipe: the crucible unit is spent and nothing
    /// comes out, which the player only sees as the machine doing nothing.
    #[test]
    fn a_melted_down_product_becomes_its_metal_again() {
        let c = casting();
        assert_eq!(c.molten_form("forge:iron_plate"), "petramond:raw_iron");
        assert_eq!(c.molten_form("petramond:raw_iron"), "petramond:raw_iron");
        // A metal that declares no `melts_to` is its OWN canonical form, so a
        // row may omit the field entirely and still key every cast route.
        assert_eq!(c.molten_form("other:mystery"), "other:mystery");
    }

    /// A METAL'S NUMBERS ARE STATED ONCE. Every derived form (a plate, an
    /// ingot) declares only what it IS — `melts_to` — and takes its melt time
    /// and both colours from the metal it becomes. Copy them per row instead
    /// and a re-tuned melt is a re-tune only some sources of that metal got,
    /// with nothing to fail.
    #[test]
    fn a_derived_form_inherits_its_metals_numbers() {
        let raw = |melts_to: Option<&str>, ticks: Option<u32>| RawMetal {
            melt_ticks: ticks,
            molten: ticks.map(|_| [1, 2, 3]),
            solid: ticks.map(|_| [4, 5, 6]),
            name: ticks.map(|_| "Iron".to_owned()),
            melts_to: melts_to.map(str::to_owned),
        };
        let metals = resolve_metals(HashMap::from([
            ("petramond:raw_iron".to_owned(), raw(None, Some(300))),
            (
                "forge:iron_plate".to_owned(),
                raw(Some("petramond:raw_iron"), None),
            ),
            ("other:mystery".to_owned(), raw(Some("other:absent"), None)),
        ]));
        let plate = &metals["forge:iron_plate"];
        assert_eq!(plate.melt_ticks, 300, "the plate melts at iron's rate");
        assert_eq!((plate.molten, plate.solid), ([1, 2, 3], [4, 5, 6]));
        assert_eq!(
            plate.name.as_deref(),
            Some("Iron"),
            "a plate melted down is iron — the crucible names the metal, not the form"
        );
        // A row naming a metal nobody declared still gets a working default
        // rather than a zero-tick melt — and no name, so no tooltip.
        assert_eq!(metals["other:mystery"].melt_ticks, MELT_TICKS_DEFAULT);
        assert_eq!(metals["other:mystery"].name, None);
    }

    #[test]
    fn only_a_mould_names_a_cast_class() {
        let c = casting();
        assert_eq!(
            c.mould_class("forge:axe_head_mould"),
            Some("forge:cast_axe_head")
        );
        assert_eq!(c.mould_class("petramond:raw_iron"), None);
    }
}
