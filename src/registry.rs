//! Shared id-ordered registry abstraction.
//!
//! `biome`, `block`, and `item` each index a `#[repr(u8)]` enum into an
//! id-ordered `&'static [Def]` table. The lookup pattern — `from_id` (clamp an
//! arbitrary `u8` to a known key, falling back to a default) and `def` (read the
//! row for a key, with a load-bearing ordering `debug_assert`) — is identical
//! across all three. This module carries that pattern (and its ordering assert)
//! once; each module supplies only its `Def` type + table and a one-line call.
//!
//! A `Def` row implements [`TableEntry`], pairing the row with the enum key it
//! belongs to. The key implements [`RegistryKey`] (its stable `u8` id). The two
//! free functions [`from_id`] / [`def`] then work over any such table.

/// A `#[repr(u8)]` registry key: its stable numeric id (`enum as u8`). The id is
/// the table index, so a table is "id-ordered" iff `DEFS[k.to_id()].key() == k`.
pub(crate) trait RegistryKey: Copy + PartialEq {
    fn first_id() -> u8 {
        0
    }

    fn to_id(self) -> u8;
}

/// One row of an id-ordered registry table: the row, paired with the enum key it
/// describes. The table `&[E]` is ordered by `E::key().to_id()`.
pub(crate) trait TableEntry {
    type Key: RegistryKey;
    fn key(&self) -> Self::Key;
}

/// The key for `id`, or `fallback` if `id` is out of range. Mirrors each module's
/// `from_id` (e.g. `Block::from_id(u8::MAX) == Block::Air`).
#[inline]
pub(crate) fn from_id<E: TableEntry>(defs: &'static [E], id: u8, fallback: E::Key) -> E::Key {
    let first = E::Key::first_id();
    let Some(index) = id.checked_sub(first) else {
        return fallback;
    };
    defs.get(index as usize).map_or(fallback, |d| d.key())
}

/// The `'static` row for `key`. Indexes by `key.to_id()`, guarded by the
/// ordering `debug_assert` that every module previously inlined.
#[inline]
pub(crate) fn def<E: TableEntry>(defs: &'static [E], key: E::Key) -> &'static E {
    let index = (key.to_id() - E::Key::first_id()) as usize;
    debug_assert!(
        index < defs.len() && defs[index].key() == key,
        "registry table must be ordered by key id()"
    );
    &defs[index]
}

/// Assert a registry table is id-ordered and one-to-one over `expected` (the full
/// key list in id order): the table has one row per key, every row sits at its
/// key's id, and `from_id` round-trips each key. `expected` is the module's `ALL`
/// surface. The single generic body behind every module's id-ordering check —
/// each module calls this once (its `Def` type stays private to the module).
#[cfg(test)]
pub(crate) fn assert_id_ordered<E: TableEntry>(defs: &'static [E], expected: &[E::Key])
where
    E::Key: core::fmt::Debug,
{
    assert_eq!(
        defs.len(),
        expected.len(),
        "table length must equal key count"
    );
    for (id, &key) in expected.iter().enumerate() {
        let expected_id = id + E::Key::first_id() as usize;
        assert_eq!(key.to_id() as usize, expected_id, "{key:?} out of id order");
        let row = &defs[id];
        assert_eq!(row.key(), key, "row {id} describes the wrong key");
        // The arbitrary fallback never masks an in-range key.
        assert_eq!(
            from_id(defs, expected_id as u8, key),
            key,
            "from_id({expected_id}) mismatch"
        );
    }
}

#[cfg(test)]
mod tests {
    /// One test replacing the three per-module `definitions_are_id_ordered` copies:
    /// every registry table is id-ordered and one-to-one with its key list. Each
    /// module's `assert_registry_ordered` is a one-line delegating call into the
    /// shared generic [`super::assert_id_ordered`] (so the private `Def` types
    /// never leave their module).
    #[test]
    fn definitions_are_id_ordered() {
        crate::biome::assert_registry_ordered();
        crate::block::assert_registry_ordered();
        crate::item::assert_registry_ordered();
    }
}
