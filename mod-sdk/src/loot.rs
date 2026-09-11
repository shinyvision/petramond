use crate::__rt::host_fn;

host_fn! {
    /// Sample an authored reward table. The caller owns delivery and stores
    /// completion with the recipient to prevent duplicate rewards after reload.
    pub fn loot_roll(key: &str, seed: u64) -> Option<Vec<crate::ItemStackData>>
        => LootRoll { key: key.into(), seed } => Loot
}
