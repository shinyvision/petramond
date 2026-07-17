/// A named group of items shared across recipes (e.g. any wood planks). Tags are
/// a PROPERTY OF ITEMS: each item lists its tags in its [`ItemDef`](definition::ItemDef)
/// data row, a recipe references a tag by name, and the crafting matcher asks each
/// item whether it carries the tag (see [`ItemType::has_tag`]). Keeping membership
/// in item data (not the recipe loader) means a new item joins a group by editing
/// its data row, never any recipe code.
///
/// The vocabulary is OPEN: engine tags are the named consts below (bare
/// snake_case in `items.json`, `petramond:<name>` in recipes); a pack introduces
/// its own tag by listing a namespaced `mod_id:name` on item rows and
/// referencing `mod_id:name` in recipes (interned at load — see
/// [`crate::registry::TagTable`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ItemTag(u8);

/// Engine item-tag names, id-ordered to match the consts on [`ItemTag`].
static ITEM_TAGS: crate::registry::TagTable =
    crate::registry::TagTable::new(&["planks", "logs", "fuel", "smeltable", "shovels"]);

impl ItemTag {
    /// Any wood-type planks (recipe selector `petramond:planks`).
    pub const PLANKS: ItemTag = ItemTag(0);
    /// Any wood-type log (recipe selector `petramond:logs`).
    pub const LOGS: ItemTag = ItemTag(1);
    /// Anything that burns as furnace fuel — shift-clicked into the fuel slot.
    pub const FUEL: ItemTag = ItemTag(2);
    /// Anything a furnace can smelt — shift-clicked into the input slot.
    pub const SMELTABLE: ItemTag = ItemTag(3);
    /// Any shovel-class digging tool (recipe selector `petramond:shovels`). Packs
    /// opt compatible shovels in by listing the tag on their item rows.
    pub const SHOVELS: ItemTag = ItemTag(4);

    /// Resolve a tag's registry name (a recipe selector or an
    /// `items.json` row entry), interning an unseen namespaced pack tag.
    /// `None` only for invalid names (a bare non-engine name).
    pub fn from_key(key: &str) -> Option<ItemTag> {
        ITEM_TAGS.resolve(key).ok().map(ItemTag)
    }

    /// The registered name for this tag (engine tags bare, pack tags
    /// namespaced) — the inverse of [`from_key`](Self::from_key).
    pub fn name(self) -> &'static str {
        ITEM_TAGS.name(self.0)
    }

    /// Loader-side [`from_key`](Self::from_key) that surfaces the error text.
    pub(crate) fn resolve(name: &str) -> Result<ItemTag, String> {
        ITEM_TAGS.resolve(name).map(ItemTag)
    }
}
