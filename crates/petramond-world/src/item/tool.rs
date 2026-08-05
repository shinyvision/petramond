use super::ItemStack;

/// The `petramond:tool` INSTANCE-data key: a stack may carry a JSON override
/// of its row-resolved tool properties (`{"tier": u8, "speed": f32,
/// "damage": [min, max]}` — any subset; `kind` is deliberately not
/// overridable). Same vocabulary as the row's data entry, resolved by
/// [`ItemStack::tool`]: absent fields keep the row's values. This is how a
/// pack upgrades ONE tool in the world (the forge's diamond augments) without
/// minting an item row per combination.
///
/// Unlike row data — engine-parsed strictly at load — this arrives from mods
/// at runtime, so it parses LENIENTLY: malformed JSON or an invalid field
/// degrades to the row's value rather than erroring.
pub const TOOL_DATA_KEY: &str = "petramond:tool";

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

/// A mining tool. Its three material properties are INDEPENDENT, and that is
/// the whole shape of this type:
///
/// - `tier` gates what it may HARVEST at all.
/// - `speed` is how fast it gets there.
/// - `damage` is what it does to a mob.
///
/// One number cannot carry all three. A gold tool is the proof: it should
/// reach everything a pickaxe is for and still dig no faster than a bare fist,
/// which is unsayable while speed is a function of the harvest gate. So a row
/// may state each one, and anything it leaves out is derived from `tier` — the
/// shipped ladder is exactly what those defaults produce.
///
/// Read from an item via [`ItemType::tool`]; the mining model is
/// [`crate::mining`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Tool {
    pub kind: ToolKind,
    /// Harvest gate: the highest `harvest_tier` block this may break for its
    /// drop, when it is also the block's preferred kind.
    pub tier: u8,
    /// Mining speed as a multiplier over the bare hand, BEFORE the kind's
    /// [`ToolKind::mining_efficiency`].
    pub speed: f32,
    /// Melee damage range `(min, max)`; the attacker rolls uniformly in it.
    pub damage: (f32, f32),
}

/// The mining speed a `tier` implies when a row does not state one — the
/// shipped ladder.
pub fn default_speed(tier: u8) -> f32 {
    match tier {
        0 => 1.0,
        1 => 2.0,
        2 => 4.0,
        3 => 6.0,
        _ => 8.0,
    }
}

/// The melee damage a `(kind, tier)` implies when a row does not state one.
///
/// Axes hit hardest, shovels and pickaxes share a gentler curve, and every
/// diamond tool one-shots a small mob. The band (rather than a flat integer)
/// is what lets a tool's hits-to-kill read as e.g. "3-4 hits" on 4 health.
pub fn default_damage(kind: ToolKind, tier: u8) -> (f32, f32) {
    use ToolKind::*;
    if tier >= 4 {
        return (5.0, 7.0);
    }
    match (kind, tier) {
        (Axe, 1) => (1.5, 2.5),
        (Axe, 2) => (2.0, 3.0),
        (Axe, 3) => (4.0, 6.0),
        (_, 1) => (1.0, 1.5),
        (_, 2) => (1.0, 2.5),
        (_, 3) => (2.5, 4.5),
        _ => FIST_DAMAGE,
    }
}

/// The [`TOOL_DATA_KEY`] instance-data value — every field optional, unknown
/// fields ignored (a later engine may write more than this one reads).
#[derive(serde::Deserialize)]
struct RawToolOverride {
    #[serde(default)]
    tier: Option<u8>,
    #[serde(default)]
    speed: Option<f32>,
    #[serde(default)]
    damage: Option<[f32; 2]>,
}

impl Tool {
    /// This tool with a [`TOOL_DATA_KEY`] instance-data override merged over
    /// it. Lenient by doctrine (see the key's docs): unparseable bytes or an
    /// invalid field keep the row-resolved value, field by field.
    pub fn with_override(self, bytes: &[u8]) -> Tool {
        let Ok(raw) = serde_json::from_slice::<RawToolOverride>(bytes) else {
            return self;
        };
        let mut t = self;
        if let Some(tier) = raw.tier {
            t.tier = tier;
        }
        if let Some(speed) = raw.speed {
            if speed.is_finite() && speed > 0.0 {
                t.speed = speed;
            }
        }
        if let Some([lo, hi]) = raw.damage {
            if lo.is_finite() && hi.is_finite() && 0.0 <= lo && lo <= hi {
                t.damage = (lo, hi);
            }
        }
        t
    }

    /// A tool with the ladder's own speed and damage for its `(kind, tier)` —
    /// what a row that states nothing else resolves to.
    pub fn new(kind: ToolKind, tier: u8) -> Tool {
        Tool {
            kind,
            tier,
            speed: default_speed(tier),
            damage: default_damage(kind, tier),
        }
    }

    /// The melee damage range `(min, max)` this tool rolls per hit.
    pub fn attack_damage(self) -> (f32, f32) {
        self.damage
    }
}

/// Bare-hand (fist) melee damage — the baseline when nothing, or a non-weapon item, is
/// held. Deterministic: exactly 1 per hit (so a fist always takes 4 hits on 4 health).
pub const FIST_DAMAGE: (f32, f32) = (1.0, 1.0);

/// The melee damage range `(min, max)` for attacking with `stack` in hand: the
/// tool's range if it's a weapon (instance-data override included — see
/// [`ItemStack::tool`]), else the [`FIST_DAMAGE`] baseline (an empty hand and a
/// non-weapon item like a block both punch for 1).
pub fn attack_damage(stack: Option<&ItemStack>) -> (f32, f32) {
    stack
        .and_then(ItemStack::tool)
        .map(Tool::attack_damage)
        .unwrap_or(FIST_DAMAGE)
}
