//! The per-tick SESSION facts the server publishes on its world before the
//! tick stages run: every connected player's movement intent and state
//! snapshot, the read models behind the `PlayerInput` / `Players` HostCalls,
//! plus the per-world rule defaults the session installs.
//!
//! They are plain values the world stores and hands out; the player module
//! fills them from its sessions and re-exports them under its own names.

/// A connected player's session id: the small per-world slot byte that names a
/// player on the wire, in mob aggro/hearing, and in per-player server state.
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

/// Half the horizontal width of a player's body box (0.6 wide on x and z) —
/// what the world tests a published roster position against.
pub const PLAYER_HALF_W: f32 = 0.3;
/// Full height of a player's body box.
pub const PLAYER_HEIGHT: f32 = 1.8;

/// Day+night cycle length of a world whose settings name no day length: the
/// default 15-minute day plus an equally long night at 20 TPS. Session open
/// installs the per-world value (`ServerWorld::set_day_cycle_ticks`).
pub const DEFAULT_DAY_CYCLE_TICKS: u64 = 15 * 60 * 20 * 2;

/// One press of the interact button, from press to release — and who owns it.
///
/// Most interactions resolve inside a gesture and leave it FREE, which is what
/// lets a held button keep placing blocks or flapping a door. A CONTINUOUS one
/// — an eat, a raised guard — takes the gesture instead, and nothing else is
/// offered the button until it comes up.
///
/// Transient body state: never saved, and both mirrors run the same rules over
/// it from the same input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UseGesture {
    /// Nothing owns the button; the next press or repeat is offered.
    #[default]
    Free,
    /// Owned until release, by the named claimant
    /// (`player::ENGINE_CLAIMANT` for the engine's own eat).
    Held(Box<str>),
    /// It WAS owned, the interaction finished, and the button has not come up
    /// yet — so nothing else may take it until it does.
    ///
    /// This is the whole reason a held button does not eat a stack: finishing
    /// is not the same as letting go, and the player has to say so.
    Spent,
}

impl UseGesture {
    /// Whether `claimant` owns this gesture right now.
    pub fn held_by(&self, claimant: &str) -> bool {
        matches!(self, UseGesture::Held(o) if &**o == claimant)
    }

    /// Whether anything owns it — the gate that stops a fresh click or a
    /// repeat being offered to the chain.
    pub fn is_free(&self) -> bool {
        *self == UseGesture::Free
    }
}

/// One player's movement intent for the current tick, decomposed into the
/// player's OWN yaw frame and published on the world (`ServerWorld::set_player_inputs`)
/// by the server before the tick stages run — the read model behind the
/// `PlayerInput` HostCall, so mods (vehicles, mounts, machines a player
/// stands on) can react to what a player is pressing without touching the
/// world-space wish plumbing. Derived from the same session intent latches
/// `tick_movement` integrates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlayerInputSnapshot {
    /// Session player id.
    pub id: u8,
    /// Forward(+)/back(−) component of the wish direction along the player's
    /// facing, in `[-1, 1]`.
    pub forward: f32,
    /// Right(+)/left(−) strafe component, in `[-1, 1]`.
    pub strafe: f32,
    pub jump: bool,
    pub sneak: bool,
    /// The player's look, for mods that steer by it.
    pub yaw: f32,
    pub pitch: f32,
}

/// One connected player's per-tick state snapshot, published on the world
/// beside the inputs — the read model behind the `Players` HostCall
/// (multiplayer-aware spawn/ambience/weather policy).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerRosterSnapshot {
    /// Session player id.
    pub id: u8,
    /// Feet position.
    pub pos: [f64; 3],
    pub vel: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// Half-heart points.
    pub health: i32,
    pub on_ground: bool,
    pub spectator: bool,
    /// Sneak intent, gated on gameplay focus (the session's one sneak rule).
    pub sneak: bool,
    /// Use (interact) intent, gated on gameplay focus — published beside sneak
    /// so continuous-use predicates read the snapshot every other actor
    /// context lives on.
    pub use_held: bool,
    /// The selected hotbar stack's item, if any, with its count.
    pub held: Option<petramond_world::item::ItemType>,
    pub held_count: u8,
    /// Who owns this body's interact press — the roster's copy of
    /// player's `use_gesture`, so a reader answers "is this press mine"
    /// for any session, not just the acting one.
    pub use_gesture: UseGesture,
    /// The off-hand slot's item, if any — the literal off slot (the roster is
    /// outside any acting-hand dispatch), so a reader sees both hands.
    pub off_held: Option<petramond_world::item::ItemType>,
    /// The swing facts a hand-animating mod keys off (the mod ABI's
    /// `PlayerSnapshot::swing`): the mining level as of this roster, plus
    /// the one-shots the stages latched during the tick that just ran —
    /// published on exactly one roster each, so a mod's tick system sees an
    /// edge once, one tick after it fired (which the eased pose lane hides).
    pub swing: mod_api::HandSwing,
    /// Active body conditions, the ABI view.
    pub conditions: Vec<mod_api::ConditionData>,
    /// Whether the body is entombed (see `Player::entombed`).
    pub entombed: bool,
    /// The player's stable name (the save key), for state a mod keeps past
    /// this session.
    pub name: String,
    /// Whether the player is a server operator.
    pub operator: bool,
}
