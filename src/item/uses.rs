/// An item's engine-implemented right-click use, referenced from its
/// `items.json` row: a bare handler name for parameterless handlers
/// (`"use": "shear"`) or a tagged object carrying the handler's row data
/// (`"use": {"bucket_fill": {"becomes": "petramond:water_bucket"}}` — params
/// ride inside the handler object, like `effects.json` behaviors). The row's
/// key resolves at load (see `load::convert`; unknown names, missing params,
/// and unknown `becomes` items are load errors), and the tick-side dispatch
/// (`game::item_use`, `game::placement`) matches on the resolved handler —
/// never on concrete item ids — so packs can put an engine use on their own
/// items. Gameplay data the handler needs (the empty↔filled bucket pair)
/// rides the row too, like [`DroppedReaction`](super::DroppedReaction)'s
/// `result`: a pack's iron bucket fills into the pack's OWN filled bucket,
/// never a hardcoded engine item.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemUse {
    /// Scoop a targeted water source into the held item: on success the held
    /// stack becomes `becomes` (the row-declared filled counterpart).
    BucketFill {
        /// The item the held one turns into on a successful scoop.
        becomes: super::ItemType,
    },
    /// Empty the held item into the clicked cell as water: on success the held
    /// item becomes `becomes` (the row-declared empty counterpart).
    BucketPour {
        /// The item the held one turns into on a successful pour.
        becomes: super::ItemType,
    },
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
