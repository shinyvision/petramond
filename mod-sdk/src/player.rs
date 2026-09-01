//! Sim-scoped player calls: state, input, the damage funnel, knockback,
//! items, health, teleports, status effects, and chat delivery.

use mod_api::{
    BodyAction, BonePoseData, EffectStateData, EntityRef, HandMotion, HeldPose, PlayerAttribute,
    PlayerId, PlayerInputData, PlayerSnapshot,
};

use crate::__rt::host_fn;

/// The horizontal direction a player yaw faces — PLAYER convention: yaw `0`
/// faces `+Z` (π apart from the mob convention, [`crate::mob_facing_xz`]);
/// a mount aligned with its rider takes `player_yaw + π` as its mob yaw.
pub fn player_facing_xz(yaw: f32) -> [f32; 2] {
    let (s, c) = yaw.sin_cos();
    [s, c]
}

host_fn! {
    /// The player's current state (position, velocity, look, health, flags).
    pub fn player_state() -> PlayerSnapshot => PlayerState => Player
}

host_fn! {
    /// One player's movement intent this tick (forward/strafe in their own yaw
    /// frame, jump/sneak, look) — how a vehicle mod reads what its driver is
    /// pressing. `None` = no such player connected.
    pub fn player_input(player_id: PlayerId) -> Option<PlayerInputData>
        => PlayerInput { player_id } => PlayerInput
}

host_fn! {
    /// The named session's currently held stack, INSTANCE DATA included — the
    /// per-player, per-stack read [`player_state`]'s row-level `held` id
    /// cannot be (an augmented tool's `petramond:tool` override lives in the
    /// stack's data). `None` = empty hand, no such connected session, or a
    /// dispatch site without a sessions view (event handlers and attached
    /// tick systems always have one).
    pub fn player_held(player: PlayerId) -> Option<mod_api::ItemStackData>
        => PlayerHeld { player } => HeldStack
}

host_fn! {
    /// Consume `count` units of the ACTING player's held stack, atomically, only
    /// when it holds `item` with at least `count` — the spend primitive for item
    /// uses that place no block (spawning an entity from an `item_use_pre`
    /// handler). `false` = consumed nothing.
    pub fn consume_held(item: mod_api::ItemId, count: u32) -> bool
        => ConsumeHeld { item, count } => Bool
}

host_fn! {
    /// Swap ONE of the held stack for `replacement` (by registry name) when the
    /// held stack holds at least one of `item`. A single-item stack swaps in
    /// place; a larger stack consumes one unit and gives the replacement through
    /// normal inventory fill. `false` = wrong/empty hand, unknown replacement, or
    /// no room. This is the bucket empty/fill primitive.
    pub fn replace_held_one(item: mod_api::ItemId, replacement: &str) -> bool
        => ReplaceHeldOne { item, replacement: replacement.into() } => Bool
}

host_fn! {
    /// Damage `player` through the engine funnel. The victim's global
    /// engine-owned i-frames and `player_damage_pre` apply. Queued; applied
    /// at the next in-tick drain point; an unknown session is a no-op.
    ///
    /// `attacker` is WHO the hit lands for, exactly as on [`crate::damage_mob`]:
    /// `None` is the mod's own damage (`DamageSource::Mod`, `origin` spatial
    /// context only); `Some(EntityRef::Player(..))` is that player's melee
    /// strike — the victim's `player_damage_pre` sees
    /// `DamageSource::PlayerAttack` with the `origin` (a shield judges its
    /// arc from it) and an applied hit shoves them away from it like the
    /// engine's own hit; `Some(EntityRef::Mob(..))` is that mob's.
    ///
    /// To KILL a player, pass their current health ([`players`]) as
    /// `amount` — same funnel; i-frames or a pre-event handler can still
    /// reject it. There is no separate kill call.
    pub fn damage_player(
        player: PlayerId,
        amount: i32,
        origin: Option<[f32; 3]>,
        attacker: Option<EntityRef>,
    )
        => DamagePlayer { player, amount, origin, attacker }
}

host_fn! {
    /// Add a knockback impulse to the player's velocity (spectator no-op).
    pub fn apply_knockback(impulse: [f32; 3]) => ApplyKnockback { impulse }
}

host_fn! {
    /// Give the player items (by registry NAME) through the normal inventory
    /// fill; overflow drops at the player's feet. `false` = unknown item name.
    pub fn give_item(item: &str, count: u8) -> bool
        => GiveItem { item: item.into(), count, data: Vec::new() } => Bool
}

host_fn! {
    /// [`give_item`] carrying per-stack instance data (namespaced key →
    /// value bytes; ≤4 keys, ≤64-byte values — an over-cap map is a hard
    /// error that disables the mod). Stacks merge only on byte-identical
    /// data.
    pub fn give_item_data(item: &str, count: u8, data: &[(&str, &[u8])]) -> bool
        => GiveItem {
            item: item.into(),
            count,
            data: data.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect(),
        } => Bool
}

host_fn! {
    /// [`give_item`] addressed to a NAMED session (the explicit-player
    /// addressing doctrine): fill THAT player's inventory, drop the overflow
    /// at that player's feet, `data` as the stack's instance data (pass `&[]`
    /// for a plain stack). The delivery a machine owes a specific viewer —
    /// a transient panel returning its contents on close. `false` = unknown
    /// item name or no such connected session (deliver another way, e.g.
    /// `spawn_item` at the machine).
    pub fn give_item_to(player: PlayerId, item: &str, count: u8, data: &[(&str, &[u8])]) -> bool
        => GiveItemTo {
            player,
            item: item.into(),
            count,
            data: data.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect(),
        } => Bool
}

host_fn! {
    /// Rewrite the instance data on the stack `player` is HOLDING, iff it is
    /// still an `expect_item` carrying exactly `expect_data` — the
    /// compare-and-set a wear system needs to restamp a tool between the event
    /// it observed and this write. The compare covers the VALUE being
    /// replaced, not just the item: a hand swapped OR the same stack
    /// re-stamped by another handler or another mod in between refuses rather
    /// than clobbers, so two writers in one tick cannot silently drop one of
    /// the two updates. Pass the map you READ off the stack as `expect_data`
    /// (`&[]` expects a plain stack) and the FULL replacement as `data` (≤4
    /// namespaced keys; `&[]` clears it); the item and count stay. `false` =
    /// empty/other hand, data that no longer matches `expect_data`, unknown
    /// item name, or no such connected session — re-read the stack and
    /// recompute rather than retrying the same write.
    pub fn set_player_held_data(
        player: PlayerId,
        expect_item: &str,
        expect_data: &[(&str, &[u8])],
        data: &[(&str, &[u8])],
    ) -> bool
        => SetPlayerHeldData {
            player,
            expect_item: expect_item.into(),
            expect_data: expect_data.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect(),
            data: data.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect(),
        } => Bool
}

host_fn! {
    /// Overwrite the player's health (clamped to `0..=20` half-hearts), bypassing
    /// the damage funnel — the heal/set primitive, no events fire.
    pub fn set_health(value: i32) => SetHealth { value }
}

host_fn! {
    /// Move the player's feet to `pos`; fall tracking is cleared so a teleport can
    /// never land as fall damage.
    pub fn teleport(pos: [f32; 3]) => Teleport { pos }
}

host_fn! {
    /// Grant the player the status effect `key` (an `effects.json` row — engine
    /// `petramond:*` rows and every pack's rows alike) for `ticks` game ticks. An
    /// already-active effect is overwritten with the new duration; `0` removes it.
    /// A state primitive like [`set_health`] — no events fire. `false` = unknown
    /// effect key.
    pub fn effect_apply(key: &str, ticks: u32) -> bool
        => EffectApply { key: key.into(), ticks } => Bool
}

/// Remove the status effect `key` from the player if active. `false` =
/// unknown effect key. Sugar for [`effect_apply`] with `ticks: 0` — the
/// engine has no separate remove call.
pub fn effect_remove(key: &str) -> bool {
    effect_apply(key, 0)
}

host_fn! {
    /// The player's active status effects, in application order.
    pub fn effects_active() -> Vec<EffectStateData> => EffectsActive => Effects
}

host_fn! {
    /// Deliver one server-authored chat line. `targets: None` broadcasts to every
    /// currently connected client; `Some(ids)` sends only to those player ids
    /// (unknown / left ids are ignored). Markup `$[fg=color]` is parsed by the
    /// server. Empty / whitespace-only text returns `false`.
    pub fn chat_send(text: &str, targets: Option<&[PlayerId]>) -> bool
        => ChatSend {
            text: text.into(),
            targets: targets.map(|ids| ids.to_vec()),
        } => Bool
}

host_fn! {
    /// Every connected player this tick, in session-id order (single player =
    /// one entry) — the multiplayer-aware roster for spawn/ambience/weather
    /// policy. Address a specific player through the entry's `id`.
    pub fn players() -> Vec<mod_api::PlayerListEntry> => Players => Players
}

host_fn! {
    /// Unlock a crafting recipe for `player`: it joins their recipe browser at
    /// whatever station the recipe declares, and the server starts accepting it
    /// from them. Idempotent and persistent — `true` = this call is what
    /// unlocked it, `false` = already unlocked, unknown recipe key, or unknown
    /// player.
    ///
    /// Unlocking is a CONSEQUENCE of whatever the mod decides earns it; call
    /// this from an event handler, never from a per-tick poll.
    pub fn unlock_recipe(player: PlayerId, recipe: &str) -> bool
        => UnlockRecipe { player, recipe: recipe.into() } => Bool
}

host_fn! {
    /// Has `player` unlocked `recipe`? The read half of [`unlock_recipe`], for
    /// gating a mod's own hints or follow-up rewards.
    pub fn recipe_unlocked(player: PlayerId, recipe: &str) -> bool
        => RecipeUnlocked { player, recipe: recipe.into() } => Bool
}

host_fn! {
    /// Claim a SCALE on one of `player`'s engine quantities
    /// ([`PlayerAttribute`]): the engine keeps the base — a constant, a
    /// mode, a formula — and your claim multiplies it. `MoveSpeed` slows or
    /// hastes the body; `AttackCooldown` at `0.0` removes the engine's melee
    /// rate limit, for a pack whose own pacing already gates the hand.
    ///
    /// Every claimant gets a slot and the engine applies the PRODUCT, beside
    /// its own claim (the status effects' speed) — your scale and another
    /// pack's compose instead of stomping, which is also why the claim is a
    /// multiplier and never an absolute. `1.0` releases yours, `0.0` zeroes
    /// the quantity, finite values clamp into the attribute's own bound,
    /// non-finite is a hard error. Transient: re-state it on your own
    /// cadence. `false` = no such reachable session.
    ///
    /// SERVER only — every attribute is simulation the server enforces, so
    /// the answers are mirrored rather than predicted.
    pub fn set_player_attribute(player: PlayerId, attribute: PlayerAttribute, scale: f32) -> bool
        => SetPlayerAttribute { player, attribute, scale } => Bool
}

host_fn! {
    /// Claim a HELD-ITEM POSE on `player`, per hand — an extra Blockbench
    /// display transform composed onto whatever that hand already holds, in
    /// first person and on every observer's third-person body.
    ///
    /// Author it exactly as a `display` entry: rotation in DEGREES (X, Y, Z),
    /// translation in 1/16-BLOCK pixels, relative to the item's authored hold.
    /// One per view ([`HeldPose`]) — the two start from different authored
    /// poses. The off hand mirrors by Blockbench's own left-hand rule, and
    /// every held render kind wears it alike.
    ///
    /// `None` releases a hand; claims resolve last-wins in claimant order.
    /// Transient — re-publish on your own cadence; the client eases between
    /// updates, so a 20 Hz publisher still glides.
    ///
    /// Legal on the CLIENT for the LOCAL player ([`PlayerSnapshot::id`]), so
    /// the pose presents on the frame the input asks for it. Run the same
    /// predicate on both sides and the two answers cannot disagree.
    pub fn set_player_held_pose(
        player: PlayerId,
        main: Option<HeldPose>,
        off: Option<HeldPose>,
    ) -> bool => SetPlayerHeldPose { player, main, off } => Bool
}

host_fn! {
    /// Claim BONE OFFSETS on `player`'s body — rotate or shift named rig
    /// bones, composed onto whatever animation is already posing them (a walk
    /// cycle, a punch, a head-look).
    ///
    /// The body counterpart of [`set_player_held_pose`]: that poses what a
    /// hand is HOLDING, this poses the hand. An offset on a shoulder carries
    /// through the whole arm and everything in its fist.
    ///
    /// Name bones from [`bone`](mod_api::bone) — the arms especially, because
    /// the rig authors the MAIN hand's arm as the model's left. Degrees about
    /// the bone's posed pivot, translations in 1/16-block pixels. Every
    /// claimant's offsets apply; an empty list releases yours, and a name the
    /// rig lacks is dropped. Transient.
    ///
    /// Legal on the CLIENT for the LOCAL player, the same predicted path as
    /// [`set_player_held_pose`].
    pub fn set_player_bone_pose(player: PlayerId, bones: Vec<BonePoseData>) -> bool
        => SetPlayerBonePose { player, bones } => Bool
}

host_fn! {
    /// Take `player`'s current USE GESTURE — one press of the interact button —
    /// and keep it until they let go. `false` = no such reachable session.
    ///
    /// A gesture has at most one owner. Most interactions resolve inside it and
    /// leave it free, which is what lets a held button keep placing blocks;
    /// this is how a CONTINUOUS use says otherwise. While you hold it nothing
    /// else is offered the button, and [`PlayerSnapshot::holds_use`] answers
    /// `true` for you and nobody else — write the rule against THAT, never
    /// against the raw held button.
    ///
    /// Call it from a [`EventKind::UseUnclaimed`] handler — the fall-through
    /// fired once the whole interact chain has passed. Taking the press is not
    /// an interaction: nothing happened to the world and no hand jabs, so pose
    /// the body yourself.
    ///
    /// [`EventKind::UseUnclaimed`]: mod_api::EventKind::UseUnclaimed
    pub fn hold_use(player: PlayerId) -> bool => HoldUse { player } => Bool
}

host_fn! {
    /// Bar a set of [`BodyAction`]s on `player` — the claim for "these hands
    /// are busy". An empty list releases yours; `false` = no such reachable
    /// session.
    ///
    /// The sibling of [`set_player_speed_scale`], but resolved by UNION: two
    /// packs barring different things both get their way, and neither can
    /// un-bar the other's. Transient — re-state it from whatever tick system
    /// owns the rule.
    ///
    /// SERVER only, and MIRRORED to that player's client so their own
    /// prediction stops with it: a client still predicting a break the server
    /// will refuse shows a crack creeping up a block that never breaks.
    pub fn set_player_denied_actions(player: PlayerId, actions: Vec<BodyAction>) -> bool
        => SetPlayerDeniedActions { player, actions } => Bool
}

host_fn! {
    /// Take over some of a hand's ENGINE MOTIONS on `player`
    /// ([`HandMotion`]) — the claim that stops the engine playing its own
    /// copy of each named gesture, so the animation this mod publishes
    /// (poses and bone offsets per [`player_state`]`().swing` phase) is the
    /// whole motion, not a layer fighting the vanilla one.
    ///
    /// `Swing` silences the mining loop and the break/attack punches; `Jab`
    /// the soft use gesture. A claimed motion plays nothing of the engine's,
    /// so the claimant owes that hand an animation every frame the facts say
    /// it is happening; a motion left unclaimed keeps its engine default (a
    /// swing-only claimant's hand still jabs on a placement). The facts stay
    /// published exactly as before.
    ///
    /// An empty list releases a hand; a vanilla motion returns once no mod
    /// claims it (claims UNION across mods per motion, like the denied
    /// actions). CLIENT-legal for the LOCAL player, the predicted path of
    /// [`set_player_held_pose`] — pose the hands a round trip early by
    /// running the same rule on both sides.
    pub fn set_player_hand_motions(player: PlayerId, main: Vec<HandMotion>, off: Vec<HandMotion>) -> bool
        => SetPlayerHandMotions { player, main, off } => Bool
}
