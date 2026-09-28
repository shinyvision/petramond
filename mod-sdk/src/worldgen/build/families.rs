use serde::Deserialize;

use super::material::{Form, Material, Name};
use crate::FxHashMap;

/// Block row data naming a block's material family and its form in it:
/// `"petramond:material": {"family": "oak", "form": "slab"}`.
pub const MATERIAL_DATA_KEY: &str = "petramond:material";

#[derive(Deserialize)]
struct Row {
    family: String,
    form: String,
}

/// Every material family the loaded packs declare, by family and form.
#[derive(Clone, Debug, Default)]
pub struct Families {
    by_family: FxHashMap<Name, [Option<Name>; Form::ALL.len()]>,
    in_name_order: Vec<Name>,
}

impl Families {
    pub fn load() -> Families {
        let rows = crate::blocks_with_data_as::<Row>(MATERIAL_DATA_KEY);
        let ids: Vec<_> = rows.iter().map(|(id, _)| *id).collect();
        let names = crate::block_names(ids);
        Families::from_rows(
            rows.into_iter()
                .zip(names)
                .filter_map(|((_, row), name)| Some((name?, row.family, row.form))),
        )
    }

    pub fn from_rows(rows: impl IntoIterator<Item = (String, String, String)>) -> Families {
        let mut families = Families::default();
        for (block, family, form) in rows {
            let Some(form) = Form::parse(&form) else {
                crate::log(&format!(
                    "{MATERIAL_DATA_KEY}: unknown form '{form}' on {block}"
                ));
                continue;
            };
            let family = Name::new(&family);
            let forms = families.by_family.entry(family).or_insert_with(|| {
                families.in_name_order.push(family);
                [None; Form::ALL.len()]
            });
            forms[form as usize] = Some(Name::new(&block));
        }
        families.in_name_order.sort_unstable_by_key(|n| n.as_str());
        families
    }

    #[inline]
    pub fn get(&self, family: Name, form: Form) -> Option<Material> {
        let block = self.by_family.get(&family)?[form as usize]?;
        Some(Material::of(block, form).in_family(family))
    }

    #[inline]
    pub fn has(&self, family: Name, form: Form) -> bool {
        self.by_family
            .get(&family)
            .is_some_and(|forms| forms[form as usize].is_some())
    }

    /// Families that declare every one of `forms`, in name order.
    pub fn with_forms(&self, forms: &[Form]) -> Vec<Name> {
        self.in_name_order
            .iter()
            .copied()
            .filter(|family| forms.iter().all(|&f| self.has(*family, f)))
            .collect()
    }

    /// The same member of `material`'s family in another form (a slab's full block).
    pub fn sibling(&self, material: &Material, form: Form) -> Option<Material> {
        self.get(material.family?, form)
    }
}
