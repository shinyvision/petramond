use petramond_ui::{UiMap, UiValue};
use petramond_world::item::ItemType;
use std::sync::Arc;

type RegistryIdentity = (usize, usize);

#[derive(Default)]
pub(super) struct Catalog {
    built_for: Option<(String, RegistryIdentity)>,
    items: Arc<[ItemType]>,
    rows: Arc<Vec<UiMap>>,
}

impl Catalog {
    pub(super) fn view(&mut self, query: &str) -> (Arc<[ItemType]>, Arc<Vec<UiMap>>) {
        let registry = ItemType::all();
        let identity = (registry.as_ptr() as usize, registry.len());
        let current = self
            .built_for
            .as_ref()
            .is_some_and(|(q, id)| q == query && *id == identity);
        if !current {
            self.items = matching(registry, query).into();
            self.rows = Arc::new(
                self.items
                    .iter()
                    .map(|i| {
                        UiMap::from([("item_icon".into(), UiValue::Str(i.registry_name().into()))])
                    })
                    .collect(),
            );
            self.built_for = Some((query.to_owned(), identity));
        }
        (self.items.clone(), self.rows.clone())
    }
}

fn matching(registry: &[ItemType], query: &str) -> Vec<ItemType> {
    let query = query.to_lowercase();
    let mut items: Vec<ItemType> = registry
        .iter()
        .copied()
        .filter(|i| {
            i.creative_visible()
                && (query.is_empty()
                    || i.registry_name().to_lowercase().contains(&query)
                    || i.name().to_lowercase().contains(&query))
        })
        .collect();
    items.sort_by_key(|i| i.world_tool().is_none());
    items
}

#[cfg(test)]
mod tests;
