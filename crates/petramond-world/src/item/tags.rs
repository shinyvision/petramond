/// Group of items across recipes, like wood planks. Lives on the item, not the recipe.
/// `ItemDef` row lists tags, recipe just names one, matcher does `ItemType::has_tag`. Want
/// an item in a group? Edit its row. No recipe code needed.
///
/// Vocabulary isn't fixed. Consts below are engine tags, snake_case in `items.json`,
/// `petramond:<name>` in recipes. Packs put `mod_id:name` on rows and recipes, `TagTable`
/// interns at load.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ItemTag(u8);

static ITEM_TAGS: crate::content::Slot<crate::registry::TagTable> =
    crate::content::Slot::new("item tags", &[], |_| {
        Ok(crate::registry::TagTable::new(&[
            "planks",
            "logs",
            "fuel",
            "smeltable",
            "shovels",
            "raw_ore",
            "flowers",
        ]))
    });

impl ItemTag {
    pub const PLANKS: ItemTag = ItemTag(0);
    pub const LOGS: ItemTag = ItemTag(1);
    pub const FUEL: ItemTag = ItemTag(2);
    pub const SMELTABLE: ItemTag = ItemTag(3);
    pub const SHOVELS: ItemTag = ItemTag(4);
    pub const RAW_ORE: ItemTag = ItemTag(5);

    pub fn from_key(key: &str) -> Option<ItemTag> {
        ITEM_TAGS.current().resolve(key).ok().map(ItemTag)
    }

    pub fn name(self) -> &'static str {
        ITEM_TAGS.current().name(self.0)
    }

    pub fn resolve(name: &str) -> Result<ItemTag, String> {
        ITEM_TAGS.current().resolve(name).map(ItemTag)
    }

    pub fn lookup(name: &str) -> Option<ItemTag> {
        ITEM_TAGS.current().lookup(name).map(ItemTag)
    }
}
