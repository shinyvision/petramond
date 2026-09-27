use crate::__rt::host_fn;

host_fn! {
    pub fn loot_roll(key: &str, seed: u64) -> Option<Vec<crate::ItemStackData>>
        => LootRoll { key: key.into(), seed } => Loot
}
