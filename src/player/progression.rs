use std::collections::HashSet;

use petramond_world::item::{ItemSet, ItemType};

#[derive(Clone, Default)]
pub struct Progression {
    obtained: ItemSet,
    unlocked: Vec<String>,
    index: HashSet<String>,
}

impl Progression {
    #[inline]
    pub fn obtained(&self) -> &ItemSet {
        &self.obtained
    }

    #[inline]
    pub fn obtain(&mut self, item: ItemType) -> bool {
        item != ItemType::Air && self.obtained.insert(item)
    }

    #[inline]
    pub fn is_unlocked(&self, recipe: &str) -> bool {
        self.index.contains(recipe)
    }

    pub fn unlock(&mut self, recipe: &str) -> bool {
        if self.index.contains(recipe) {
            return false;
        }
        self.index.insert(recipe.to_owned());
        self.unlocked.push(recipe.to_owned());
        true
    }

    #[inline]
    pub fn unlocked(&self) -> &[String] {
        &self.unlocked
    }

    pub fn restore(&mut self, obtained: impl IntoIterator<Item = ItemType>, unlocked: Vec<String>) {
        self.obtained = obtained.into_iter().collect();
        self.unlocked = Vec::with_capacity(unlocked.len());
        self.index = HashSet::with_capacity(unlocked.len());
        for key in unlocked {
            if self.index.insert(key.clone()) {
                self.unlocked.push(key);
            }
        }
    }
}
