#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PlayerId(pub u8);

pub const PLAYER_HALF_W: f32 = 0.3;
pub const PLAYER_HEIGHT: f32 = 1.8;

pub const DEFAULT_DAY_CYCLE_TICKS: u64 = 15 * 60 * 20 * 2;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UseGesture {
    #[default]
    Free,
    Held(Box<str>),
    Spent,
}

impl UseGesture {
    pub fn held_by(&self, claimant: &str) -> bool {
        matches!(self, UseGesture::Held(o) if &**o == claimant)
    }

    pub fn is_free(&self) -> bool {
        *self == UseGesture::Free
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlayerInputSnapshot {
    pub id: u8,
    pub forward: f32,
    pub strafe: f32,
    pub jump: bool,
    pub sneak: bool,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerRosterSnapshot {
    pub id: u8,
    pub pos: [f64; 3],
    pub vel: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub health: i32,
    pub on_ground: bool,
    pub spectator: bool,
    pub sneak: bool,
    pub use_held: bool,
    pub held: Option<petramond_world::item::ItemType>,
    pub held_count: u8,
    pub use_gesture: UseGesture,
    pub off_held: Option<petramond_world::item::ItemType>,
    pub swing: mod_api::HandSwing,
    pub conditions: Vec<mod_api::ConditionData>,
    pub entombed: bool,
    pub name: String,
    pub operator: bool,
}
