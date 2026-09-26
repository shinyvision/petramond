//! Authoritative player state: health, knockback, teleports, items, effects,
//! inputs, progression, permissions, and the session roster.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::data::{EntityRef, PlayerAttribute};
use crate::ids::{ItemId, PlayerId};
use crate::legality::prelude::*;

host_domain! {
    /// Authoritative player state: health, knockback, teleports, items, effects,
    /// inputs, progression, permissions, and the session roster.
    PlayerCall {
        /// Damage `player` through the single engine funnel. The victim's global
        /// engine-owned i-frames and `player_damage_pre` apply. Queued; applied
        /// at the next action drain point (same tick, defined order); an
        /// unknown session is a silent no-op. → [`HostRet::Unit`](crate::HostRet::Unit).
        ///
        /// `attacker` names who the hit is landed for, exactly as on
        /// [`EntityCall::DamageMob`]: `None` is the mod's own damage
        /// ([`DamageSource::Mod`], `origin` spatial context only);
        /// `Some(EntityRef::Player(..))` is that player's melee strike — a
        /// `player_damage_pre` handler sees [`DamageSource::PlayerAttack`] with
        /// the `origin`, and an applied hit shoves the victim away from it like
        /// the engine's own hit does; `Some(EntityRef::Mob(..))` is that mob's.
        ///
        /// To KILL a player, pass their current health ([`PlayerCall::Players`])
        /// as `amount` — same funnel, and i-frames or a pre-event handler can
        /// still reject it. There is no separate kill call.
        ///
        /// [`DamageSource::Mod`]: crate::DamageSource::Mod
        /// [`DamageSource::PlayerAttack`]: crate::DamageSource::PlayerAttack
        DamagePlayer {
            player: PlayerId,
            amount: i32,
            origin: Option<[f64; 3]>,
            attacker: Option<EntityRef>,
        } => legal(SERVER, Sim, Write),
        /// Add a knockback impulse to the player's velocity on the tick (spectator
        /// no-op; a positive-y impulse reads as a launch). Non-finite components
        /// are rejected with [`HostRet::Err`](crate::HostRet::Err). → [`HostRet::Unit`](crate::HostRet::Unit).
        ApplyKnockback {
            impulse: [f32; 3],
        } => legal(SERVER, Sim, Write),
        /// Give the player `count` of an item (by registry NAME) through the normal
        /// inventory fill; whatever doesn't fit drops at the player's feet like any
        /// other overflow, carrying `data` as the stack's instance data (empty =
        /// plain; see [`ItemStackData::data`](crate::ItemStackData::data)). `false` = unknown name; a
        /// malformed `data` map is [`HostRet::Err`](crate::HostRet::Err). → [`HostRet::Bool`](crate::HostRet::Bool).
        GiveItem {
            item: String,
            count: u8,
            data: Vec<(String, Vec<u8>)>,
        } => legal(SERVER, Sim, Write),
        /// Overwrite the player's health (clamped to `0..=20` half-hearts),
        /// BYPASSING the damage funnel — this is the heal/set primitive, not a
        /// damage source (no events fire). → [`HostRet::Unit`](crate::HostRet::Unit).
        SetHealth {
            value: i32,
        } => legal(SERVER, Sim, Write),
        /// Move the player's feet to `pos`, clearing fall tracking so the
        /// teleport can never land as fall damage. Non-finite components are
        /// rejected with [`HostRet::Err`](crate::HostRet::Err). → [`HostRet::Unit`](crate::HostRet::Unit).
        Teleport {
            pos: [f64; 3],
        } => legal(SERVER, Sim, Write),
        /// Deliver one server-authored chat line to connected clients. Chat is
        /// not simulation state: the host sanitizes/`$[fg=…]` markup-parses
        /// `text` and ships a structured line out-of-band (not on `TickUpdate`).
        /// `targets: None` = every currently connected session; `Some(ids)` =
        /// those player ids only (unknown / already-left ids are ignored; at most
        /// 4096 entries — the sim batch cap). Empty
        /// / whitespace-only text is a no-op (`Bool(false)`). → [`HostRet::Bool`](crate::HostRet::Bool).
        ChatSend {
            text: String,
            targets: Option<Vec<PlayerId>>,
        } => legal(SERVER, Sim, Write),
        /// Grant the player the status effect registered under `key` (an
        /// `effects.json` row — engine `petramond:*` rows and every pack's rows alike)
        /// for `ticks` game ticks. An already-active effect is OVERWRITTEN with
        /// the new duration; `ticks == 0` REMOVES it (there is no separate remove
        /// call — the SDK's `effect_remove` is a wrapper for `ticks: 0`). Like
        /// `SetHealth` this is a state primitive: no events fire. →
        /// [`HostRet::Bool`](crate::HostRet::Bool) (`false` = unknown effect key).
        EffectApply {
            key: String,
            ticks: u32,
        } => legal(SERVER, Sim, Write),
        /// Read the player's active status effects, in application order. →
        /// [`HostRet::Effects`](crate::HostRet::Effects).
        EffectsActive => legal(SERVER, Sim, Read),
        /// Consume `count` units of the ACTING player's selected (held) stack,
        /// atomically, only when it holds `item` with at least `count` units —
        /// the consumption primitive for item uses that spend the item without
        /// placing a block (spawning an entity from `item_use_pre`). `false`
        /// consumed nothing (wrong/empty hand, short stack).
        /// → [`HostRet::Bool`](crate::HostRet::Bool).
        ConsumeHeld {
            item: ItemId,
            count: u32,
        } => legal(SERVER, Sim, Write),
        /// Swap ONE of the selected stack for `replacement` (by registry NAME) when
        /// the selected stack holds at least one of `item`. For a single-item stack
        /// the replacement lands in the same slot (the bucket empty/fill case); for
        /// larger stacks one unit is consumed and the replacement is given through
        /// normal inventory fill. `false` = wrong/empty hand, unknown replacement
        /// name, or no room for the replacement. → [`HostRet::Bool`](crate::HostRet::Bool).
        ReplaceHeldOne {
            item: ItemId,
            replacement: String,
        } => legal(SERVER, Sim, Write),
        /// One player's movement intent this tick, decomposed into the player's
        /// own yaw frame — how a vehicle mod reads what its driver is pressing.
        /// `None` = no such player connected. → [`HostRet::PlayerInput`](crate::HostRet::PlayerInput).
        PlayerInput {
            player_id: PlayerId,
        } => legal(SERVER, Sim, Read),
        /// Every connected player this tick, in session-id order (single player =
        /// one entry) — the multiplayer-aware "where is everyone" for spawn,
        /// ambience, and weather policy. → [`HostRet::Players`](crate::HostRet::Players).
        Players => legal(SERVER, Sim, Read),
        /// Unlock a crafting recipe for one player: it joins their browser at
        /// whatever station the recipe declares, and the server starts accepting
        /// it from them. Idempotent — `true` = this call is what unlocked it,
        /// `false` = already unlocked, no such recipe key, or no such player.
        /// Persists with the player. → [`HostRet::Bool`](crate::HostRet::Bool).
        ///
        /// The unlock is a CONSEQUENCE, not an event: call it from whatever
        /// handler decides the player has earned it (an `item_obtained`, a
        /// `mob_died`, the mod's own [`EmitEvent`](crate::CoreCall::EmitEvent)). A recipe
        /// nobody unlocks stays invisible, so a pack that authors recipes and no
        /// policy still gets the engine's ingredient-discovery default.
        ///
        /// Every dispatch reaches every connected session; an id that is not
        /// connected answers `false` (and logs it).
        UnlockRecipe {
            player: PlayerId,
            recipe: String,
        } => legal(SERVER, Sim, Write),
        /// Has `player` unlocked `recipe`? The read half of
        /// [`UnlockRecipe`](Self::UnlockRecipe), for gating a mod's own hints,
        /// GUIs, or follow-up rewards. `false` for an unknown recipe or player.
        /// → [`HostRet::Bool`](crate::HostRet::Bool).
        RecipeUnlocked {
            player: PlayerId,
            recipe: String,
        } => legal(SERVER, Sim, Read),
        /// The named session's currently HELD stack, instance data included —
        /// the per-player, per-stack read [`PlayerState`](crate::BodyCall::PlayerState)'s
        /// row-level `held` id cannot be: an augmented tool's `petramond:tool`
        /// override lives in the stack's data, and only containers exposed it
        /// before. `None` = empty hand or no such connected session.
        /// → [`HostRet::HeldStack`](crate::HostRet::HeldStack).
        PlayerHeld {
            player: PlayerId,
        } => legal(SERVER, Sim, Read),
        /// [`GiveItem`](Self::GiveItem) addressed to a NAMED session (the
        /// explicit-player addressing doctrine): fill that player's inventory,
        /// drop whatever doesn't fit at that player's feet, `data` as the
        /// stack's instance data. The delivery a machine owes a specific viewer
        /// — a transient panel returning its contents on close — where a tick
        /// system has no actor to give to. `false` = unknown item name or no such
        /// connected session
        /// (deliver another way — e.g. spawn at the machine); a malformed
        /// `data` map is [`HostRet::Err`](crate::HostRet::Err). → [`HostRet::Bool`](crate::HostRet::Bool).
        GiveItemTo {
            player: PlayerId,
            item: String,
            count: u8,
            data: Vec<(String, Vec<u8>)>,
        } => legal(SERVER, Sim, Write),
        /// Rewrite the INSTANCE DATA on the stack `player` is holding, iff that
        /// stack is still an `expect_item` carrying exactly `expect_data` — the
        /// compare half of a compare-and-set, over the VALUE being replaced and
        /// not merely the item's identity. A hand swapped, or the same stack
        /// re-stamped by another handler or another mod, between the mod's read
        /// and this write refuses rather than clobbers: two writers that both
        /// read one tool would otherwise silently drop one of the two updates.
        /// The write a wear/repair system needs: an augment record lives on the
        /// HELD tool's stack, and only a machine's own cells were mod-writable
        /// before. `expect_data` is the map the mod READ off the stack (empty =
        /// expect a plain stack); `data` is the FULL replacement map (empty
        /// clears it); count and item stay. `false` = empty/other hand, unknown
        /// `expect_item`, data that no longer matches `expect_data`, or no such
        /// connected session; a malformed `data` map is [`HostRet::Err`](crate::HostRet::Err).
        /// → [`HostRet::Bool`](crate::HostRet::Bool).
        SetPlayerHeldData {
            player: PlayerId,
            expect_item: String,
            expect_data: Vec<(String, Vec<u8>)>,
            data: Vec<(String, Vec<u8>)>,
        } => legal(SERVER, Sim, Write),
        /// Claim a SCALE on one of a body's engine quantities
        /// ([`PlayerAttribute`](crate::PlayerAttribute)): the engine keeps the
        /// base — a constant, a mode, a formula — and the claim multiplies it.
        ///
        /// A multiplier rather than an absolute, deliberately: every claimant
        /// gets its own slot and the engine applies the PRODUCT (beside its own
        /// claim, e.g. the speed-carrying status effects), so two packs' scales
        /// compose rather than stomping, and no claim can force "exactly X" over
        /// another's head — the same reason the denials union. `1.0` releases
        /// this claim, `0.0` zeroes the quantity (a rooted body, a cooldown-free
        /// hand), finite values clamp into the attribute's own bound and a
        /// non-finite one is a [`HostRet::Err`](crate::HostRet::Err). TRANSIENT: never saved, so
        /// re-state it on your own cadence (a per-tick system naturally does).
        ///
        /// Server only — every attribute is simulation the server enforces
        /// (movement speed is validated, the attack cooldown gates damage), so a
        /// client claiming one would be predicting its own permission.
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such reachable session).
        SetPlayerAttribute {
            player: PlayerId,
            attribute: PlayerAttribute,
            scale: f32,
        } => legal(SERVER, Sim, Write),
        /// Bar a set of [`BodyAction`](crate::BodyAction)s on one body — the claim
        /// for "these hands are busy". An empty list releases it.
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such reachable session).
        ///
        /// The sibling of [`SetPlayerAttribute`](Self::SetPlayerAttribute) but
        /// resolved by UNION, not product: two claimants barring different things
        /// both mean it, and one able to un-bar another's would make "may this
        /// body mine" depend on claimant order. TRANSIENT, and MIRRORED to the
        /// barred player's client so their own prediction stops with it — a client
        /// still predicting a break the server will refuse shows a crack creeping
        /// up a block that never breaks.
        ///
        /// Cancelling at [`EventKind::BlockBreakPre`](crate::EventKind::BlockBreakPre) is not a substitute: it
        /// refuses the break only at the END of the timer, after a second of crack
        /// animation, and a swing at the air has no pre-event at all. Barring the
        /// ACTION is the honest shape — the button simply does nothing, on both
        /// sides, for as long as the claim stands.
        ///
        /// [`EventKind::BlockBreakPre`]: crate::EventKind::BlockBreakPre
        SetPlayerDeniedActions {
            player: PlayerId,
            actions: Vec<crate::BodyAction>,
        } => legal(SERVER, Sim, Write),
        /// Remove `count` units of `item` (by registry NAME) from `player`'s
        /// inventory, atomically, in the one carried-slot layout
        /// [`PlayerInventory`](crate::BodyCall::PlayerInventory) publishes: the grid in slot
        /// order, then the off hand. `data` picks the variant: `None` takes from
        /// stacks sharing the FIRST matching stack's instance data (one variant
        /// leaves, never a blend); `Some(map)` takes only from stacks whose data
        /// is exactly `map` (empty = plain stacks). Nothing is removed unless
        /// the whole count is there. The spend half of a launch — an arrow
        /// leaving a quiver that is not the hand holding the bow. →
        /// [`HostRet::ItemStack`]: the taken stack (`count` units, the variant
        /// they carried), `None` = not enough, unknown item, or no such
        /// connected session; a malformed `data` map is [`HostRet::Err`](crate::HostRet::Err).
        TakeItem {
            player: PlayerId,
            item: String,
            count: u8,
            data: Option<Vec<(String, Vec<u8>)>>,
        } => legal(SERVER, Sim, Write),
        /// A connected player's lasting identity: their stable name and whether
        /// they are an operator. → [`HostRet::Identity`](crate::HostRet::Identity) (`None` = no such
        /// connected player). Server only.
        PlayerIdentity {
            player: PlayerId,
        } => legal(SERVER, Sim, Read),
        /// [`PlayerState`](crate::BodyCall::PlayerState) for a NAMED session.
        /// → [`HostRet::PlayerOf`](crate::HostRet::PlayerOf): `None` = no such connected session.
        PlayerStateOf {
            player: PlayerId,
        } => legal(SERVER, Sim, Read),
        /// [`ApplyKnockback`](Self::ApplyKnockback) on a named session.
        /// → [`HostRet::Bool`](crate::HostRet::Bool): `false` = no such connected session.
        ApplyKnockbackTo {
            player: PlayerId,
            impulse: [f32; 3],
        } => legal(SERVER, Sim, Write),
        /// [`SetHealth`](Self::SetHealth) on a named session.
        /// → [`HostRet::Bool`](crate::HostRet::Bool): `false` = no such connected session.
        SetHealthOf {
            player: PlayerId,
            value: i32,
        } => legal(SERVER, Sim, Write),
        /// [`Teleport`](Self::Teleport) a named session.
        /// → [`HostRet::Bool`](crate::HostRet::Bool): `false` = no such connected session.
        TeleportPlayer {
            player: PlayerId,
            pos: [f64; 3],
        } => legal(SERVER, Sim, Write),
        /// [`EffectApply`](Self::EffectApply) on a named session.
        /// → [`HostRet::Bool`](crate::HostRet::Bool): `false` = unknown effect or no such session.
        EffectApplyTo {
            player: PlayerId,
            key: String,
            ticks: u32,
        } => legal(SERVER, Sim, Write),
        /// [`EffectsActive`](Self::EffectsActive) of a named session.
        /// → [`HostRet::EffectsOf`](crate::HostRet::EffectsOf): `None` = no such connected session.
        EffectsActiveOf {
            player: PlayerId,
        } => legal(SERVER, Sim, Read),
        /// [`ConsumeHeld`](Self::ConsumeHeld) from a named session's acting
        /// hand. → [`HostRet::Bool`](crate::HostRet::Bool): `false` = consumed nothing (or no such
        /// session).
        ConsumeHeldBy {
            player: PlayerId,
            item: ItemId,
            count: u32,
        } => legal(SERVER, Sim, Write),
        /// [`ReplaceHeldOne`](Self::ReplaceHeldOne) in a named session's acting
        /// hand. → [`HostRet::Bool`](crate::HostRet::Bool): `false` = wrong/empty hand, unknown
        /// replacement, no room, or no such session.
        ReplaceHeldOneBy {
            player: PlayerId,
            item: ItemId,
            replacement: String,
        } => legal(SERVER, Sim, Write),
        /// [`PlayerCall::PlayerInput`] for many players in one crossing — how
        /// a vehicle tick reads every driver at once. At most
        /// [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) ids.
        /// → [`HostRet::PlayerInputs`](crate::HostRet::PlayerInputs), parallel
        /// to `player_ids`.
        PlayerInputs {
            player_ids: Vec<PlayerId>,
        } => legal(SERVER, Sim, Read),
    }
}
