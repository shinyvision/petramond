use super::ItemType;

/// What family of tool an item is, for mining. A tool speeds up the block class
/// it is *for* — a [`Pickaxe`](ToolKind::Pickaxe) mines stone & ore, an
/// [`Axe`](ToolKind::Axe) mines wood, a [`Shovel`](ToolKind::Shovel) mines dirt &
/// sand, [`Shears`](ToolKind::Shears) mine wool — and a wrong-kind tool (an axe
/// on stone, a shovel on a log) mines no faster than a bare hand and unlocks no
/// drop. The block half of this pairing is
/// [`Block::preferred_tool`](crate::block::Block::preferred_tool).
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Pickaxe,
    Axe,
    Shovel,
    Shears,
}

impl ToolKind {
    /// The snake_case row name (`"pickaxe"`, …) — the same string the
    /// `items.json` `tool.kind` field carries.
    pub fn name(self) -> &'static str {
        match self {
            ToolKind::Pickaxe => "pickaxe",
            ToolKind::Axe => "axe",
            ToolKind::Shovel => "shovel",
            ToolKind::Shears => "shears",
        }
    }

    /// How effective this kind of tool is at mining its own block class, as a
    /// multiplier on the shared material-tier speed ladder (see
    /// [`crate::mining::break_time`]). A pickaxe and an axe are the baseline
    /// (`1.0`); a shovel is a clumsier digging implement, so it clears its dirt &
    /// sand at `0.5625` of the speed an equal-tier pickaxe gets on stone —
    /// uniformly slower at every tier, because the factor scales the whole ladder.
    /// Tuned low enough that even a diamond shovel (the ×8 tier) tops out at ×4.5,
    /// the dirt-clearing rate of an iron-tier tool. This is a property of the tool
    /// KIND (the real reason a shovel digs slower), separate from the material
    /// `tier` it shares with the other kinds.
    #[inline]
    pub fn mining_efficiency(self) -> f32 {
        match self {
            ToolKind::Pickaxe | ToolKind::Axe | ToolKind::Shears => 1.0,
            // 0.5625 = 9/16: scales the ×8 diamond tier down to ×4.5.
            ToolKind::Shovel => 0.5625,
        }
    }
}

/// A mining tool: its [`kind`](Self::kind) and material `tier` (`1` = wooden,
/// `2` = stone, `3` = iron, `4` = diamond). Read from an item via
/// [`ItemType::tool`]; the mining model (see [`crate::mining`]) keys both the
/// speed multiplier and the harvest gate off it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Tool {
    pub kind: ToolKind,
    pub tier: u8,
}

impl Tool {
    /// The melee damage range `(min, max)` this tool rolls per hit. A weapon's damage
    /// is a property of the tool itself — its KIND and material TIER: axes hit hardest,
    /// shovels and pickaxes share a gentler curve, and every diamond tool one-shots a
    /// small mob. The attacker rolls a uniform value in this range each swing, so a
    /// tool's hits-to-kill against a given mob spans a small band rather than a fixed
    /// count (a flat integer-per-hit couldn't produce e.g. "3–4 hits" on 4 health).
    pub fn attack_damage(self) -> (f32, f32) {
        use ToolKind::*;
        // Diamond is uniformly lethal regardless of kind.
        if self.tier >= 4 {
            return (5.0, 7.0);
        }
        match (self.kind, self.tier) {
            (Axe, 1) => (1.5, 2.5),
            (Axe, 2) => (2.0, 3.0),
            (Axe, 3) => (4.0, 6.0),
            // Shovels and pickaxes share a curve (clumsier weapons than an axe).
            (_, 1) => (1.0, 1.5),
            (_, 2) => (1.0, 2.5),
            (_, 3) => (2.5, 4.5),
            // Tiers are 1..=4; anything else falls back to the fist baseline.
            _ => FIST_DAMAGE,
        }
    }
}

/// Bare-hand (fist) melee damage — the baseline when nothing, or a non-weapon item, is
/// held. Deterministic: exactly 1 per hit (so a fist always takes 4 hits on 4 health).
pub const FIST_DAMAGE: (f32, f32) = (1.0, 1.0);

/// The melee damage range `(min, max)` for attacking with `item` in hand: the tool's
/// range if it's a weapon, else the [`FIST_DAMAGE`] baseline (an empty hand and a
/// non-weapon item like a block both punch for 1).
pub fn attack_damage(item: Option<ItemType>) -> (f32, f32) {
    item.and_then(ItemType::tool)
        .map(Tool::attack_damage)
        .unwrap_or(FIST_DAMAGE)
}
