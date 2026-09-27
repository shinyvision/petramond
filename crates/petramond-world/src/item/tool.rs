use super::ItemStack;

pub const TOOL_DATA_KEY: &str = "petramond:tool";

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Pickaxe,
    Axe,
    Shovel,
    Shears,
    Sword,
}

impl ToolKind {
    pub fn name(self) -> &'static str {
        match self {
            ToolKind::Pickaxe => "pickaxe",
            ToolKind::Axe => "axe",
            ToolKind::Shovel => "shovel",
            ToolKind::Shears => "shears",
            ToolKind::Sword => "sword",
        }
    }

    #[inline]
    pub fn mining_efficiency(self) -> f32 {
        match self {
            ToolKind::Pickaxe | ToolKind::Axe | ToolKind::Shears | ToolKind::Sword => 1.0,
            ToolKind::Shovel => 0.5625,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Tool {
    pub kind: ToolKind,
    pub tier: u8,
    pub speed: f32,
    pub damage: (f32, f32),
    pub knockback: f32,
}

pub const DEFAULT_KNOCKBACK: f32 = 1.0;

pub fn default_speed(tier: u8) -> f32 {
    match tier {
        0 => 1.0,
        1 => 2.0,
        2 => 4.0,
        3 => 6.0,
        _ => 8.0,
    }
}

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

#[derive(serde::Deserialize)]
struct RawToolOverride {
    #[serde(default)]
    tier: Option<u8>,
    #[serde(default)]
    speed: Option<f32>,
    #[serde(default)]
    damage: Option<[f32; 2]>,
    #[serde(default)]
    knockback: Option<f32>,
}

impl Tool {
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
        if let Some(k) = raw.knockback {
            if k.is_finite() && k >= 0.0 {
                t.knockback = k;
            }
        }
        t
    }

    pub fn new(kind: ToolKind, tier: u8) -> Tool {
        Tool {
            kind,
            tier,
            speed: default_speed(tier),
            damage: default_damage(kind, tier),
            knockback: DEFAULT_KNOCKBACK,
        }
    }

    pub fn attack_damage(self) -> (f32, f32) {
        self.damage
    }
}

pub const FIST_DAMAGE: (f32, f32) = (1.0, 1.0);

pub fn attack_damage(stack: Option<&ItemStack>) -> (f32, f32) {
    stack
        .and_then(ItemStack::tool)
        .map(Tool::attack_damage)
        .unwrap_or(FIST_DAMAGE)
}
