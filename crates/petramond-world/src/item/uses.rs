/// An item's engine-implemented right-click use, referenced from its
/// `items.json` row: a bare handler name for parameterless handlers
/// (`"use": "shear"`) or a tagged object carrying the handler's row data
/// (`"use": {"bucket_fill": {"becomes": {"petramond:water": "petramond:water_bucket"}}}`
/// — params ride inside the handler object, like `effects.json` behaviors).
/// The row's key resolves at load (see `load::convert`; unknown names,
/// missing params, and unknown `becomes` items are load errors), and the
/// tick-side dispatch (`game::item_use`, `game::placement`) matches on the
/// resolved handler — never on concrete item ids — so packs can put an engine
/// use on their own items. Gameplay data the handler needs (the empty↔filled
/// bucket pair) rides the row too, like
/// [`DroppedReaction`](super::DroppedReaction)'s `result`: a pack's iron
/// bucket fills into the pack's OWN filled bucket, never a hardcoded engine
/// item.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemUse {
    /// Scoop a targeted fluid source into the held item: on success the held
    /// stack becomes the result the row declares FOR THAT FLUID. The keys of
    /// `fills` are the whole restriction — a bucket scoops exactly the fluids
    /// it has a result for, so a row listing every fluid is the universal
    /// bucket and a row listing one is restricted to it. Nothing here names a
    /// concrete fluid: a third fluid is one more row entry.
    BucketFill {
        /// Per scoopable fluid block, the item the held one turns into.
        fills: &'static [(crate::block::Block, super::ItemType)],
    },
    /// Empty the held item into the clicked cell as `fluid`: on success the
    /// held item becomes `becomes` (the row-declared empty counterpart).
    BucketPour {
        /// The item the held one turns into on a successful pour.
        becomes: super::ItemType,
        /// The fluid this bucket pours.
        fluid: crate::block::Block,
    },
    /// Shear the targeted mob (runs at the earlier shear stage, before block
    /// interaction — see `game::placement`'s `tick_place`).
    Shear,
}

/// How this item's USE CLICK resolves its block target — which raycast the
/// crosshair runs against the world while the item is held (`"use_ray"` in
/// `items.json`: `"solid"`, or `{"fluids": ["petramond:water"]}`).
/// Selection/mining stay on the normal fluid-transparent ray either way; this
/// only changes the target a use click (and `item_use_pre`) carries. Fluid
/// hits are recomputed authoritatively when the server latches the click, and
/// the same selected slot/item must still hold at tick consumption.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum UseRay {
    /// The normal selection ray: every fluid is transparent.
    #[default]
    Solid,
    /// A cell of any listed fluid stops the ray as a full cube (solids still
    /// stop it first) — for items that act ON a fluid surface (placing a boat;
    /// the bucket handlers run their own server-side fluid rays).
    Fluids(&'static [crate::block::Block]),
}

impl UseRay {
    /// Whether a cell of `fluid` stops this ray.
    pub fn stops_at(self, fluid: crate::block::Block) -> bool {
        match self {
            UseRay::Solid => false,
            UseRay::Fluids(fluids) => fluids.contains(&fluid),
        }
    }

    /// Whether this ray sees any fluid at all.
    pub fn sees_fluid(self) -> bool {
        matches!(self, UseRay::Fluids(fluids) if !fluids.is_empty())
    }
}
