//! Live mobs and item entities: spawning, queries, damage, riding, drive and
//! kinematic intents, named animations, navigation probes, and presentation.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::data::EntityRef;
use crate::ids::PlayerId;
use crate::legality::prelude::*;

host_domain! {
    /// Live mobs and item entities: spawning, queries, damage, riding, drive and
    /// kinematic intents, named animations, navigation probes, and presentation.
    EntityCall {
        /// Spawn a mob by species key at `pos` (feet) facing `yaw`. With
        /// `checked: false` the spawn is unconditional (site fitness is the
        /// caller's business); `checked: true` spawns only when the COMPLETE
        /// declared body fits — every covered section loaded and stream-final, no
        /// terrain collision overlap, no live solid mob overlap — validated and
        /// inserted as one atomic sim operation (use it for player-placed bodies:
        /// a failed call mutates nothing, so the item can be refunded). The reply
        /// carries the newborn's STABLE id (`None` = unknown key, or
        /// a failed check) so the spawner can immediately tag/configure it.
        /// → [`HostRet::SpawnedMob`](crate::HostRet::SpawnedMob).
        SpawnMob {
            key: String,
            pos: [f64; 3],
            yaw: f32,
            checked: bool,
        } => legal(SERVER, Sim, Write),
        /// Snapshot the live mobs within `radius` (3-D, of feet positions) of
        /// `pos`. Deterministic order = the live set's storage order (spawn order,
        /// perturbed only by removals). Dead (ragdolling) mobs are excluded.
        /// → [`HostRet::Mobs`](crate::HostRet::Mobs).
        MobsInRadius {
            pos: [f64; 3],
            radius: f32,
        } => legal(SERVER, Sim, Read),
        /// Damage the live mob `mob_id` through its global engine-owned i-frames
        /// and the `mob_damage_pre` pipeline. Applied at the next action drain
        /// point (same tick), so a handler cannot re-enter the bus; a mob gone
        /// by then is a silent no-op. → [`HostRet::Unit`](crate::HostRet::Unit).
        ///
        /// `attacker` names WHO the hit is landed for. `None` is the mod's own
        /// damage ([`DamageSource::Mod`]): not an attack, so no default
        /// knockback and no retaliation memory; `origin` is then only spatial
        /// context for feedback/handlers. `Some(EntityRef::Player(..))` makes
        /// the request that player's melee strike
        /// ([`DamageSource::PlayerAttack`]) — the victim remembers them, and
        /// with an `origin` the species' knockback shoves away from it —
        /// exactly as if the engine's own crosshair hit had landed; the id must
        /// be a connected session ([`HostRet::Err`](crate::HostRet::Err) otherwise).
        /// `Some(EntityRef::Mob(..))` is that mob's strike
        /// ([`DamageSource::MobAttack`]); a mob no longer alive degrades to the
        /// mod's own damage.
        ///
        /// `feedback` composes the damage pipeline for THIS request; `None` uses
        /// the species' resolved `damage_feedback`. A pipeline without the
        /// `Immunity` component is damage on its own clock: neither blocked by
        /// the victim's active i-frame window nor granting one.
        ///
        /// [`DamageSource::Mod`]: crate::DamageSource::Mod
        /// [`DamageSource::PlayerAttack`]: crate::DamageSource::PlayerAttack
        /// [`DamageSource::MobAttack`]: crate::DamageSource::MobAttack
        DamageMob {
            mob_id: u64,
            amount: f32,
            origin: Option<[f64; 3]>,
            feedback: Option<crate::events::MobDamageFeedback>,
            attacker: Option<EntityRef>,
        } => legal(SERVER, Sim, Write),
        /// Remove the live mob `mob_id` from the world immediately (not saved,
        /// no death/loot). `false` = no such live mob. → [`HostRet::Bool`](crate::HostRet::Bool).
        DespawnMob {
            mob_id: u64,
        } => legal(SERVER, Sim, Write),
        /// Spawn `count` of an item (by registry NAME — the one mod-facing item
        /// identity, e.g. `"petramond:coal"`, `"farming:wheat"`) as a dropped-item
        /// entity at `pos`, carrying `data` as the stacks' instance data (empty =
        /// plain; see [`ItemStackData::data`](crate::ItemStackData::data)). `false` = unknown name / zero
        /// count; a malformed `data` map is [`HostRet::Err`](crate::HostRet::Err). →
        /// [`HostRet::Bool`](crate::HostRet::Bool).
        SpawnItem {
            item: String,
            count: u8,
            pos: [f64; 3],
            data: Vec<(String, Vec<u8>)>,
        } => legal(SERVER, Sim, Write),
        /// Toggle one KEYED particle-emitter bundle on the live mob `mob_id`.
        /// `key` names a `particle_emitters.json` catalog row (engine
        /// `petramond:*` rows — `petramond:burn_light`, `petramond:burn_great` —
        /// and every pack's rows alike, the same cross-namespace rule as
        /// effects): one or more particle rows plus an optional body tint. The
        /// active set (≤ 4 per mob) is presentation-only, replicates to every
        /// client, survives death (a corpse keeps its already-active effects
        /// through the ragdoll — though a corpse can no longer be addressed), and
        /// is NOT persisted: the owning mod re-derives it, e.g. from its own
        /// per-mob state. → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = unknown/dead mob,
        /// unregistered key, or the mob's active set is full).
        MobEmitterSet {
            mob_id: u64,
            key: String,
            active: bool,
        } => legal(SERVER, Sim, Write),
        /// Seat player `player_id` in `seat` of the live mob `mob_id` (stable
        /// id). Validated by the engine: the mob is alive and its species row
        /// declares that seat (`seats` in `mobs.json`), the seat is free, and the
        /// player is not already mounted. WHO may sit WHERE is the calling mod's
        /// policy — usually decided in its `interact_attempt` handler. From this tick
        /// the engine slaves the rider to the seat; every detach path announces
        /// [`EventKind::PlayerDismounted`](crate::EventKind::PlayerDismounted). → [`HostRet::Bool`](crate::HostRet::Bool).
        ///
        /// [`EventKind::PlayerDismounted`]: crate::EventKind::PlayerDismounted
        MobMount {
            mob_id: u64,
            player_id: PlayerId,
            seat: u8,
        } => legal(SERVER, Sim, Write),
        /// Unseat `player_id` from whatever they ride (the mod-initiated detach;
        /// the engine's own valves — sneak gesture, death, despawn — detach
        /// without this call). `false` = they were not mounted.
        /// → [`HostRet::Bool`](crate::HostRet::Bool).
        MobDismount {
            player_id: PlayerId,
        } => legal(SERVER, Sim, Write),
        /// The declared seat capacity and every rider of the live mob `mob_id`,
        /// in player-id order. `None` = no such live mob, which is distinct from
        /// a live mob with zero seats or riders. → [`HostRet::Riders`](crate::HostRet::Riders).
        MobRiders {
            mob_id: u64,
        } => legal(SERVER, Sim, Read),
        /// Drive the live mob `mob_id` kinematically for THIS tick — full 3-D
        /// velocity access, each part independently optional so a mod composes
        /// with, or replaces, the engine's own locomotion:
        /// - `horizontal`: a world-space `[x, z]` velocity (m/s) that REPLACES
        ///   the brain's wish locomotion for the tick (a vehicle; the mob does
        ///   not read as walking). `None` leaves the brain's walking untouched.
        /// - `vertical`: a vertical velocity (m/s) set for the tick; gravity
        ///   resumes next tick, and water buoyancy stays engine-owned. Composes
        ///   with EITHER horizontal source — an upward value from the ground is
        ///   a launch (the walking gait carries through the arc), which is how a
        ///   pack authors a gait like a hop without the engine knowing the word.
        ///   An engine navigation step-jump keeps priority over it on the tick
        ///   both fire.
        /// - `yaw`, when present, sets the absolute facing (mob convention: yaw
        ///   `0` faces `-Z`, facing `(-sin yaw, 0, -cos yaw)`).
        ///
        /// Like the wish it is an intent, not a state: re-issue it every tick
        /// (friction, steering feel, and control policy are the driving mod's) —
        /// a mod that stops calling leaves the mob to its brain. Knockback
        /// stagger overrides the drive for its duration. Collision always stays
        /// engine-owned.
        ///
        /// `while_walking` carries the intent's PREMISE: when `true`, the intent
        /// is consumed only on a tick whose brain locomotion is actually walking
        /// the mob (the snapshot's `moving` fact) and silently dropped
        /// otherwise. A latched intent is decided from LAST tick's state, and
        /// the walk it was premised on can end in between (arrival, a route
        /// abandoned, knockback) — an unconditional launch then fires one stale
        /// in-place bounce at the destination. A walking-gated intent cannot
        /// carry `horizontal` (walking IS the horizontal locomotion; the host
        /// refuses the combination). `false` = unknown or dead mob.
        /// → [`HostRet::Bool`](crate::HostRet::Bool).
        MobDrive {
            mob_id: u64,
            horizontal: Option<[f32; 2]>,
            vertical: Option<f32>,
            yaw: Option<f32>,
            while_walking: bool,
            /// The horizontal drive is the body WALKING ITSELF there (a step
            /// sideways, a shuffle to the middle of its block), not something
            /// carrying it: the mob reads as `moving` — walk clip paced to the
            /// driven speed, footsteps — where a plain drive (a boat, a cart)
            /// deliberately does not.
            gait: bool,
        } => legal(SERVER, Sim, Write),
        /// Toggle a NAMED model animation on the live mob `mob_id` — the
        /// animation sibling of [`EntityCall::MobEmitterSet`]: presentation-only,
        /// at most 4 active per mob, replicated, never persisted (the owning mod
        /// re-derives it). Each active animation LAYERS over the walk/idle/rest
        /// base pose with its OWN self-clocked phase (activation starts it at
        /// phase 0, rate 1) — drive the playback with
        /// [`EntityCall::MobAnimRate`]. `anim` is an animation name from the mob's
        /// own `.bbmodel`; unknown names are accepted and draw nothing (the sim
        /// never loads models — same forgiveness as a disabled pack). `false` =
        /// unknown mob or the per-mob cap. → [`HostRet::Bool`](crate::HostRet::Bool).
        MobAnimSet {
            mob_id: u64,
            anim: String,
            active: bool,
        } => legal(SERVER, Sim, Write),
        /// Set the PLAYBACK RATE of an active named animation on the live mob
        /// `mob_id` (see [`EntityCall::MobAnimSet`]): its phase advances by
        /// `rate` animation-seconds per real second — `1.0` plays, `0.0` FREEZES
        /// mid-stroke exactly where it is (an oar pauses in place, never snaps
        /// home), negative plays in reverse. Cancels an in-flight
        /// [`EntityCall::MobAnimSeek`]. Code-driven playback over an authored
        /// clip: the motion's SHAPE stays tunable in Blockbench, the mod owns
        /// play/pause/reverse/speed. `false` = unknown mob or the anim is not
        /// active. → [`HostRet::Bool`](crate::HostRet::Bool).
        MobAnimRate {
            mob_id: u64,
            anim: String,
            rate: f32,
        } => legal(SERVER, Sim, Write),
        /// SEEK an active named animation to the absolute `phase` at `|rate|`
        /// animation-seconds per second: the layer's phase approaches the target
        /// DIRECTLY (no modulo — the caller picks the nearest-cycle target for a
        /// shortest-path return), lands on it EXACTLY, and holds (rate 0). How
        /// an oar settles gently back onto its authored pose from wherever the
        /// stroke stopped. A [`EntityCall::MobAnimRate`] cancels the seek. `false`
        /// = unknown mob or the anim is not active. → [`HostRet::Bool`](crate::HostRet::Bool).
        MobAnimSeek {
            mob_id: u64,
            anim: String,
            phase: f32,
            rate: f32,
        } => legal(SERVER, Sim, Write),
        /// Read the authoritative playback state of active named animation
        /// `anim` on live mob `mob_id`. `None` = missing/dead mob or inactive
        /// animation. This is the source of truth for control policy that needs
        /// the current phase (for example, choosing a nearest-cycle seek target).
        /// → [`HostRet::MobAnimState`](crate::HostRet::MobAnimState).
        MobAnimState {
            mob_id: u64,
            anim: String,
        } => legal(SERVER, Sim, Read),
        /// Snapshot ONE live mob by its stable id — the single-mob sibling of
        /// [`EntityCall::MobsInRadius`], for a handler that already holds an id
        /// (an event payload, a stored tag) and needs the mob's current state
        /// (pose to act on, species to branch on). `None` = no such live mob
        /// (dead mobs are gone to the ABI, as everywhere).
        /// → [`HostRet::Mob`](crate::HostRet::Mob).
        MobInfo {
            mob_id: u64,
        } => legal(SERVER, Sim, Read),
        /// Whether the live mob `mob_id` can genuinely NAVIGATE from where it
        /// stands to `cell` — a bounded engine pathfinding probe with the mob's
        /// real body, the same honesty test the engine's own wander applies to
        /// its destination picks. Ask this before committing the mob to any
        /// PICKED walk-target cell (food to graze, a trough, a partner's cell):
        /// the pathfinder deliberately answers an unreachable goal with a
        /// best-effort partial route (chases must crowd their target), which
        /// PARKS the mob against the obstacle when the goal was just a picked
        /// cell — grass beyond a fence pins a penned animal to the fence
        /// forever. `false` = unreachable within the probe budget, no such live
        /// mob, or the mob is airborne (nothing provable — retry later).
        /// → [`HostRet::Bool`](crate::HostRet::Bool).
        MobCanReach {
            mob_id: u64,
            cell: [i32; 3],
        } => legal(SERVER, Sim, Read),
        /// Pin `player_id` in a named POSE at the world-space `anchor` (rider
        /// feet origin), body facing `yaw` (player convention: yaw `0` faces
        /// `+Z`) — the static-seat primitive. The calling mod owns WHERE poses
        /// exist (its own seat layout) and WHO may take one; the engine owns the
        /// mechanism: one pose per player, no two players on one exact anchor,
        /// replication + the posed body, and every release valve (sneak gesture,
        /// death, spectator, leave). Pose vocabulary: [`crate::pose`] (`0` is
        /// reserved; unknown values pin the rest pose). Poses are TRANSIENT
        /// (never persisted) and NOT tied to any block — a mod whose furniture
        /// breaks releases the sitter itself ([`EntityCall::MobDismount`]); a
        /// player a disabled mod leaves posed escapes through the engine valves.
        /// Occupancy is read back from the roster
        /// ([`crate::PlayerSnapshot::pose_anchor`]), never mirrored in mod
        /// state. `false` = already posed or mounted, anchor taken, reserved
        /// pose `0`, or a non-finite anchor/yaw. → [`HostRet::Bool`](crate::HostRet::Bool).
        PlayerPoseSet {
            player_id: PlayerId,
            anchor: [f64; 3],
            yaw: f32,
            pose: u8,
        } => legal(SERVER, Sim, Write),
        /// The placed MODEL-BLOCK group at `pos` (any of its cells): the group's
        /// base cell and placement facing — what block-local policy needs to map
        /// footprint-space data (a seat layout, a machine front) into the world.
        /// `None` = no model group there or the cell is unloaded.
        /// → [`HostRet::ModelGroup`](crate::HostRet::ModelGroup).
        BlockModelGroup {
            pos: [i32; 3],
        } => legal(SERVER, Sim, Read),
        /// Whether `cell` is a place a body of species `key` could stand and
        /// still ROAM: a navigation foothold whose reachable ground is open world
        /// rather than a closed-off region (a pen). The positional twin of
        /// [`EntityCall::MobCanReach`] — it needs no live mob, so a mod can judge a
        /// site BEFORE spawning anything there.
        ///
        /// Ask it about any site your own spawner picked. Body clearance alone
        /// (what [`EntityCall::SpawnMob`]'s `checked` proves) still admits a site
        /// over a hole, inside rock, or inside somebody's fenced pasture — where
        /// a spawned animal would fall, suffocate, or be born captive in a pen
        /// its owner built for other animals. `false` = no footing, confined, an
        /// unknown species, or unloaded terrain (all "don't spawn here").
        /// → [`HostRet::Bool`](crate::HostRet::Bool).
        SiteOpen {
            key: String,
            cell: [i32; 3],
        } => legal(SERVER, Sim, Read),
        /// Spawn ONE `item` (by registry NAME, `data` as its instance data) as
        /// an item entity IN FLIGHT: launched from `pos` at velocity `vel`
        /// (m/s), heading along its motion, falling and slowing per the row's
        /// `petramond:projectile` data, and STRIKING what it flies into — the
        /// first live body or collidable block along each tick's motion raises
        /// [`EventKind::ProjectileHit`](crate::EventKind::ProjectileHit). `owner` is who launched it: reported on
        /// the entity ([`ItemEntity`](Self::ItemEntity)), and not a target until
        /// the item has once left the launcher's body — it starts inside it, so
        /// the body it leaves through is not a hit, while a launch that comes
        /// back around strikes its launcher like anyone else. An arrow leaves a
        /// bow through this, and so would a thrown spear or a snowball: the
        /// engine owns flight, impact detection, lodging, replication,
        /// persistence and pickup; what an impact DOES is the launcher's policy
        /// in its handler. → [`HostRet::U64`](crate::HostRet::U64), the entity's stable session id,
        /// `0` = unknown item; a malformed `data` map or non-finite vector is
        /// [`HostRet::Err`](crate::HostRet::Err).
        ///
        /// [`EventKind::ProjectileHit`]: crate::EventKind::ProjectileHit
        LaunchItem {
            item: String,
            pos: [f64; 3],
            vel: [f32; 3],
            owner: Option<EntityRef>,
            data: Vec<(String, Vec<u8>)>,
        } => legal(SERVER, Sim, Write),
        /// Snapshot ONE item entity by its stable id — the read a
        /// [`EventKind::ProjectileHit`](crate::EventKind::ProjectileHit) handler makes to learn what struck (the
        /// stack, its instance data, who launched it), or any rule tracking a
        /// drop it spawned. → [`HostRet::ItemEntity`](crate::HostRet::ItemEntity), `None` = no such live
        /// entity.
        ///
        /// [`EventKind::ProjectileHit`]: crate::EventKind::ProjectileHit
        ItemEntity {
            entity: u64,
        } => legal(SERVER, Sim, Read),
        /// AUTHOR the live mob `mob_id`'s transform for THIS tick — the
        /// constrained sibling of [`MobDrive`]: where a drive hands the engine a
        /// velocity and lets its physics land the body, a kinematic placement
        /// hands it the RESULT. `pos` (feet), `yaw` (mob convention: `0` faces
        /// `-Z`), `pitch` (radians about the lateral axis inside the yaw,
        /// positive = nose up) and `roll` (radians about the facing axis inside
        /// both, positive = right side up) are written as given; for that tick
        /// the engine runs none of its own motion — no gravity, no buoyancy, no
        /// terrain sweep, no knockback stagger, no brain locomotion — and the
        /// body still blocks players, pushes soft mobs and seats its riders from
        /// the pose it was given. This is the seam for a body whose path is a
        /// CONSTRAINT the engine cannot know — a cart on a rail, a car on a
        /// track, a lift on a cable, a hull on a swell: the mod integrates along
        /// the constraint, the engine presents.
        ///
        /// An intent, never a state: re-issue it every tick. A mod that stops
        /// calling leaves the body AIRBORNE at its last pose carrying the
        /// velocity the placements implied, so it flies, falls and lands by the
        /// engine's own physics — a cart running off the end of its track
        /// arcs into the air instead of freezing where the rail ended, and a
        /// disabled mod's vehicle simply drops onto whatever it was on — and its
        /// tilt eases back to level over a few ticks. Fall bookkeeping
        /// re-anchors on every placement, so a released body only ever pays for
        /// the drop it actually makes afterwards.
        ///
        /// The host refuses non-finite values, a placement farther than the
        /// 16-block sweep bound from the body's current position, a pitch
        /// outside `±π/2` and a roll outside `±π`. `false` = unknown or dead
        /// mob. → [`HostRet::Bool`](crate::HostRet::Bool).
        ///
        /// [`MobDrive`]: Self::MobDrive
        MobKinematic {
            mob_id: u64,
            pos: [f64; 3],
            yaw: f32,
            pitch: f32,
            roll: f32,
        } => legal(SERVER, Sim, Write),
        /// Whether a body of species `key` standing at foothold `from` can walk
        /// to foothold `to`, treating every `blocked` cell as a solid block — a
        /// wall that is planned but not built. The probe spends up to `max_nodes`
        /// search expansions (at most the navigator's own route budget) from a
        /// per-tick budget shared by every route probe. → [`HostRet::Route`](crate::HostRet::Route):
        /// `None` = the budget cannot cover it this tick (ask again next tick) or
        /// the species is unknown. Server only.
        PathProbe {
            key: String,
            from: [i32; 3],
            to: [i32; 3],
            blocked: Vec<[i32; 3]>,
            max_nodes: u32,
        } => legal(SERVER, Sim, Read),
        /// Which `cells` a body of species `key` could stand in (a navigation
        /// foothold with room for the body, out of hazards), parallel to `cells`
        /// (at most `SIM_BATCH_MAX`). Unknown species = all `false`. Server only.
        /// → [`HostRet::Bools`](crate::HostRet::Bools).
        Footholds {
            key: String,
            cells: Vec<[i32; 3]>,
        } => legal(SERVER, Sim, Read),
        /// Draw item `main` in the live mob's main hand and `off` in its off hand
        /// (registry names; `None` = empty), at the hand bones its row names.
        /// Presentation only: replicated, never persisted — the claiming mod
        /// re-derives it. → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such live mob or an
        /// unknown item).
        MobHeldDisplay {
            mob_id: u64,
            main: Option<String>,
            off: Option<String>,
        } => legal(SERVER, Sim, Write),
        /// Every foothold inside the inclusive box `min..=max` that a body of
        /// species `key` walks to from foothold `from` without leaving the box —
        /// or, with `toward`, every foothold in it that walks to `from` — each
        /// `blocked` cell treated as a solid block. The moves are
        /// [`PathProbe`](Self::PathProbe)'s, so one call answers a probe per cell,
        /// and a detour inside the box is never cut short. Spends from the same
        /// per-tick budget, at most `max_nodes` footholds. → [`HostRet::Flood`](crate::HostRet::Flood)
        /// (an unknown species is `Exceeded`: no asking again answers it). Server
        /// only.
        WalkRegion {
            key: String,
            from: [i32; 3],
            min: [i32; 3],
            max: [i32; 3],
            blocked: Vec<[i32; 3]>,
            toward: bool,
            max_nodes: u32,
        } => legal(SERVER, Sim, Read),
        /// Replace the retained draw set a live mob wears (empty clears it):
        /// [`SetBlockDraw`](crate::BlockCall::SetBlockDraw) for a body instead of a cell.
        /// Prim space has its origin at the mob's FEET centre, in block units;
        /// `frame` says whether it turns with the body's yaw (a hat) or keeps
        /// the world's axes (a mark hung in the air beside it). The set follows
        /// the body between ticks on every viewer's own interpolation, is
        /// replicated, never saved, and ends with the mob. Bounded and
        /// finite-checked like a block's set. → [`HostRet::Bool`](crate::HostRet::Bool): `false` = no
        /// such mob. Server only.
        SetMobDraw {
            mob_id: u64,
            frame: crate::DrawFrame,
            prims: Vec<crate::DrawPrim>,
        } => legal(SERVER, Sim, Write),
        /// [`EntityCall::MobDrive`] for many mobs in one crossing: each element
        /// is one tick's intent, validated and applied exactly as the single
        /// call would. At most [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) drives.
        /// → [`HostRet::Bools`](crate::HostRet::Bools), parallel to `drives`.
        MobDriveMany {
            drives: Vec<crate::MobDriveData>,
        } => legal(SERVER, Sim, Write),
        /// [`EntityCall::MobKinematic`] for many mobs in one crossing (same
        /// validation per pose). At most
        /// [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) poses.
        /// → [`HostRet::Bools`](crate::HostRet::Bools), parallel to `poses`.
        MobKinematicMany {
            poses: Vec<crate::MobKinematicData>,
        } => legal(SERVER, Sim, Write),
        /// `MobAnimSet` / `MobAnimRate` / `MobAnimSeek` commands for many mobs
        /// in one crossing, applied in order (a `Set` activating a clip and a
        /// `Rate` steering it may ride the same batch). At most
        /// [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) commands.
        /// → [`HostRet::Bools`](crate::HostRet::Bools), parallel to `ops`,
        /// each meaning what the single call's `Bool` means.
        MobAnimMany {
            ops: Vec<crate::MobAnimOp>,
        } => legal(SERVER, Sim, Write),
        /// [`EntityCall::MobRiders`] for many mobs in one crossing. At most
        /// [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) ids.
        /// → [`HostRet::RidersMany`](crate::HostRet::RidersMany), parallel to
        /// `mob_ids`.
        MobRidersMany {
            mob_ids: Vec<u64>,
        } => legal(SERVER, Sim, Read),
    }
}
