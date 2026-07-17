/// An item's engine-implemented right-click use, referenced from its
/// `items.json` row by name (`"use": "bucket_fill"`). The string-keyed
/// registry of engine handlers: [`from_name`](Self::from_name) resolves a
/// row's key at load, and the tick-side dispatch (`game::item_use`,
/// `game::placement`) matches on the resolved handler — never on concrete
/// item ids — so packs can put an engine use on their own items.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemUse {
    /// Scoop a targeted water source into the held item (the empty bucket).
    BucketFill,
    /// Empty the held item into the clicked cell as water (the full bucket).
    BucketPour,
    /// Shear the targeted mob (runs at the earlier shear stage, before block
    /// interaction — see `game::placement`'s `tick_place`).
    Shear,
}

/// How this item's USE CLICK resolves its block target — which raycast the
/// crosshair runs against the world while the item is held (`"use_ray"` in
/// `items.json`). Selection/mining stay on the normal water-transparent ray
/// either way; this only changes the target a use click (and `item_use_pre`)
/// carries. Water hits are recomputed authoritatively when the server latches
/// the click, and the same selected slot/item must still hold at tick
/// consumption.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UseRay {
    /// The normal selection ray: water is transparent.
    #[default]
    Solid,
    /// Any water cell stops the ray as a full cube (solids still stop it
    /// first) — for items that act ON water (placing a boat; the bucket
    /// handlers run their own server-side water rays and don't need this).
    Water,
}

impl ItemUse {
    /// Resolve an `items.json` `use` key to an engine handler. There is no
    /// namespaced (`mod_id:key`) form: a mod reacts to its item's use through
    /// the `item_use_pre` event instead of declaring a handler.
    pub fn from_name(name: &str) -> Option<ItemUse> {
        Some(match name {
            "bucket_fill" => ItemUse::BucketFill,
            "bucket_pour" => ItemUse::BucketPour,
            "shear" => ItemUse::Shear,
            _ => return None,
        })
    }
}
