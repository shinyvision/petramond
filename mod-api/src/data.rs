//! Payload structs and small vocabularies shared by calls and replies.

use serde::{Deserialize, Serialize};

use crate::ids::{BlockId, ItemId, MobId, PlayerId};

/// Maximum UTF-8 byte length of a named mob animation crossing the mod API.
/// The simulation stores and replicates active names, so the mechanism bounds
/// them independently of whether the mob's model recognizes the name.
pub const MAX_MOB_ANIM_NAME_BYTES: usize = 64;

/// Largest absolute named-animation phase accepted from a mod, in authored
/// animation seconds.
pub const MAX_MOB_ANIM_PHASE_MAGNITUDE: f32 = 1_000_000.0;

/// Largest absolute named-animation playback/seek rate accepted from a mod,
/// in authored animation seconds per real second.
pub const MAX_MOB_ANIM_RATE_MAGNITUDE: f32 = 1_000.0;

/// One value of the open GUI session's state map. Written by mods
/// on the tick ([`HostCall::GuiStateSet`]); read per frame by the renderer to
/// drive `label` text, `rotimage` angles (radians, `F32`), and mod overlay
/// fractions. Keys are mod-local: the map belongs to one GUI session (cleared
/// on open/close), so no namespace prefix is enforced.
///
/// [`HostCall::GuiStateSet`]: crate::HostCall::GuiStateSet
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GuiValue {
    F32(f32),
    I32(i32),
    Str(String),
}

/// One value in a live mob's tag map. Engine tags use the `petramond:`
/// namespace (e.g., `petramond:confined`); mods may invent `mod_id:` keys.
/// Tags persist with the mob and are visible to AI.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MobTagValue {
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
}

/// The outcome of [`HostCall::MobTagGet`](crate::HostCall::MobTagGet): a mob
/// that is GONE (dead, unloaded, never spawned) is told apart from a live mob
/// simply not carrying the key — the two mean different things to a mod
/// (retry vs. store), so they are never conflated.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MobTagLookup {
    /// No such LIVE mob (dead, unloaded, or never existed).
    MissingMob,
    /// The mob is live but carries nothing under the key.
    Absent,
    /// The mob carries this value under the key.
    Value(MobTagValue),
}

/// A live mob's snapshot for [`HostCall::MobsInRadius`] /
/// [`HostCall::MobsWithTag`]. The mob's ADDRESS is the stable
/// [`id`](Self::id) — every mob call and event payload speaks it
/// (see the mob-addressing note on [`HostCall`](crate::HostCall)). `index` is
/// only an intra-tick JOIN key against other snapshots taken this tick; it is
/// never accepted by a call and renumbers on any removal.
///
/// [`HostCall::MobsInRadius`]: crate::HostCall::MobsInRadius
/// [`HostCall::MobsWithTag`]: crate::HostCall::MobsWithTag
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MobSnapshot {
    /// Live-set list position THIS TICK — an intra-tick join key only, never
    /// an address (calls take [`id`](Self::id)).
    pub index: u32,
    /// The species' session id, matching the `kind` in event payloads
    /// ([`EventPayload::MobDied`] etc.). Deliberately the ONLY species field:
    /// a snapshot carries no `"pack:species"` string, because a crowd query
    /// answers dozens of snapshots per tick and a heap string per mob is the
    /// most expensive thing in the whole marshalling. Resolve a key ONCE with
    /// [`HostCall::ResolveMob`] (or [`HostCall::MobNames`] for the reverse)
    /// and compare ids.
    ///
    /// [`EventPayload::MobDied`]: crate::EventPayload::MobDied
    /// [`HostCall::ResolveMob`]: crate::HostCall::ResolveMob
    /// [`HostCall::MobNames`]: crate::HostCall::MobNames
    pub kind: MobId,
    /// Feet position.
    pub pos: [f32; 3],
    pub health: f32,
    /// Stable session id for this live mob — THE mob address, held across
    /// ticks. It survives unrelated removals; it is not a species id and is
    /// not promised stable across save/load.
    pub id: u64,
    /// Body facing, radians about +Y. MOB convention: yaw `0` faces `-Z`,
    /// so the facing direction is `(-sin yaw, 0, -cos yaw)` — the same frame
    /// [`HostCall::MobDrive`] yaws speak.
    ///
    /// [`HostCall::MobDrive`]: crate::HostCall::MobDrive
    pub yaw: f32,
    /// Body pitch, radians about the lateral axis inside the yaw, positive =
    /// nose up. `0` for every body the engine moves itself; a body a mod
    /// authors through [`HostCall::MobKinematic`] reads back what it was
    /// given, and a released body eases back to level.
    ///
    /// [`HostCall::MobKinematic`]: crate::HostCall::MobKinematic
    pub pitch: f32,
    /// Body roll, radians about the facing axis inside the yaw and pitch,
    /// positive = right side up. Level and authored exactly like `pitch`.
    pub roll: f32,
    /// Current velocity (m/s). Read-only; steer through
    /// [`HostCall::MobDrive`].
    ///
    /// [`HostCall::MobDrive`]: crate::HostCall::MobDrive
    pub vel: [f32; 3],
    /// Whether the body rests on the ground this tick (the same fact the
    /// engine's own locomotion gates jumps on) — with
    /// [`moving`](Self::moving), what a gait policy needs to decide a
    /// [`HostCall::MobDrive`] launch.
    ///
    /// [`HostCall::MobDrive`]: crate::HostCall::MobDrive
    pub on_ground: bool,
    /// Whether the brain's WALKING locomotion drove the body this tick — the
    /// same fact that selects the walk pose. Deliberate motion only: shoves
    /// from other bodies, knockback flights, and kinematic drives all read
    /// `false`, while a ballistic arc that began as a walk stays `true`
    /// through its unsteered descent. THE intent signal for a gait policy:
    /// [`vel`](Self::vel) alone cannot distinguish a mob going somewhere
    /// from a mob being pushed around.
    pub moving: bool,
    /// Body extents — the same envelope the engine's collision, targeting
    /// and riding use: a box `half_width` either side of the feet position,
    /// `height` tall from the feet up. A LONG body (a hull) also has a
    /// `half_length` ALONG its facing: it occupies a run of `half_width`
    /// squares whose centres span `±(half_length - half_width)` along
    /// `(-sin yaw, 0, -cos yaw)`. Square bodies answer `half_length ==
    /// half_width`.
    pub half_width: f32,
    pub height: f32,
    pub half_length: f32,
}

/// One item entity's snapshot ([`HostCall::ItemEntity`]): a stack loose in
/// the world, in flight, or lodged in a block.
///
/// [`HostCall::ItemEntity`]: crate::HostCall::ItemEntity
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ItemEntityData {
    /// Stable session id — THE item-entity address (event payloads, calls).
    pub id: u64,
    /// What it is: item, count, instance data.
    pub stack: ItemStackData,
    /// Who launched it ([`HostCall::LaunchItem`]), while it is in flight;
    /// `None` for a drop, or once it has come to rest.
    ///
    /// [`HostCall::LaunchItem`]: crate::HostCall::LaunchItem
    pub owner: Option<EntityRef>,
    /// Centre, world space.
    pub pos: [f32; 3],
    /// Velocity, m/s (zero once lodged).
    pub vel: [f32; 3],
    pub motion: ItemMotion,
}

/// How an item entity is moving ([`ItemEntityData::motion`]).
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemMotion {
    /// An ordinary drop: falling, settling, drifting to a reaching player.
    Loose,
    /// Launched and flying ([`HostCall::LaunchItem`]): pointed along its
    /// velocity, striking what it meets.
    ///
    /// [`HostCall::LaunchItem`]: crate::HostCall::LaunchItem
    Flight,
    /// Lodged in `cell`, heading kept, until that block goes.
    Stuck { cell: [i32; 3] },
}

/// A living thing a call can name as the ACTOR behind something — the
/// attacker a damage request is landed on behalf of
/// ([`HostCall::DamageMob`], [`HostCall::DamagePlayer`]). Players by
/// session id, mobs by their stable id.
///
/// [`HostCall::DamageMob`]: crate::HostCall::DamageMob
/// [`HostCall::DamagePlayer`]: crate::HostCall::DamagePlayer
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EntityRef {
    Player(PlayerId),
    Mob(u64),
}

/// What stops a [`HostCall::Raycast`].
///
/// [`HostCall::Raycast`]: crate::HostCall::Raycast
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum RayFilter {
    /// What the crosshair selects: every block with a selection shape —
    /// solids, sub-cell shapes by their real geometry, plants by their
    /// selection box. Air and water pass.
    Selectable,
    /// What a body collides with: only cells holding collision boxes, tested
    /// by their shape. Plants, snow layers, water and decorative
    /// no-collision models pass — the ray a swung tool or a projectile
    /// follows.
    Collidable,
}

/// One [`HostCall::Raycast`] hit.
///
/// [`HostCall::Raycast`]: crate::HostCall::Raycast
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct RaycastHitData {
    /// The cell the ray stopped in.
    pub block: [i32; 3],
    /// The crossed face's normal, pointing back toward the ray's origin
    /// (zero when the origin started inside the cell).
    pub face: [i32; 3],
    /// Distance from the origin to the hit, in blocks.
    pub distance: f32,
}

/// What a player is attached to, for mount HostCalls and
/// [`EventPayload::PlayerDismounted`].
///
/// [`EventPayload::PlayerDismounted`]: crate::EventPayload::PlayerDismounted
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub enum MountTarget {
    /// Live mob, addressed by its stable session id.
    Mob(u64),
    /// A static world-space pose anchor ([`HostCall::PlayerPoseSet`]) — the
    /// anchor position the pose was pinned at.
    ///
    /// [`HostCall::PlayerPoseSet`]: crate::HostCall::PlayerPoseSet
    Anchor([f32; 3]),
}

/// Named actor-pose vocabulary for [`HostCall::PlayerPoseSet`] (`0` is
/// reserved). Unknown values pin the body in its ordinary rest pose — like a
/// disabled pack, never an error.
///
/// [`HostCall::PlayerPoseSet`]: crate::HostCall::PlayerPoseSet
pub mod pose {
    /// Seated: thighs forward, shins down — chairs, benches, sofas.
    pub const SITTING: u8 = 1;
}

/// One rider of a mount, for [`HostCall::MobRiders`].
///
/// [`HostCall::MobRiders`]: crate::HostCall::MobRiders
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct MobRiderData {
    /// Seat index into the mount's declared `seats` list.
    pub seat: u8,
    /// The riding session.
    pub player_id: PlayerId,
}

/// Seat declaration and current occupants of one mount, for
/// [`HostCall::MobRiders`].
///
/// [`HostCall::MobRiders`]: crate::HostCall::MobRiders
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MobRidersData {
    /// Number of seats declared by the mount's row. Valid seat indices
    /// are `0..capacity`.
    pub capacity: u8,
    /// Current occupants, in player-id order.
    pub riders: Vec<MobRiderData>,
}

impl MobRidersData {
    /// The lowest declared seat index nobody occupies, or `None` when the
    /// mount is full (or declares no seats) — the shared boarding pick.
    pub fn first_free_seat(&self) -> Option<u8> {
        (0..self.capacity).find(|s| !self.riders.iter().any(|r| r.seat == *s))
    }
}

/// A placed model-block group's world placement, for
/// [`HostCall::BlockModelGroup`] — everything block-local policy (a seat
/// layout, a machine front) needs to map its own footprint-space data into
/// the world.
///
/// [`HostCall::BlockModelGroup`]: crate::HostCall::BlockModelGroup
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct ModelGroupData {
    /// The group's BASE cell (the rotated footprint's min corner).
    pub base: [i32; 3],
    /// The placement facing the group was placed with.
    pub facing: crate::Facing,
}

/// Authoritative playback state of one active named mob animation, for
/// [`HostCall::MobAnimState`].
///
/// [`HostCall::MobAnimState`]: crate::HostCall::MobAnimState
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct MobAnimStateData {
    /// Absolute authored-animation phase in seconds.
    pub phase: f32,
    /// Current playback rate. While seeking this is the non-negative approach
    /// rate; after landing it is `0`.
    pub rate: f32,
    /// Absolute seek target, or `None` during ordinary rate-driven playback.
    pub seek: Option<f32>,
}

/// One player's movement intent this tick, for [`HostCall::PlayerInput`] —
/// decomposed into the player's own yaw frame so a driving mod never touches
/// the world-space wish plumbing.
///
/// [`HostCall::PlayerInput`]: crate::HostCall::PlayerInput
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct PlayerInputData {
    /// Forward(+)/back(−) along the player's facing, `[-1, 1]`.
    pub forward: f32,
    /// Right(+)/left(−) strafe, `[-1, 1]`.
    pub strafe: f32,
    pub jump: bool,
    pub sneak: bool,
    /// The player's look. PLAYER convention: yaw `0` faces `+Z` (facing
    /// `(sin yaw, 0, cos yaw)`) — π apart from the mob yaw convention; a mod
    /// aligning a mount to its rider adds π.
    pub yaw: f32,
    pub pitch: f32,
}

/// One extra Blockbench DISPLAY TRANSFORM on top of an item's authored hold —
/// same units and axis order as the `display` block, so a pose tuned in the
/// modelling tool transfers digit-for-digit.
///
/// Composed OUTSIDE the authored transform (`offset · authored`), so the
/// translation moves the item within the hold frame rather than along its own
/// tilted axes. [`IDENTITY`](Self::IDENTITY) means the same as no pose at all.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Default)]
pub struct HeldPoseData {
    /// Rotation in DEGREES about X, then Y, then Z — the `display` block's
    /// convention.
    pub rotation: [f32; 3],
    /// Translation in 1/16-BLOCK pixels — the `display` block's convention.
    pub translation: [f32; 3],
}

impl HeldPoseData {
    /// The neutral offset: the authored hold, unchanged.
    pub const IDENTITY: Self = Self {
        rotation: [0.0; 3],
        translation: [0.0; 3],
    };

    /// Whether this offset changes nothing.
    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// Every component finite. A NaN would poison every transform downstream
    /// of the hand, so the engine refuses one outright.
    pub fn is_finite(&self) -> bool {
        self.rotation
            .iter()
            .chain(&self.translation)
            .all(|c| c.is_finite())
    }
}

/// One hand's held-item pose for [`HostCall::SetPlayerHeldPose`]: an offset
/// per VIEW, because the two views hold an item from different authored poses
/// (`firstperson_righthand` vs `thirdperson_righthand`), so the same intent is
/// a different delta in each.
///
/// The OFF hand is never authored separately — the engine mirrors by the rule
/// Blockbench applies to a left-hand slot (negate the x-translation and the
/// y/z rotations).
///
/// [`HostCall::SetPlayerHeldPose`]: crate::HostCall::SetPlayerHeldPose
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Default)]
pub struct HeldPose {
    /// Composed onto the item's `firstperson_*hand` hold — the wielder's own
    /// screen.
    pub first_person: HeldPoseData,
    /// Composed onto the item's `thirdperson_*hand` hold — the body every
    /// observer sees (and the wielder's own, in third person).
    pub third_person: HeldPoseData,
}

impl HeldPose {
    /// The neutral pose: both views keep their authored hold.
    pub const IDENTITY: Self = Self {
        first_person: HeldPoseData::IDENTITY,
        third_person: HeldPoseData::IDENTITY,
    };

    /// Whether this pose changes nothing in either view.
    pub fn is_identity(&self) -> bool {
        self.first_person.is_identity() && self.third_person.is_identity()
    }

    /// Every component of both views finite.
    pub fn is_finite(&self) -> bool {
        self.first_person.is_finite() && self.third_person.is_finite()
    }
}

/// How a [`BonePoseData`] meets the animation already posing its bone.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum BonePoseMode {
    /// COMPOSE onto whatever the animation put the bone at, so the offset
    /// layers over the motion — a nudge that still walks, sneaks and swings.
    #[default]
    Compose,
    /// REPLACE the animation's own posing of this bone: HELD at the rig's rest
    /// pose plus this rotation, whatever the walk cycle wanted.
    ///
    /// What a STANCE needs — an arm raised mid-stride must not also swing, and
    /// composing cannot express that because the swing is still underneath.
    /// Descendants keep their own animation relative to the held bone, so
    /// holding a shoulder does NOT freeze the elbow: a stance must hold every
    /// joint it owns.
    Replace,
}

/// One bone's pose: an offset composed onto the engine's own animation, or a
/// stance that replaces it — see [`BonePoseMode`].
///
/// The rotation is about the bone's PIVOT and carries through every descendant,
/// which is what makes "rotate the shoulder" move the whole arm and the thing
/// in its fist.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct BonePoseData {
    /// The rig bone to pose — see the [`bone`](crate::bone) names. A name
    /// the rig does not have is ignored, like a disabled pack.
    pub bone: String,
    /// Rotation in DEGREES about the bone's pivot, applied X then Y then Z —
    /// the same convention as a `.bbmodel`'s own rotations, so a pose posed
    /// in Blockbench transfers digit for digit.
    pub rotation: [f32; 3],
    /// Translation in 1/16-BLOCK pixels, in the bone's frame.
    pub translation: [f32; 3],
    /// Whether this pose layers over the animation or replaces it.
    pub mode: BonePoseMode,
}

impl BonePoseData {
    /// Every component finite; a NaN would poison every transform below the
    /// bone, so the engine refuses the list whole. The NAME is not validated
    /// here — an unknown one resolves to nothing and is dropped.
    pub fn is_finite(&self) -> bool {
        self.rotation
            .iter()
            .chain(&self.translation)
            .all(|c| c.is_finite())
    }
}

/// The player rig's bone names, for [`HostCall::SetPlayerBonePose`]. Any
/// authored name works; these are the shipped rig's, named by intent rather
/// than by how the model file spells them.
///
/// THAT MATTERS FOR THE ARMS: the rig authors the MAIN hand's arm as the
/// model's LEFT (the body faces engine-forward, which swaps the sides you
/// see), so reaching for `"right_shoulder"` by intuition moves the wrong arm.
///
/// [`HostCall::SetPlayerBonePose`]: crate::HostCall::SetPlayerBonePose
pub mod bone {
    /// The head — rotating it composes with, and is overridden by, the
    /// engine's own head-look when an animation is not driving the head.
    pub const HEAD: &str = "head";
    /// The upper body (chest + arms + head). Rotating it turns everything
    /// above the waist.
    pub const BODY: &str = "body";
    /// Everything above the legs.
    pub const WAIST: &str = "waist";

    /// The MAIN hand's whole arm, from the shoulder joint down.
    pub const MAIN_SHOULDER: &str = "left_shoulder";
    /// The MAIN hand's forearm, from the elbow down.
    pub const MAIN_ELBOW: &str = "left_elbow";
    /// The OFF hand's whole arm, from the shoulder joint down.
    pub const OFF_SHOULDER: &str = "right_shoulder";
    /// The OFF hand's forearm, from the elbow down.
    pub const OFF_ELBOW: &str = "right_elbow";
}

/// One thing a body does with its hands, and can be barred from doing
/// ([`HostCall::SetPlayerDeniedActions`]).
///
/// These are the three gates a player's own buttons drive, and they stay
/// separate because they are separate gates: a body that can still mine but
/// not fight is a reasonable thing to want.
///
/// Barring an action stops the ACTION, never the intent behind it. A body
/// denied [`Use`](Self::Use) still has its use button read as held, which is
/// what lets the same press that raises a guard be the press the guard
/// swallows.
///
/// [`HostCall::SetPlayerDeniedActions`]: crate::HostCall::SetPlayerDeniedActions
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BodyAction {
    /// Swing at a mob, another player, or the air.
    Attack,
    /// Break blocks — both the held-button timer and a client's claimed finish.
    Mine,
    /// Interact: the whole use dispatch — placing, doors and containers, an
    /// item's own use, starting an eat — and the hold-repeat that re-runs it.
    Use,
}

/// One of a body's engine QUANTITIES a claim can scale
/// ([`HostCall::SetPlayerAttribute`]): the engine keeps the base — a
/// constant, a mode, a formula — and the resolved claims multiply it.
///
/// The vocabulary is engine-defined and grows by appending a variant at the
/// engine quantity that wants a knob; a claim is always a MULTIPLIER
/// (product across claimants, `1.0` releases), never an absolute, so any
/// two packs' claims compose without an order to argue about.
///
/// [`HostCall::SetPlayerAttribute`]: crate::HostCall::SetPlayerAttribute
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PlayerAttribute {
    /// The land-speed multiplier: scales whatever mode the player's own
    /// input selected (walk, sprint and sneak together; swim, climb and
    /// flight untouched). The engine's own claim — the speed-carrying
    /// status effects — multiplies in beside yours.
    MoveSpeed,
    /// The cooldown between primary-button attack swings (the engine's
    /// melee rate limit). `0.0` removes it — for a pack whose own pacing
    /// already gates the hand (a swing-claim animation barring attacks
    /// mid-arc), so the ANIMATION becomes the attack pace instead of a
    /// constant the animation has to chase.
    AttackCooldown,
}

impl PlayerAttribute {
    /// Every attribute, in declaration order — the index space claim
    /// storage sizes itself on. A new variant is appended here too.
    pub const ALL: [PlayerAttribute; 2] = [Self::MoveSpeed, Self::AttackCooldown];

    /// This attribute's slot in [`Self::ALL`]: the declaration order, which
    /// is also its wire discriminant.
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// One kind of one-shot hand action the engine latches per tick (client:
/// per frame) — the raw gesture that fired, so a body-posing mod keys its
/// own curve off the same trigger and off nothing pre-interpreted. The
/// engine's own animation collapses these into two motions (the
/// [`HandMotion`] vocabulary): `Attack`/`Break` play the full swing, the
/// rest the softer jab.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SwingKind {
    /// An attack swing — a mob, another player, or a punch at the air.
    Attack,
    /// A block broke under this hand's mining timer.
    Break,
    /// A block placement.
    Place,
    /// A throw or drop left this hand.
    Throw,
    /// A use click that something consumed: a screen, a door, a bed, an
    /// item use.
    Interact,
}

/// One of the engine's OWN hand motions a claim can silence
/// ([`HostCall::SetPlayerHandMotions`]): the vocabulary of what the engine
/// animates on a hand by itself, so a claimant names exactly the motions it
/// takes over and the engine keeps playing the rest.
///
/// [`HostCall::SetPlayerHandMotions`]: crate::HostCall::SetPlayerHandMotions
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum HandMotion {
    /// The full-strength swing family: the mining loop and the
    /// break/attack punches.
    Swing,
    /// The soft use jab (a [`SwingKind::Place`]/[`SwingKind::Throw`]/
    /// [`SwingKind::Interact`] edge's motion).
    Jab,
}

/// What a body's hands are doing with the PRIMARY button this tick (client:
/// frame), as [`HostCall::PlayerState`] / [`HostCall::Players`] publish it —
/// the raw swing facts a body-posing mod animates from when it claims the
/// hands via [`HostCall::SetPlayerHandMotions`].
///
/// Deliberately raw TRIGGERS, not a phase: each side runs its own clock off
/// them (ticks on the server, frame seconds on the client) exactly as the
/// recoil-cue pattern does. The mining level is a LEVEL (the held button is
/// working a block), the one-shots are edges the newest wins.
///
/// [`HostCall::PlayerState`]: crate::HostCall::PlayerState
/// [`HostCall::Players`]: crate::HostCall::Players
/// [`HostCall::SetPlayerHandMotions`]: crate::HostCall::SetPlayerHandMotions
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct HandSwing {
    /// The MAIN hand is mid-mine (held button on a block, timer running).
    pub mining: bool,
    /// The main hand's one-shot this tick, if it fired one.
    pub main: Option<SwingKind>,
    /// The off hand's one-shot (its place jab), if it fired one.
    pub off: Option<SwingKind>,
}

/// The player's state for [`HostCall::PlayerState`].
///
/// [`HostCall::PlayerState`]: crate::HostCall::PlayerState
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerSnapshot {
    /// WHOSE state this is — the identity every player-addressed HostCall
    /// takes, so a handler acts on the player it was handed instead of
    /// guessing. `None` only where the dispatch has no session behind it (mod
    /// init, unit fixtures). On a CLIENT instance, always the local player.
    pub id: Option<PlayerId>,
    /// Feet position.
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    /// Look direction, radians (yaw about +Y, pitch clamped short of vertical).
    pub yaw: f32,
    pub pitch: f32,
    /// Half-heart points (`0..=20`).
    pub health: i32,
    pub on_ground: bool,
    pub spectator: bool,
    /// Whether the player is sneaking (held intent, gated on gameplay focus).
    /// Part of the snapshot so anything consuming an
    /// [`EventPayload::InteractAttempt`] gates its own claim on the actor's
    /// state instead of reconstructing input from the roster.
    ///
    /// [`EventPayload::InteractAttempt`]: crate::EventPayload::InteractAttempt
    pub sneak: bool,
    /// The selected hotbar stack's item (`None` = empty hand); bridge with
    /// [`HostCall::ItemNames`] / [`HostCall::ResolveItem`].
    ///
    /// [`HostCall::ItemNames`]: crate::HostCall::ItemNames
    /// [`HostCall::ResolveItem`]: crate::HostCall::ResolveItem
    pub held: Option<ItemId>,
    /// The OFF-HAND slot's item (`None` = empty). Always literally that slot,
    /// unlike [`held`](Self::held), which resolves the ACTING hand during a use
    /// click's second pass — so a consumer can see both hands at once.
    pub off_held: Option<ItemId>,
    /// Whether the interact (use) button is HELD, gated on gameplay focus like
    /// [`sneak`](Self::sneak). The unconditional intent, for continuous-use
    /// predicates: a held button also re-fires
    /// [`EventPayload::InteractAttempt`] every few ticks, but only at whatever
    /// the crosshair holds.
    ///
    /// [`EventPayload::InteractAttempt`]: crate::EventPayload::InteractAttempt
    pub use_held: bool,
    /// Whether THIS caller holds the actor's current use gesture — took the
    /// press ([`HostCall::HoldUse`]) and has not seen the button come up.
    ///
    /// The predicate a CONTINUOUS use is written against, in place of
    /// [`use_held`](Self::use_held): a raw held button says nothing about
    /// whether the press was yours, so a pack keying off it starts its own
    /// interaction on top of whatever the click actually did.
    ///
    /// [`HostCall::HoldUse`]: crate::HostCall::HoldUse
    pub holds_use: bool,
    /// The selected stack's count (0 = empty hand) — lets a consumer gate an
    /// atomic multi-item spend (the trough's three-wheat fill) exactly.
    pub held_count: u8,
    /// The world-space anchor this player is pose-pinned at
    /// ([`HostCall::PlayerPoseSet`]), or `None` when not posed. THE occupancy
    /// read model for static seats: a consumer derives "is this seat taken"
    /// by comparing its own seat anchors against the roster — the engine's
    /// registry is always truth, so there is no mod-side bookkeeping to
    /// desync. Anchors round-trip verbatim (`f32` bit-exact), so exact
    /// equality against the anchor a mod passed is sound.
    ///
    /// [`HostCall::PlayerPoseSet`]: crate::HostCall::PlayerPoseSet
    pub pose_anchor: Option<[f32; 3]>,
    /// What this body's hands did with the action buttons this tick (client:
    /// this frame) — the swing facts a hand-animating mod keys its curves
    /// off. See [`HostCall::SetPlayerHandMotions`], the matching ownership
    /// claim.
    ///
    /// [`HostCall::SetPlayerHandMotions`]: crate::HostCall::SetPlayerHandMotions
    pub swing: HandSwing,
    /// Body extents, the same envelope the engine collides and targets:
    /// a box `half_width` either side of the feet, `height` tall, with the
    /// eye `eye_height` above the feet (where this body's look ray starts).
    pub half_width: f32,
    pub height: f32,
    pub eye_height: f32,
}

/// One entry of [`HostCall::Players`]: a connected player's session id plus
/// their state snapshot.
///
/// [`HostCall::Players`]: crate::HostCall::Players
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerListEntry {
    /// The session's player id — the value per-player calls
    /// (`PlayerInput`, `MobMount`) address.
    pub id: PlayerId,
    pub state: PlayerSnapshot,
}

/// One session with a mod GUI open, as [`HostCall::GuiViewers`] reports it.
///
/// `anchor` is the cell the session was opened on — the SAME cell a machine is
/// keyed at (its container anchor), so matching a viewer to one of your placed
/// machines is an equality test, not a search. `None` for a GUI with no block
/// behind it (a station, a programmatic `GuiOpen`).
///
/// [`HostCall::GuiViewers`]: crate::HostCall::GuiViewers
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GuiViewerData {
    pub player_id: PlayerId,
    /// The GUI kind key (`mod_id:name`) this session has open.
    pub kind: String,
    pub anchor: Option<[i32; 3]>,
}

/// One core-selected candidate for programmatic hostile spawning. The engine
/// owns physical site selection; registered hostile spawners decide whether a
/// specific hostile species admits this site.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HostileSpawnCandidate {
    /// Feet position, centered in the candidate cell.
    pub pos: [f32; 3],
    /// Feet cell.
    pub cell: [i32; 3],
    /// Cached light channels on the 6-bit `0..=63` scale.
    pub combined_light: u8,
    pub sky_light: u8,
    pub block_light: u8,
    /// Distance (blocks) from this site to the NEAREST connected player — the
    /// multiplayer-correct input for proximity spawn rules (the host-session
    /// `PlayerState` snapshot only sees one player).
    pub nearest_player_dist: f32,
}

/// Which isolated runtime instance is executing this module. Server and
/// worldgen instances are deterministic simulation runtimes; `Client` is a
/// presentation-only instance with read-only replica queries and sandboxed
/// client storage.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum RuntimeSide {
    Server,
    Worldgen,
    Client,
}

/// One thing a mod draws on a placed block, in the BLOCK'S OWN SPACE.
///
/// For a MODEL block that is its FOOTPRINT space — the coordinates its
/// `.bbmodel` is authored in (16 authored px = 1.0), origin at the footprint
/// base and turned by the placed facing — so `[0,0,0]` is the same corner
/// whichever of its cells you addressed, and geometry computed against the
/// model in Blockbench lands right at every placement. For anything else it is
/// the cell, `0..1`.
///
/// This is the primitive under any presentation a mod SIMULATES rather than
/// stages: liquid in a channel, a needle on a dial, a part sliding. The set is
/// retained and replaced wholesale, redrawn every frame from the replica, and
/// costs no re-mesh — which is what makes it usable at tick rate, unlike a
/// block-row swap or a parts mask.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum DrawPrim {
    /// An axis-aligned box wearing an atlas TILE (the same names a block row's
    /// `tiles` use). `tint` multiplies it; `emissive` lifts it out of the
    /// cell's light so molten metal glows in a dark forge.
    ///
    /// The tile maps over the box's faces in CELL units, so a face longer than
    /// one block along either of its axes gets the tile STRETCHED rather than
    /// repeated — a prim is a machine's moving part, not a wall. Split a long
    /// run into per-cell boxes if you want the texture to tile.
    Cuboid {
        min: [f32; 3],
        max: [f32; 3],
        tile: String,
        tint: [u8; 3],
        emissive: bool,
    },
    /// An ITEM, drawn the way the game draws that item everywhere else — its
    /// sprite extruded, or its bbmodel — so a mould in a basin and the mould
    /// in your hand cannot drift apart when the art changes. `scale` is in
    /// block units (1.0 = one block wide).
    ///
    /// `pitch` (about +X) is applied BEFORE `yaw` (about +Y), both in radians.
    /// Pitch is not decoration: a sprite item is a VERTICAL slab, so laying
    /// one flat in a basin is `pitch = FRAC_PI_2` and nothing else will do it.
    Item {
        at: [f32; 3],
        scale: f32,
        yaw: f32,
        pitch: f32,
        item: String,
        tint: [u8; 3],
    },
}

/// One item stack crossing the ABI: the item's registry NAME (the one
/// mod-facing item identity — see the identity note on
/// [`HostCall`](crate::HostCall)) + count + per-stack instance data.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ItemStackData {
    /// Registry name (`"petramond:coal"`, `"kitchen:raw_mutton"`).
    pub item: String,
    pub count: u8,
    /// The stack's instance data: namespaced key → small opaque value, sorted
    /// by key, empty = plain stack (the ordinary case). Stacks merge only on
    /// byte-identical data; caps are tight (≤4 keys, ≤64-byte values) and an
    /// over-cap or malformed map on a write call is a HARD error (mod bug).
    /// Like registry row data, ANY namespaced consumer key may be attached —
    /// describing an item in another system's vocabulary is the interop point.
    pub data: Vec<(String, Vec<u8>)>,
}

/// One item's registry row (see [`HostCall::ItemInfo`]) — the stable,
/// mod-relevant fields of its `items.json` row, the same data engine
/// mechanics read. Presentation internals (sprite/model/held pose) stay
/// engine-side. Session-stable: cache it mod-side, never re-ask per tick.
///
/// [`HostCall::ItemInfo`]: crate::HostCall::ItemInfo
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ItemInfoData {
    /// Effective per-slot stack cap (durable items — tools — never stack).
    pub max_stack: u8,
    /// Fuel burn duration in game ticks; `0` = not a fuel. Any machine may
    /// consume it (the furnace reads exactly this field).
    pub fuel_burn_ticks: u32,
    /// The item's tag names (engine tags bare, pack tags namespaced).
    pub tags: Vec<String>,
    /// Human-readable display name (UI text only — never an identity).
    pub display_name: String,
    /// Session id of the block this item places (the row's `block` link), or
    /// `None` for an item-only item (tools, raw drops, ingots). Compare
    /// against `get_block` reads; resolve a name via `BlockNames`.
    pub block: Option<BlockId>,
    /// The mining tool this item acts as, or `None`.
    pub tool: Option<ToolInfoData>,
    /// Edible-item data, or `None` for non-food.
    pub food: Option<FoodInfoData>,
    /// The ENGINE use handler the row declares (`"bucket_fill"`,
    /// `"bucket_pour"`, `"shear"`), or `None`. Mods react to any item's use
    /// through `item_use_pre` — this field only reveals engine-handled uses.
    pub item_use: Option<String>,
}

/// An item's mining-tool row data (see [`ItemInfoData::tool`]), RESOLVED: a
/// row that states only `kind` and `tier` answers the tier ladder's derived
/// speed and damage here, so a mod computing over a tool (the forge's anvil
/// multiplying an augment onto a base tool) never re-implements the engine's
/// default ladder — the duplicated-constants trap.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolInfoData {
    /// Tool family: `"pickaxe"`, `"axe"`, `"shovel"`, `"shears"`, or `"sword"`.
    pub kind: String,
    /// Material tier `1..=4` (wooden, stone, iron, diamond).
    pub tier: u8,
    /// Mining speed as a multiplier over the bare hand (the row's, or the
    /// tier's derived rung).
    pub speed: f32,
    /// Melee damage range `[min, max]` (the row's, or the derived rung).
    pub damage: [f32; 2],
    /// Knockback multiplier over the victim's own authored shove (`1.0` =
    /// a plain hit; the row's, or a stack override's).
    pub knockback: f32,
}

/// One block's registry row (see [`HostCall::BlockInfo`]) — the stable,
/// mod-relevant harvest facts of its `blocks.json` row, the same data the
/// engine's own break gate reads (so a mod computing over a break never
/// re-implements the material→tool ladder). Session-stable: cache it
/// mod-side, never re-ask per tick.
///
/// [`HostCall::BlockInfo`]: crate::HostCall::BlockInfo
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BlockInfoData {
    /// The row's `material` string (`"stone"`, `"dirt"`, `"ore"`, `"wood"`,
    /// …; `"none"` for the unset default) — the sound/tool class the row
    /// declared, verbatim.
    pub material: String,
    /// Break hardness (seconds-scale; the row's value).
    pub hardness: f32,
    /// The tool tier the harvest gate demands (`0` = harvested by hand).
    pub harvest_tier: u8,
    /// The tool family the gate credits against this block (`"pickaxe"`,
    /// `"axe"`, `"shovel"`), or `None` when any hand harvests — the engine's
    /// own material→tool derivation, answered rather than re-derived.
    pub preferred_tool: Option<String>,
    /// Session id of the ITEM that places this block (the reverse of
    /// [`ItemInfoData::block`]; lowest item id wins when several link), or
    /// `None` when no item places it. Resolve a name via `ItemNames`.
    pub item: Option<ItemId>,
    /// The row's default form's collision boxes as cell-local `(min, max)`
    /// corners in `0..1` — what a body walks into: empty for air, plants,
    /// torches, rails and anything else walked through; one full box for a
    /// cube; the real shape for a slab, a stair, a machine. The registry-time
    /// answer to "what in that cell is a wall", so a rule can sweep its own
    /// body against it without a world read.
    pub collision: Vec<([f32; 3], [f32; 3])>,
}

/// An item's edible row data (see [`ItemInfoData::food`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FoodInfoData {
    /// Game ticks of held-button eating before the item is consumed.
    pub eat_ticks: u32,
    /// Status effects granted when the eat completes.
    pub effects: Vec<FoodEffectData>,
}

/// One granted food effect: an `effects.json` registry key + duration.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FoodEffectData {
    pub effect: String,
    pub ticks: u32,
}

/// Which [`BlockBehavior`](crate::GuestCall::BlockBehavior) hook fired — the mod-side
/// mirror of the engine `BlockBehavior` trait's methods.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockHookKind {
    /// The probabilistic per-section random tick (a few cells per section per
    /// game tick). Mod-behavior blocks always receive random ticks.
    RandomTick,
    /// A scheduled tick previously requested via [`HostCall::ScheduleTick`].
    ///
    /// [`HostCall::ScheduleTick`]: crate::HostCall::ScheduleTick
    ScheduledTick,
    /// The cell or one of its 6 neighbours changed (the ANNOUNCE phase).
    NeighborUpdate,
}

/// One active status effect crossing the ABI (see [`HostCall::EffectsActive`]).
///
/// [`HostCall::EffectsActive`]: crate::HostCall::EffectsActive
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EffectStateData {
    /// The effect's registry key (`"petramond:regeneration"`, `"mod_id:haste"`).
    pub key: String,
    /// Remaining game ticks.
    pub remaining: u32,
}

/// Cached light at a loaded cell (see [`HostCall::LightAt`]), all on the
/// renderer's 6-bit `0..=63` scale; `combined = max(sky, block)`.
///
/// [`HostCall::LightAt`]: crate::HostCall::LightAt
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct LightData {
    pub combined: u8,
    pub sky: u8,
    /// Block-light BRIGHTNESS. Unchanged by coloured light: it is the strongest
    /// channel of [`block_rgb`](Self::block_rgb), so a light-level rule
    /// (crop growth, hostile spawning) means what it always meant.
    pub block: u8,
    /// Per-channel block light, same scale. `block == max(block_rgb)`, and
    /// colourless light is `[block; 3]` — so a mod only needs to look here if
    /// it cares about the HUE (a plant that only grows under blue glow).
    /// Skylight has no per-channel form; it is white by construction.
    pub block_rgb: [u8; 3],
}

/// The collision-shape CLASS of a world cell (see
/// [`HostCall::CollisionShapeAt`]) — generic physics with no gameplay policy
/// baked in. Spawn/placement rules compose on top of it in mod code (e.g.
/// `Full` + not water + not tagged `petramond:leaves`).
///
/// [`HostCall::CollisionShapeAt`]: crate::HostCall::CollisionShapeAt
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum CollisionShape {
    /// No collision boxes: air, water, walk-through cover (tall grass).
    Empty,
    /// Collision boxes that do not amount to one full unit cube: stairs,
    /// slabs, doors, snow layers, model blocks.
    Partial,
    /// Exactly one collision box spanning the whole unit cell.
    Full,
}

/// The read-only mob snapshot an [`GuestCall::AiNode`] decision sees.
///
/// The baseline fields (the mob's own state, the current tick, and the
/// nearest player's id/position) are always present. Fact fields beyond the
/// baseline are DECLARED INPUTS: the brain node row lists the facts its node
/// reads (`"inputs": ["player_held"]` in `mobs.json`), and only declared
/// facts are computed and shipped — an undeclared fact always reads `None`.
/// Every `player_*` fact describes the SAME player, [`player_id`]
/// (the nearest one), mutually consistent within a dispatch.
///
/// [`player_id`]: AiNodeCtx::player_id
/// [`GuestCall::AiNode`]: crate::GuestCall::AiNode
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AiNodeCtx {
    /// Stable id of the deciding mob — key per-mob guest state off it.
    pub mob_id: u64,
    /// Mob feet position (world space).
    pub pos: [f32; 3],
    /// Mob foothold voxel.
    pub cell: [i32; 3],
    /// Body facing (radians).
    pub yaw: f32,
    /// The current game tick — the same value `current_tick()` returns
    /// (dispatch runs once per owning mob per game tick), carried here so
    /// timekeeping costs no host call.
    pub tick: u64,
    /// Session id of the NEAREST player — the player every `player_*` fact
    /// in this snapshot describes, and the target of an attack decision.
    pub player_id: PlayerId,
    /// That player's body-centre (world space).
    pub player_pos: [f32; 3],
    /// True when the navigator has no active path ("the mob is idle").
    pub nav_idle: bool,
    /// True when the mob's body is in water.
    pub in_water: bool,
    /// DECLARED INPUT `"player_held"`: the nearest player's selected (held)
    /// item — resolve names via `ResolveItem` and compare (a lure, a beg, a
    /// trade gate all read this same fact). `None` when the input is
    /// undeclared, the hand is empty, or the player is a spectator.
    pub player_held: Option<ItemId>,
    /// DECLARED INPUT `"player_foothold"`: the mob-standable navigation
    /// foothold nearest that player (what the engine's `chase_player` paths
    /// toward) — the ready-made `goal` for any follow/approach node. `None`
    /// when the input is undeclared, the player is airborne or has no
    /// reachable foothold, or the player is more than 32 blocks away (the
    /// outer edge of player-reactive mob AI — the scan is skipped past it).
    pub player_foothold: Option<[i32; 3]>,
    /// The deciding mob's OWN tag map (baseline — the mob's own state),
    /// sorted by key: the same view `mob_tags_get` returns, without a host
    /// call. Persist per-mob node state by WRITING tags back through
    /// [`AiNodeDecision::tags`] instead of keying a guest-side map off
    /// `mob_id` — tag state lives, saves, and dies with the mob.
    pub tags: Vec<(String, MobTagValue)>,
}

/// One tag write a scripted node's decision carries back — applied by the
/// ENGINE after the detached dispatch returns (a node cannot call `mob_tag_set`
/// mid-decision). `value: None` deletes the key.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MobTagWrite {
    /// Namespaced tag key. Must carry the deciding node's OWN `mod_id:`
    /// prefix — a decision may not write engine or foreign tags (unlike
    /// `mob_tag_set`, which may write exposed `petramond:*` keys).
    pub key: String,
    /// The value to store, or `None` to delete the key.
    pub value: Option<MobTagValue>,
}

/// One scripted node's contribution to a mob's tick. The opinion fields
/// default to "no opinion"; the engine keeps the highest-priority non-`None`
/// value per field across the whole brain (scripted and engine nodes alike).
/// `tags` is NOT arbitrated: every node's writes apply, in brain order.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct AiNodeDecision {
    /// A navigation destination (world voxel) to path toward.
    pub goal: Option<[i32; 3]>,
    /// A desired head orientation `[yaw, pitch]` relative to the body.
    pub head_look: Option<[f32; 2]>,
    /// An `idle_*` animation index to play.
    pub idle_anim: Option<u8>,
    /// A melee strike `[damage, knockback]` to land on the player this tick.
    pub attack: Option<[f32; 2]>,
    /// Tag writes on the deciding mob itself, applied by the engine after the
    /// dispatch (own-namespace keys only; the 32-tag cap refuses NEW keys
    /// past it). This is the persistence channel for per-mob node state —
    /// see [`AiNodeCtx::tags`] for the read side.
    pub tags: Vec<MobTagWrite>,
}
