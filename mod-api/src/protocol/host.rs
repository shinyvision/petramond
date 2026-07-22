use serde::{Deserialize, Serialize};

pub use super::guest::GuestCall;
use crate::client::{
    ClientCanvasElement, ClientOverlayAnchor, ClientSurfaceColumn, ClientSurfaceQuery,
    ClientTextRun,
};
use crate::data::{
    CollisionShape, EffectStateData, GuiValue, ItemInfoData, ItemStackData, LightData,
    MobAnimStateData, MobRidersData, MobSnapshot, MobTagLookup, MobTagValue, PlayerInputData,
    PlayerListEntry, PlayerSnapshot, RuntimeSide,
};
use crate::events::EventKind;
use crate::ids::{BlockId, ItemId, MobId, PlayerId};
use crate::sched::{AttachSide, Stage, WorldgenStage};

/// Guest → host: what a mod asks the engine for through `host_dispatch`.
/// One exhaustive match on the host routes each variant to its domain
/// handler (`src/modding/host/`).
///
/// The world-touching calls are sim-scoped: legal wherever a `SimCtx` is
/// published (`mod_init`, tick systems, event handlers), [`HostRet::Error`]
/// outside any guest dispatch.
///
/// # Item identity
///
/// Items have ONE mod-facing identity: the registry NAME (`"petramond:coal"`,
/// `"farming:wheat"` — the `item` field of an `items.json` row). Every
/// name-addressed call speaks it, and [`ItemStackData`] carries it. The
/// numeric [`ItemId`] is a session-scoped compact form for id-bearing
/// payloads (events, [`HostCall::ConsumeHeld`]); bridge the two with
/// [`HostCall::ResolveItem`] (name → id) and [`HostCall::ItemNames`]
/// (id → name), and never persist numeric ids. The `key` field on
/// `items.json` rows is engine-internal recipe plumbing and does not cross
/// the ABI.
///
/// # Mob addressing
///
/// A LIVE mob has ONE address: its stable session id
/// ([`MobSnapshot::id`], the `mob_id` field on every mob call and event
/// payload). It survives unrelated removals and is the key for cross-tick
/// mod state; the list `index` on [`MobSnapshot`] is only an intra-tick
/// join key between snapshots and is accepted by no call. Dead
/// (ragdolling) mobs are GONE to this surface — id-addressed reads answer
/// `None`, writes answer `false`, exactly the live set
/// [`HostCall::MobsInRadius`] enumerates. Mob SPECIES are keyed by their
/// `mobs.json` `key` string (`"petramond:sheep"`); the numeric [`MobId`]
/// is its session-scoped compact form in payloads — bridge with
/// [`HostCall::ResolveMob`] (key → id) and [`HostCall::MobNames`]
/// (id → key), and never persist either the numeric species id or a live
/// mob's session id.
///
/// # Player addressing
///
/// Every call or payload field that names a player carries the
/// [`PlayerId`] newtype EXPLICITLY ([`HostCall::PlayerInput`],
/// [`HostCall::MobMount`], [`HostCall::ChatSend`] targets, event payloads
/// like `InteractAttempt`/`PlayerDismounted`). This is the frozen rule for NEW
/// surface: a player-touching call takes a `player_id` — never a bare `u8`,
/// and never a new implicit-player call. The older single-player-era calls
/// ([`HostCall::PlayerState`], [`HostCall::DamagePlayer`],
/// [`HostCall::GiveItem`], [`HostCall::Teleport`], ...) address the ACTING
/// session's player as a documented default — the session whose dispatch is
/// running (the interacting player for event handlers, the host session for
/// global tick systems). They are the legacy exception, not the pattern;
/// their per-player reshape is pending, and enumerating sessions is already
/// explicit via [`HostCall::Players`].
///
/// # Batch bounds
///
/// Batched sim/registry calls (`GetBlocks`, `SetBlocks`, `ContainerGetMany`,
/// `ContainerSet` slots, the `*Names` reverse resolvers, `ChatSend` targets)
/// are capped at 4096 entries per call. Exceeding the cap is
/// [`HostRet::Error`] (SDK panic → mod disabled): a batch that size is a mod
/// bug, and the watchdog deliberately does not charge host-side work — the
/// cap is what keeps one call from stalling the tick. Client-instance calls
/// carry their own (tighter, per-frame) documented caps.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum HostCall {
    /// Log through the engine logger (mods have no stdout).
    Log {
        msg: String,
    },
    /// The current game tick (20 per second). → [`HostRet::U64`].
    CurrentTick,
    /// Next value of the mod's named deterministic RNG stream (SplitMix64,
    /// seeded from world seed + mod id + key). → [`HostRet::U64`].
    RngU64 {
        stream_key: String,
    },
    /// Attach a tick system. Legal ONLY during `mod_init`; the engine later
    /// dispatches [`GuestCall::TickSystem`] with `system_id` every tick.
    RegisterTickSystem {
        stage: Stage,
        attach: AttachSide,
        priority: i32,
        system_id: u32,
    },
    /// Register an event handler. Legal ONLY during `mod_init`; the engine
    /// later dispatches [`GuestCall::HandleEvent`] with `handler_id`.
    RegisterEventHandler {
        event: EventKind,
        priority: i32,
        handler_id: u32,
    },
    // --- blocks -------------------------------------------------------------
    /// The block at a world cell: `Some` (air included) when its section is
    /// loaded, `None` when unloaded / outside the vertical range.
    /// → [`HostRet::Block`].
    GetBlock {
        pos: [i32; 3],
    },
    /// Batched [`HostCall::GetBlock`], one result per position in order.
    /// At most 4096 positions per call (the sim batch cap — see "Batch
    /// bounds" on [`HostCall`]); more is [`HostRet::Error`].
    /// → [`HostRet::Blocks`].
    GetBlocks {
        positions: Vec<[i32; 3]>,
    },
    /// Set one block through the engine's full edit path (relight, neighbour
    /// updates, `block` events' world state all hold). `false` = the cell is
    /// unloaded / out of range. → [`HostRet::Bool`].
    SetBlock {
        pos: [i32; 3],
        block: BlockId,
    },
    /// Batched [`HostCall::SetBlock`]; applied in order, each through the full
    /// edit path. NOTE: every write pays its own relight/remesh of the 3×3×3
    /// section neighbourhood — huge batches are expensive; this batches the
    /// ABI crossing, not the world work. At most 4096 writes per call (the
    /// sim batch cap); more is [`HostRet::Error`].
    /// → [`HostRet::U64`] (cells actually set).
    SetBlocks {
        blocks: Vec<([i32; 3], BlockId)>,
    },
    /// Run the cell's block behavior `scheduled_tick` in `delay` game ticks
    /// (first schedule per cell wins, like water's flow checks).
    /// → [`HostRet::Unit`].
    ScheduleTick {
        pos: [i32; 3],
        delay: u64,
    },
    /// Whether the section owning the cell is loaded. → [`HostRet::Bool`].
    IsLoaded {
        pos: [i32; 3],
    },
    /// Cached light at a cell on the renderer's 6-bit scale (`0..=63`):
    /// combined = max(sky, block). `None` = section unloaded / streamed
    /// content not yet final — the [`HostCall::GetBlock`] contract ("state
    /// frozen, retry later"), never a fabricated open-sky fallback, so
    /// light-driven policy can never act on values the world does not hold.
    /// → [`HostRet::Light`].
    LightAt {
        pos: [i32; 3],
    },

    // --- entities -----------------------------------------------------------
    /// Spawn a mob by species key at `pos` (feet) facing `yaw`. With
    /// `checked: false` the spawn is unconditional (site fitness is the
    /// caller's business); `checked: true` spawns only when the COMPLETE
    /// declared body fits — every covered section loaded and stream-final, no
    /// terrain collision overlap, no live solid mob overlap — validated and
    /// inserted as one atomic sim operation (use it for player-placed bodies:
    /// a failed call mutates nothing, so the item can be refunded). The reply
    /// carries the newborn's STABLE id (`None` = unknown key, the mob cap, or
    /// a failed check) so the spawner can immediately tag/configure it.
    /// → [`HostRet::SpawnedMob`].
    SpawnMob {
        key: String,
        pos: [f32; 3],
        yaw: f32,
        checked: bool,
    },
    /// Snapshot the live mobs within `radius` (3-D, of feet positions) of
    /// `pos`. Deterministic order = the live set's storage order (spawn order,
    /// perturbed only by removals). Dead (ragdolling) mobs are excluded.
    /// → [`HostRet::Mobs`].
    MobsInRadius {
        pos: [f32; 3],
        radius: f32,
    },
    /// Damage the live mob `mob_id` through its global engine-owned i-frames
    /// and the `mob_damage_pre` pipeline. Mod damage is not an attack, so
    /// default knockback is not applied; `origin` is only spatial context for
    /// feedback/handlers. Applied at the next action drain point (same tick),
    /// so a handler cannot re-enter the bus; a mob gone by then is a silent
    /// no-op. → [`HostRet::Unit`].
    ///
    /// `feedback` composes the damage pipeline for THIS request; `None` uses
    /// the species' resolved `damage_feedback`. A pipeline without the
    /// `Immunity` component is damage-over-time (burn): neither blocked by
    /// the victim's active i-frame window nor granting one.
    DamageMob {
        mob_id: u64,
        amount: f32,
        origin: Option<[f32; 3]>,
        feedback: Option<crate::events::MobDamageFeedback>,
    },
    /// Remove the live mob `mob_id` from the world immediately (not saved,
    /// no death/loot). `false` = no such live mob. → [`HostRet::Bool`].
    DespawnMob {
        mob_id: u64,
    },
    /// Spawn `count` of an item (by registry NAME — the one mod-facing item
    /// identity, e.g. `"petramond:coal"`, `"farming:wheat"`) as a dropped-item
    /// entity at `pos`. `false` = unknown name / zero count. → [`HostRet::Bool`].
    SpawnItem {
        item: String,
        count: u8,
        pos: [f32; 3],
    },

    // --- player ---------------------------------------------------------------
    /// The player's current state. → [`HostRet::Player`].
    PlayerState,
    /// Damage the player through the single engine funnel. The victim's global
    /// engine-owned i-frames and `player_damage_pre` apply, with
    /// [`DamageSource::Mod`] carrying the calling mod's id. Queued; applied at
    /// the next action drain point (same tick, defined order). →
    /// [`HostRet::Unit`].
    ///
    /// To KILL the player, pass their current health ([`HostCall::PlayerState`])
    /// as `amount` — same funnel, and i-frames or a pre-event handler can still
    /// reject it. There is no separate kill call.
    ///
    /// [`DamageSource::Mod`]: crate::DamageSource::Mod
    DamagePlayer {
        amount: i32,
    },
    /// Add a knockback impulse to the player's velocity on the tick (spectator
    /// no-op; a positive-y impulse reads as a launch). Non-finite components
    /// are rejected with [`HostRet::Error`]. → [`HostRet::Unit`].
    ApplyKnockback {
        impulse: [f32; 3],
    },
    /// Give the player `count` of an item (by registry NAME) through the normal
    /// inventory fill; whatever doesn't fit drops at the player's feet like any
    /// other overflow. `false` = unknown name. → [`HostRet::Bool`].
    GiveItem {
        item: String,
        count: u8,
    },
    /// Overwrite the player's health (clamped to `0..=20` half-hearts),
    /// BYPASSING the damage funnel — this is the heal/set primitive, not a
    /// damage source (no events fire). → [`HostRet::Unit`].
    SetHealth {
        value: i32,
    },
    /// Move the player's feet to `pos`, clearing fall tracking so the
    /// teleport can never land as fall damage. Non-finite components are
    /// rejected with [`HostRet::Error`]. → [`HostRet::Unit`].
    Teleport {
        pos: [f32; 3],
    },

    // --- sound ----------------------------------------------------------------
    /// Play a sound by `sounds.json` key (namespaced for pack sounds), routed
    /// through the tick→presentation channel — the sim never touches audio.
    /// `pos` attenuates by the sound row's `attenuation_distance`; `None`
    /// plays at full volume. `false` = unknown key. → [`HostRet::Bool`].
    EmitSound {
        key: String,
        pos: Option<[f32; 3]>,
    },

    // --- persistent KV --------------------------------------------------------
    // Keys are namespaced. WRITES (set/delete) must use the calling mod's own
    // prefix or an exposed engine `petramond:*` key; READS may cross namespaces —
    // that is the cross-mod interop surface (core day/night publishes, zombies
    // reads). Limits: key ≤ 256 bytes, value ≤ 64 KiB; violations return
    // `HostRet::Error`.
    /// World KV (persists in `level.dat`). → [`HostRet::Bytes`].
    WorldKvGet {
        key: String,
    },
    /// → [`HostRet::Unit`].
    WorldKvSet {
        key: String,
        value: Vec<u8>,
    },
    /// → [`HostRet::Bool`] (whether the key was present).
    WorldKvDelete {
        key: String,
    },
    /// Per-cell KV riding the cell's section save record (`pos` is a world
    /// block position). `Bytes(None)` when absent OR the section is unloaded.
    /// → [`HostRet::Bytes`].
    SectionKvGet {
        pos: [i32; 3],
        key: String,
    },
    /// `false` = the section is unloaded (nothing stored). Cell KV is
    /// per-BLOCK state: breaking/replacing the cell's block clears it (a
    /// `SwapModelBlock` flip carries it across). → [`HostRet::Bool`].
    SectionKvSet {
        pos: [i32; 3],
        key: String,
        value: Vec<u8>,
    },
    /// → [`HostRet::Bool`] (whether the key was present).
    SectionKvDelete {
        pos: [i32; 3],
        key: String,
    },
    /// Per-mob tag map: typed key/value pairs attached to a live mob instance.
    /// → [`HostRet::MobTag`] carrying a [`MobTagLookup`]:
    /// [`MissingMob`](MobTagLookup::MissingMob) for a dead/absent mob,
    /// [`Absent`](MobTagLookup::Absent) for a live mob not carrying the key.
    MobTagGet {
        mob_id: u64,
        key: String,
    },
    /// `false` = no such live mob, or the mob's tag map is full (32 entries)
    /// and `key` would be a NEW one — replacing an existing key always
    /// succeeds. → [`HostRet::Bool`].
    MobTagSet {
        mob_id: u64,
        key: String,
        value: MobTagValue,
    },
    /// → [`HostRet::Bool`] (whether the key was present).
    MobTagDelete {
        mob_id: u64,
        key: String,
    },

    // --- registry queries (see also the reverse resolvers appended at the
    // end) + worldgen hooks ---------------------------------------------------
    /// Resolve a block registry NAME (`"petramond:stone"`, `"kitchen:oven"`) to
    /// its session-scoped runtime id. Registry-only, needs no simulation
    /// context — legal on ANY instance, any time (worldgen and client
    /// instances included). `None` = not registered (a typo'd or absent pack —
    /// degrade gracefully, don't panic). → [`HostRet::Block`].
    ResolveBlock {
        name: String,
    },
    /// Register a worldgen FEATURE that runs after `stage` (typically
    /// [`WorldgenStage::Trees`], the end of the pipeline). Legal ONLY during
    /// `mod_init`; `stage == Climate` is rejected (features write blocks;
    /// climate is column-level). The engine later dispatches
    /// [`GuestCall::GenFeature`] once per generated 16³ section, on worldgen
    /// worker threads — see the determinism contract on [`GuestCall::GenFeature`].
    /// → [`HostRet::Unit`].
    RegisterWorldgenFeature {
        feature_id: u32,
        stage: WorldgenStage,
    },
    /// REPLACE one engine worldgen stage. Legal ONLY during `mod_init`. The
    /// engine dispatches [`GuestCall::GenStage`] instead of running its own
    /// stage. If several mods replace the same stage, the LAST in load order
    /// wins (logged). A failing replacement falls back to the ENGINE stage.
    /// → [`HostRet::Unit`].
    RegisterStageReplacement {
        stage: WorldgenStage,
        callback_id: u32,
    },
    /// Replace the WHOLE generator: shorthand for replacing every stage with
    /// `callback_id` (the guest switches on the dispatched `stage`). Same
    /// window, conflict, and fallback rules as
    /// [`HostCall::RegisterStageReplacement`]. → [`HostRet::Unit`].
    RegisterGenerator {
        callback_id: u32,
    },

    // --- mod GUIs ---------------------------------------------------------------
    /// Write a key of the open GUI session's state map (tick-owned; the
    /// renderer reads a snapshot per frame). Keys are mod-local — the map
    /// belongs to one GUI session and is cleared on open/close. Sim-scoped.
    /// → [`HostRet::Unit`].
    GuiStateSet {
        key: String,
        value: GuiValue,
    },
    /// Read a key of the GUI state map (`None` = absent). Sim-scoped.
    /// → [`HostRet::GuiValue`].
    GuiStateGet {
        key: String,
    },
    /// Ask the app shell to open the mod GUI registered under `kind_key`
    /// (`"wheel:wheel"` — a baked manifest or `open_gui` block row must have
    /// registered it). Queued like [`HostCall::DamagePlayer`]; the screen
    /// opens after this tick, only from gameplay (an already-open menu drops
    /// the request). `false` = unknown / non-mod kind. → [`HostRet::Bool`].
    GuiOpen {
        kind_key: String,
    },
    /// Close the open mod GUI (a no-op if none is open — engine containers
    /// are not closable from mods). Queued like [`HostCall::GuiOpen`].
    /// → [`HostRet::Unit`].
    GuiClose,

    /// Deliver one server-authored chat line to connected clients. Chat is
    /// not simulation state: the host sanitizes/`$[fg=…]` markup-parses
    /// `text` and ships a structured line out-of-band (not on `TickUpdate`).
    /// `targets: None` = every currently connected session; `Some(ids)` =
    /// those player ids only (unknown / already-left ids are ignored; at most
    /// 4096 entries — the sim batch cap). Empty
    /// / whitespace-only text is a no-op (`Bool(false)`). → [`HostRet::Bool`].
    ChatSend {
        text: String,
        targets: Option<Vec<PlayerId>>,
    },

    // --- Bugfix round 1 (audio): spatial mod sounds -----------------------
    /// Start a positional sound at a fixed world position. The host resolves
    /// `key` through `sounds.json`, queues a deterministic presentation
    /// command, and returns a session sound handle. `0` means the key was
    /// unknown or the parameters were invalid, so no sound was queued.
    /// `volume` is a linear multiplier, `pitch` is playback speed, and travel
    /// distance comes from the sound row's `attenuation_distance`.
    /// → [`HostRet::U64`].
    SoundPlayAt {
        key: String,
        pos: [f32; 3],
        volume: f32,
        pitch: f32,
    },
    /// Start a positional sound pinned to a live mob's stable [`MobSnapshot::id`].
    /// The app/audio side follows that mob's per-frame presentation position; if
    /// the mob despawns, the sound finishes at its last known position. Returns
    /// `0` when the sound key or mob id is unknown, or parameters are invalid.
    /// Travel distance comes from the sound row's `attenuation_distance`.
    /// → [`HostRet::U64`].
    SoundPlayOnMob {
        mob_id: u64,
        key: String,
        volume: f32,
        pitch: f32,
    },
    /// Stop a spatial sound previously started by this session handle. Unknown
    /// handles are a no-op. → [`HostRet::Unit`].
    SoundStop {
        handle: u64,
    },

    // --- Shader parameters ------------------------------------------------
    /// Set one named visual shader parameter (`vec4<f32>`). Mods may write
    /// their own `mod_id:name` keys or exposed engine `petramond:*` keys; active
    /// shader packs map keys onto fixed GPU slots. Not persisted: re-apply it
    /// from mod state on load.
    /// → [`HostRet::Unit`].
    ShaderSetParam {
        key: String,
        value: [f32; 4],
    },

    // --- Hostile spawning -------------------------------------------------
    /// Register a hostile-spawn callback. The core engine supplies candidate
    /// sites and enforces caps/body fit; the callback returns a hostile mob key
    /// if this mod wants to spawn something there. Legal ONLY during `mod_init`.
    /// → [`HostRet::Unit`].
    RegisterHostileSpawner {
        callback_id: u32,
        priority: i32,
    },

    // --- block behaviors --------------------------------------------------------
    /// Register the reactive behavior for block rows whose `blocks.json`
    /// `behavior` field is `key` — a `mod_id:name` owned by THIS pack. The
    /// engine then dispatches [`GuestCall::BlockBehavior`] with `callback_id`
    /// for every hook that fires on such a block. Legal ONLY during
    /// `mod_init`. → [`HostRet::Unit`].
    RegisterBlockBehavior {
        key: String,
        callback_id: u32,
    },

    // --- Scripted AI nodes (landed 2026-07-06) ------------------------------
    /// Register the scripted AI node for `mobs.json` brain rows whose `node`
    /// key is `key` — a `mod_id:name` owned by THIS pack. The engine then
    /// dispatches [`GuestCall::AiNode`] with `callback_id` once per owning
    /// mob per game tick. Legal ONLY during `mod_init`. → [`HostRet::Unit`].
    RegisterAiNode {
        key: String,
        callback_id: u32,
    },

    // --- Mod container slots (landed 2026-07-07) ----------------------------
    /// Read every slot of the mod container at `pos` (the engine-backed item
    /// storage behind a mod GUI document's `container` role slots; multi-cell
    /// model blocks key it at the group's base cell — the `block_placed`
    /// anchor). `None` when the section is unloaded or no container exists
    /// there yet (one is created when the GUI first opens, or by the first
    /// `ContainerSet`). → [`HostRet::ContainerSlots`].
    ContainerGet {
        pos: [i32; 3],
    },
    /// Write container slots at `pos` as `(slot index, stack)` entries, batched
    /// per the message-level ABI rule. Creates/grows the container as needed
    /// (never shrinks; slot indices past the engine cap are rejected). The
    /// block at `pos` must be registered to THIS mod's namespace — a mod owns
    /// only its own blocks' containers, ANY of them, decorative or not (reads
    /// may cross namespaces). Multi-cell model blocks canonicalize to the
    /// group anchor, so writing through any footprint cell edits the one
    /// container the GUI shows. Counts past the item's stack cap are CLAMPED
    /// to it — size against `ItemInfo.max_stack` if the overflow matters.
    /// At most 4096 slot entries per call (the sim batch cap); more is
    /// [`HostRet::Error`]. →
    /// [`HostRet::Bool`] (`false` = unloaded, or an unknown item name — the
    /// batch is not applied).
    ContainerSet {
        pos: [i32; 3],
        slots: Vec<(u32, Option<ItemStackData>)>,
    },
    /// Read one item's registry row (by registry NAME): the same
    /// [`ItemInfoData`] fields engine mechanics read, so mod logic (a
    /// fuel-fired oven, a filtering hopper, a tool gate) composes with
    /// pack-added items for free. Registry-only like
    /// [`HostCall::ResolveItem`]: legal on any instance, any time; row data is
    /// session-stable — cache it mod-side. `None` = unknown name. →
    /// [`HostRet::ItemInfo`].
    ItemInfo {
        item: String,
    },
    /// The loaded machine-processing result for one input item (by registry
    /// NAME) under a recipe `class` (`"petramond:smelting"` = the furnace's
    /// table; a mod machine names its own, e.g. `"kitchen:cooking"`), from the
    /// same layered `recipes.json` catalog engine machines cook from — any
    /// pack's rows for that class included. `None` = no recipe. →
    /// [`HostRet::ItemStack`].
    RecipeResult {
        class: String,
        item: String,
    },

    // --- Player status effects (landed 2026-07-07) --------------------------
    /// Grant the player the status effect registered under `key` (an
    /// `effects.json` row — engine `petramond:*` rows and every pack's rows alike)
    /// for `ticks` game ticks. An already-active effect is OVERWRITTEN with
    /// the new duration; `ticks == 0` REMOVES it (there is no separate remove
    /// call — the SDK's `effect_remove` is a wrapper for `ticks: 0`). Like
    /// `SetHealth` this is a state primitive: no events fire. →
    /// [`HostRet::Bool`] (`false` = unknown effect key).
    EffectApply {
        key: String,
        ticks: u32,
    },
    /// Read the player's active status effects, in application order. →
    /// [`HostRet::Effects`].
    EffectsActive,

    // --- Model-block state swap (landed 2026-07-07) --------------------------
    /// Swap the placed multi-cell MODEL block group at `pos` (any of its
    /// cells) to `block` — another model block sharing the exact same oriented
    /// footprint, e.g. the lit/unlit variants of a machine. Ids are rewritten
    /// in place: the engine-backed container, facing, and section cell KV all
    /// survive, and the region relights (an emission difference glows like a
    /// furnace lighting). BOTH blocks must be registered to THIS mod's
    /// namespace. → [`HostRet::Bool`] (`false` = not a model group there,
    /// footprint mismatch, or unloaded).
    SwapModelBlock {
        pos: [i32; 3],
        block: BlockId,
    },

    /// Batched [`ContainerGet`](Self::ContainerGet): every listed position's
    /// container slots in ONE crossing. A machine mod's tick loop MUST read
    /// its placed machines through this (like `GetBlocks`), never loop
    /// `ContainerGet` per machine — the per-block-per-tick hot-loop rule.
    /// At most 4096 positions per call (the sim batch cap); more is
    /// [`HostRet::Error`]. → [`HostRet::Containers`], parallel to the
    /// positions.
    ContainerGetMany {
        positions: Vec<[i32; 3]>,
    },

    // --- Mob particle emitters (landed 2026-07-10) ---------------------------
    /// Toggle one KEYED particle-emitter bundle on the live mob `mob_id`.
    /// `key` names a `particle_emitters.json` catalog row (engine
    /// `petramond:*` rows — `petramond:burn_light`, `petramond:burn_great` —
    /// and every pack's rows alike, the same cross-namespace rule as
    /// effects): one or more particle rows plus an optional body tint. The
    /// active set (≤ 4 per mob) is presentation-only, replicates to every
    /// client, survives death (a corpse keeps its already-active effects
    /// through the ragdoll — though a corpse can no longer be addressed), and
    /// is NOT persisted: the owning mod re-derives it, e.g. from its own
    /// per-mob state. → [`HostRet::Bool`] (`false` = unknown/dead mob,
    /// unregistered key, or the mob's active set is full).
    MobEmitterSet {
        mob_id: u64,
        key: String,
        active: bool,
    },
    /// Fire a ONE-SHOT particle burst: `key` names a `particle_emitters.json`
    /// BURST bundle (e.g. the core `petramond:water_splash`), spawned at `pos`
    /// for every client. `intensity` scales the particle count through the
    /// bundle's `count_per_intensity` (the core water splash passes blocks
    /// fallen). Fire-and-forget presentation, like `EmitSound`. →
    /// [`HostRet::Bool`] (`false` = unknown key or not a burst bundle).
    EmitterBurst {
        key: String,
        pos: [f32; 3],
        intensity: f32,
    },

    // --- Presentation-only client modules ---------------------------------
    /// Identify this isolated module instance. → [`HostRet::RuntimeSide`].
    RuntimeSide,
    /// Register an always-on physical-pixel overlay image. Legal only from a
    /// client instance during `mod_init`; the image may be published later.
    /// `margin` and `display_size` are physical screen pixels. →
    /// [`HostRet::Unit`].
    ClientRegisterOverlay {
        image_key: String,
        anchor: ClientOverlayAnchor,
        margin: [u16; 2],
        display_size: [u16; 2],
    },
    /// Register one REMAPPABLE key action: a stable bare `id` (the player's
    /// remap persists under `mod_id:id`), a display `label` for the Options →
    /// Controls screen (listed under the pack's name), the DEFAULT physical
    /// key (`"key_m"`, `"digit_1"`, …), and the opaque `action_id` delivered
    /// back in ClientKey events. Defaults colliding with an engine default are
    /// rejected by the app. Legal only during client `mod_init`. →
    /// [`HostRet::Unit`].
    ClientRegisterKey {
        id: String,
        label: String,
        key: String,
        action_id: u32,
    },
    /// Read whole surface chunk columns from the client replica, revision
    /// gated: a column whose host revision still equals the query's `revision`
    /// replies without cell bytes, so a steady-state resample costs near
    /// nothing. The reply is parallel to `queries`; `None` = column unknown to
    /// the replica. Query count is host-capped. →
    /// [`HostRet::ClientSurfaceColumns`].
    ClientSurfaceColumns {
        queries: Vec<ClientSurfaceQuery>,
    },
    /// Write/read the client module's document-binding state. Keys must use
    /// the caller's namespace. → [`HostRet::Unit`] / [`HostRet::GuiValue`].
    ClientUiStateSet {
        key: String,
        value: GuiValue,
    },
    ClientUiStateGet {
        key: String,
    },
    /// Publish an RGBA8 image for document nodes, physical overlays, or modal
    /// canvases. The key is namespaced and the host caps dimensions/bytes.
    /// Re-publishing the same key replaces it atomically for the next frame.
    /// → [`HostRet::Unit`].
    ClientImageSet {
        key: String,
        width: u16,
        height: u16,
        #[serde(with = "serde_bytes")]
        rgba: Vec<u8>,
    },
    /// Measure a single-line run with the host's shared text subsystem. The
    /// returned size uses physical pixels after applying `scale`. →
    /// [`HostRet::ClientTextSize`].
    ClientTextMeasure {
        text: String,
        scale: u8,
    },
    /// Draw ordered text runs into an existing namespaced client image. This
    /// is a generic image/text capability: canvases, overlays, and GUI-fed
    /// images all use the same host glyphs and metrics. → [`HostRet::Unit`].
    ClientImageDrawTexts {
        key: String,
        runs: Vec<ClientTextRun>,
    },
    /// Request a client-owned GUI document open/close. These screens release
    /// the cursor but keep the replicated world running. → [`HostRet::Bool`]
    /// / [`HostRet::Unit`].
    ClientGuiOpen {
        kind_key: String,
    },
    ClientGuiClose,
    /// Open/close a modal, centered physical-pixel canvas. While open,
    /// the cursor is released, gameplay input is gated, and pointer events are
    /// dispatched through [`GuestCall::ClientCanvas`]. → [`HostRet::Bool`] /
    /// [`HostRet::Unit`].
    ClientCanvasOpen {
        canvas_key: String,
        size: [u16; 2],
    },
    ClientCanvasClose,
    /// Replace one canvas's retained, ordered scene. Image keys and the canvas
    /// key must belong to the caller. Ordinary panning must use
    /// [`HostCall::ClientCanvasViewSet`] instead. → [`HostRet::Unit`].
    ClientCanvasSceneSet {
        canvas_key: String,
        elements: Vec<ClientCanvasElement>,
    },
    /// Change only the retained scene's logical-pixel translation. This is the
    /// hot path for panning and never republishes image bytes or scene nodes.
    /// → [`HostRet::Unit`].
    ClientCanvasViewSet {
        canvas_key: String,
        offset: [f32; 2],
    },
    /// Read a bounded batch of exact sandboxed client-storage keys. Results
    /// are parallel to `keys`; `None` means absent. Storage is scoped by
    /// server/world + mod id and inaccessible to other mods. This exact-key
    /// shape lets large spatial stores page only their working set. →
    /// [`HostRet::ClientStorageValues`].
    ClientStorageGetMany {
        keys: Vec<String>,
    },
    /// Write a batch of sandboxed client-storage entries, committing each
    /// entry atomically. This is the hot-loop shape for explored map tiles;
    /// never cross once per tile.
    /// Keys must use the caller's namespace. → [`HostRet::Bool`].
    ClientStorageSetMany {
        entries: Vec<(String, serde_bytes::ByteBuf)>,
    },
    /// Resolve an item registry NAME to this session's numeric id, or `None`
    /// for an unknown name. Registry-only (no world access): legal on any
    /// instance, any time — the [`HostCall::ResolveBlock`] contract. This is
    /// how a mod identifies its own items in id-bearing event payloads
    /// (e.g. `item_use_pre`) without persisting numeric ids. The reverse
    /// direction is [`HostCall::ItemNames`].
    /// → [`HostRet::Item`].
    ResolveItem {
        name: String,
    },
    /// Overwrite one rectangle of an existing namespaced client image in
    /// place (`origin`/`size` in image pixels, `rgba` = `size` pixels of
    /// RGBA8). The partial-update companion to [`HostCall::ClientImageSet`]:
    /// spatial clients refresh an invalidated region without re-publishing
    /// the whole image. → [`HostRet::Unit`].
    ClientImageBlit {
        key: String,
        origin: [u16; 2],
        size: [u16; 2],
        #[serde(with = "serde_bytes")]
        rgba: Vec<u8>,
    },
    /// Begin an ASYNCHRONOUS read of a bounded batch of exact sandboxed
    /// client-storage keys: the filesystem work runs on the background
    /// storage worker, so a slow disk delays the result instead of the
    /// frame. Ordered after already-queued writes (read-your-writes). Key
    /// rules and caps match [`HostCall::ClientStorageGetMany`]; a bounded
    /// number of tickets may be outstanding at once. This is the REQUIRED
    /// path for bulk spatial reads — the synchronous form is for small
    /// startup/edit reads. → [`HostRet::U64`] (the ticket).
    ClientStorageReadBegin {
        keys: Vec<String>,
    },
    /// Poll an asynchronous read begun by
    /// [`HostCall::ClientStorageReadBegin`]. `Some(values)` (parallel to the
    /// begun keys, `None` entry = absent) consumes the ticket; `None` means
    /// still in flight — poll again next frame. Polling an unknown or
    /// already-consumed ticket is an error.
    /// → [`HostRet::ClientStorageRead`].
    ClientStorageReadPoll {
        ticket: u64,
    },
    /// Consume `count` units of the ACTING player's selected (held) stack,
    /// atomically, only when it holds `item` with at least `count` units —
    /// the consumption primitive for item uses that spend the item without
    /// placing a block (spawning an entity from `item_use_pre`). `false`
    /// consumed nothing (wrong/empty hand, short stack).
    /// → [`HostRet::Bool`].
    ConsumeHeld {
        item: ItemId,
        count: u32,
    },
    /// Swap ONE of the selected stack for `replacement` (by registry NAME) when
    /// the selected stack holds at least one of `item`. For a single-item stack
    /// the replacement lands in the same slot (the bucket empty/fill case); for
    /// larger stacks one unit is consumed and the replacement is given through
    /// normal inventory fill. `false` = wrong/empty hand, unknown replacement
    /// name, or no room for the replacement. → [`HostRet::Bool`].
    ReplaceHeldOne {
        item: ItemId,
        replacement: String,
    },
    /// Seat player `player_id` in `seat` of the live mob `mob_id` (stable
    /// id). Validated by the engine: the mob is alive and its species row
    /// declares that seat (`seats` in `mobs.json`), the seat is free, and the
    /// player is not already mounted. WHO may sit WHERE is the calling mod's
    /// policy — usually decided in its `interact_attempt` handler. From this tick
    /// the engine slaves the rider to the seat; every detach path announces
    /// [`EventKind::PlayerDismounted`]. → [`HostRet::Bool`].
    ///
    /// [`EventKind::PlayerDismounted`]: crate::EventKind::PlayerDismounted
    MobMount {
        mob_id: u64,
        player_id: PlayerId,
        seat: u8,
    },
    /// Unseat `player_id` from whatever they ride (the mod-initiated detach;
    /// the engine's own valves — sneak gesture, death, despawn — detach
    /// without this call). `false` = they were not mounted.
    /// → [`HostRet::Bool`].
    MobDismount {
        player_id: PlayerId,
    },
    /// The declared seat capacity and every rider of the live mob `mob_id`,
    /// in player-id order. `None` = no such live mob, which is distinct from
    /// a live mob with zero seats or riders. → [`HostRet::Riders`].
    MobRiders {
        mob_id: u64,
    },
    /// Drive the live mob `mob_id` kinematically for THIS tick: `vel` is a
    /// horizontal world-space velocity (m/s) that replaces the brain's wish
    /// locomotion (vertical physics — gravity, water buoyancy — and collision
    /// stay engine-owned), and `yaw`, when present, sets the absolute facing
    /// (mob convention: yaw `0` faces `-Z`, facing `(-sin yaw, 0, -cos yaw)`).
    /// Like the wish it is an intent, not a state: re-issue it every tick
    /// (friction, steering feel, and control policy are the driving mod's) —
    /// a mod that stops calling leaves the mob to its brain. Knockback
    /// stagger overrides the drive for its duration. `false` = unknown or
    /// dead mob. → [`HostRet::Bool`].
    MobDrive {
        mob_id: u64,
        vel: [f32; 2],
        yaw: Option<f32>,
    },
    /// Toggle a NAMED model animation on the live mob `mob_id` — the
    /// animation sibling of [`HostCall::MobEmitterSet`]: presentation-only,
    /// at most 4 active per mob, replicated, never persisted (the owning mod
    /// re-derives it). Each active animation LAYERS over the walk/idle/rest
    /// base pose with its OWN self-clocked phase (activation starts it at
    /// phase 0, rate 1) — drive the playback with
    /// [`HostCall::MobAnimRate`]. `anim` is an animation name from the mob's
    /// own `.bbmodel`; unknown names are accepted and draw nothing (the sim
    /// never loads models — same forgiveness as a disabled pack). `false` =
    /// unknown mob or the per-mob cap. → [`HostRet::Bool`].
    MobAnimSet {
        mob_id: u64,
        anim: String,
        active: bool,
    },
    /// Set the PLAYBACK RATE of an active named animation on the live mob
    /// `mob_id` (see [`HostCall::MobAnimSet`]): its phase advances by
    /// `rate` animation-seconds per real second — `1.0` plays, `0.0` FREEZES
    /// mid-stroke exactly where it is (an oar pauses in place, never snaps
    /// home), negative plays in reverse. Cancels an in-flight
    /// [`HostCall::MobAnimSeek`]. Code-driven playback over an authored
    /// clip: the motion's SHAPE stays tunable in Blockbench, the mod owns
    /// play/pause/reverse/speed. `false` = unknown mob or the anim is not
    /// active. → [`HostRet::Bool`].
    MobAnimRate {
        mob_id: u64,
        anim: String,
        rate: f32,
    },
    /// SEEK an active named animation to the absolute `phase` at `|rate|`
    /// animation-seconds per second: the layer's phase approaches the target
    /// DIRECTLY (no modulo — the caller picks the nearest-cycle target for a
    /// shortest-path return), lands on it EXACTLY, and holds (rate 0). How
    /// an oar settles gently back onto its authored pose from wherever the
    /// stroke stopped. A [`HostCall::MobAnimRate`] cancels the seek. `false`
    /// = unknown mob or the anim is not active. → [`HostRet::Bool`].
    MobAnimSeek {
        mob_id: u64,
        anim: String,
        phase: f32,
        rate: f32,
    },
    /// One player's movement intent this tick, decomposed into the player's
    /// own yaw frame — how a vehicle mod reads what its driver is pressing.
    /// `None` = no such player connected. → [`HostRet::PlayerInput`].
    PlayerInput {
        player_id: PlayerId,
    },
    /// Read the authoritative playback state of active named animation
    /// `anim` on live mob `mob_id`. `None` = missing/dead mob or inactive
    /// animation. This is the source of truth for control policy that needs
    /// the current phase (for example, choosing a nearest-cycle seek target).
    /// → [`HostRet::MobAnimState`].
    MobAnimState {
        mob_id: u64,
        anim: String,
    },
    /// The loaded column's biome id at world `pos = [x, z]` (vocabulary:
    /// [`crate::biome`]). `None` = column unloaded. → [`HostRet::MaybeByte`].
    BiomeAt {
        pos: [i32; 2],
    },
    /// The Y of the topmost movement-blocking block of the loaded column at
    /// world `pos = [x, z]` — real footing; walk-through cover (tall grass,
    /// snow layers, water) is skipped. `None` = unloaded, all-air column, or
    /// the found footing is not yet STREAM-FINAL (retry later, like a block
    /// read). Caveat: finality is checked at the found cell — a saved build
    /// HIGHER in the column that has not streamed in yet is not visible to
    /// this scan, so treat the answer as provisional during join streaming.
    /// → [`HostRet::MaybeI32`].
    SurfaceYAt {
        pos: [i32; 2],
    },
    /// Every connected player this tick, in session-id order (single player =
    /// one entry) — the multiplayer-aware "where is everyone" for spawn,
    /// ambience, and weather policy. → [`HostRet::Players`].
    Players,
    /// CLIENT: read named shader params from the replica's replicated visual
    /// environment (the state sim mods publish with
    /// [`HostCall::ShaderSetParam`]) — how a client instance sees the same
    /// values the renderer does. At most 16 keys per call; the reply is
    /// parallel (`None` = param not present). → [`HostRet::EnvParams`].
    ClientEnvParams {
        keys: Vec<String>,
    },
    /// CLIENT: the replica column's biome id at world `pos = [x, z]`
    /// (vocabulary: [`crate::biome`]). `None` = column unknown to the
    /// replica. → [`HostRet::MaybeByte`].
    ClientBiomeAt {
        pos: [i32; 2],
    },
    /// CLIENT: drive an `ambient` particle bundle (a camera-following
    /// precipitation/ambience volume from `particle_emitters.json`) at
    /// `intensity` (clamped to `0..=1` — 1 is the bundle's full `max_count`
    /// density; `0` retires it; the engine eases changes so weather never
    /// pops) advected by `wind` (blocks/s). Per-client presentation only —
    /// never simulated, never replicated. `false` = unknown key or not an
    /// ambient bundle. → [`HostRet::Bool`].
    ClientAmbientSet {
        key: String,
        intensity: f32,
        wind: [f32; 2],
    },
    /// CLIENT: play this mod's looping sound `key` (a `sounds.json` key) at
    /// `gain` (`0` stops it; the engine eases changes so ambience never
    /// pops). Non-spatial, client-local. `false` = unknown sound key.
    /// → [`HostRet::Bool`].
    ClientLoopSet {
        key: String,
        gain: f32,
    },
    /// CLIENT: set this mod's post-process MOOD — a subtle whole-screen
    /// `darken` and `desaturate` (each clamped to `0..=0.5`; deliberately
    /// incapable of blacking out the screen) applied by the grade pass and
    /// EASED engine-side, so weather/ambience moods breathe instead of
    /// popping. Pure presentation: no light value changes, so light-driven
    /// gameplay (mob spawning) is untouched. Multiple mods combine by MAX
    /// per component. Rides the grade pass, so it is invisible in the
    /// grade-off configuration. → [`HostRet::Bool`] (always `true`).
    ClientMoodSet {
        darken: f32,
        desaturate: f32,
    },
    /// CLIENT: read replica block ids at world `positions`, reply parallel
    /// to the request. `None` = cell unknown to the replica (section
    /// unloaded, or its streamed content not yet final) — treat exactly like
    /// an unloaded server-side read: state frozen, retry later. Bounded
    /// batch (512 positions per call). → [`HostRet::Blocks`].
    ClientBlocksAt {
        positions: Vec<[i32; 3]>,
    },
    /// Every registered block carrying `tag`, in id order. Registry-only
    /// like [`HostCall::ResolveBlock`] — legal on any instance, any time.
    /// Engine tags read as `petramond:<name>` (e.g. `petramond:leaves`);
    /// pack tags as their `mod_id:name`. A name nothing lists is simply an
    /// empty set, never an error — querying cannot register a tag.
    /// → [`HostRet::BlockList`].
    BlocksByTag {
        tag: String,
    },
    /// Every registered item carrying `tag`, in id order — the item twin of
    /// [`HostCall::BlocksByTag`], same contract: registry-only (legal on any
    /// instance, any time), engine tags as `petramond:<name>`, pack tags as
    /// their `mod_id:name`, and a name nothing lists is simply an empty set —
    /// querying cannot register a tag. → [`HostRet::ItemList`].
    ItemsByTag {
        tag: String,
    },
    /// Resolve session block ids back to their registry NAMES — the reverse of
    /// [`HostCall::ResolveBlock`], batched at the message level (resolve a
    /// whole [`HostCall::BlocksByTag`] result in one crossing). Reply parallel
    /// to `blocks`; `None` = unregistered id. At most 4096 ids per call (the
    /// sim batch cap; the id space is 256 — a legitimate batch never
    /// approaches it). Registry-only: legal on any
    /// instance, any time. → [`HostRet::Names`].
    BlockNames {
        blocks: Vec<BlockId>,
    },
    /// Resolve session item ids back to their registry NAMES — the reverse of
    /// [`HostCall::ResolveItem`], same batching and contract as
    /// [`HostCall::BlockNames`]. How an id from an event payload or
    /// [`HostCall::ItemsByTag`] reaches the name-addressed calls
    /// ([`HostCall::GiveItem`], [`HostCall::ItemInfo`]). → [`HostRet::Names`].
    ItemNames {
        items: Vec<ItemId>,
    },
    /// Resolve a mob species key (`"petramond:sheep"`, `"monsters:zombie"` —
    /// the `key` field of a `mobs.json` row, the same string
    /// [`HostCall::SpawnMob`] and [`MobSnapshot::key`] speak) to its
    /// session-scoped [`MobId`] — how a mod filters the `kind` in
    /// `mob_died`/`mob_spawned`/`mob_damage_pre` payloads without string
    /// round-trips. Registry-only like [`HostCall::ResolveBlock`]: legal on
    /// any instance, any time. `None` = unregistered key. →
    /// [`HostRet::MobKind`].
    ResolveMob {
        key: String,
    },
    /// Resolve session mob species ids back to their keys — the reverse of
    /// [`HostCall::ResolveMob`], batched like [`HostCall::ItemNames`]. Reply
    /// parallel to `mobs`; `None` = unregistered id. Registry-only: legal on
    /// any instance, any time. → [`HostRet::Names`].
    MobNames {
        mobs: Vec<MobId>,
    },
    /// The collision-shape CLASS of the cell at `pos` — generic physics, no
    /// gameplay policy: [`CollisionShape::Full`] = exactly one collision box
    /// spanning the whole unit cell, [`CollisionShape::Partial`] = any other
    /// non-empty box set (stairs, slabs, doors, snow layers, model blocks),
    /// [`CollisionShape::Empty`] = no collision boxes (air, water, tall
    /// grass). `None` = section unloaded / streamed content not yet final
    /// (the [`HostCall::GetBlock`] contract: state frozen, retry later).
    /// Spawn/placement rules compose on top in mod code — e.g. "full solid
    /// footing" = `Full` + the block is not water + not in
    /// [`HostCall::BlocksByTag`]`("petramond:leaves")`.
    /// → [`HostRet::CollisionShape`].
    CollisionShapeAt {
        pos: [i32; 3],
    },

    // --- appended after the frozen set above (wire evolution is APPEND-ONLY —
    // postcard numbers variants by declaration index) ------------------------
    /// The WHOLE tag map of the live mob `mob_id`, sorted by key — one call
    /// instead of one [`HostCall::MobTagGet`] per key. `MobTags(None)` = no
    /// such live mob. → [`HostRet::MobTags`].
    MobTagsGet {
        mob_id: u64,
    },
    /// Every live mob carrying `key` (any value); with `value: Some(v)` only
    /// those whose stored value EQUALS `v` (exact match — a `F64` NaN matches
    /// nothing). Resolved host-side against the live set, dead mobs excluded
    /// exactly like [`HostCall::MobsInRadius`]. → [`HostRet::Mobs`].
    MobsWithTag {
        key: String,
        value: Option<MobTagValue>,
    },
    /// Every cell in the INCLUSIVE box `min..=max` currently holding one of
    /// `blocks`, resolved host-side in one scan (never page a box through
    /// [`HostCall::GetBlocks`] to search it). Positions come back in scan
    /// order — ascending `y`, then `z`, then `x` — so "the nearest match" is
    /// the caller's own fold over a deterministic list. The box is capped at
    /// 32768 cells (32³) and `blocks` at the sim batch cap; an inverted box
    /// (`min > max` on any axis) is an error. Reads are stream-final like
    /// [`HostCall::GetBlock`]: ANY unreadable cell in the box makes the whole
    /// reply `None` (state frozen, retry later) — a partial search would let
    /// policy act on terrain a saved overlay is about to replace. The one
    /// exception: cells OUTSIDE the world's vertical range are definitionally
    /// empty, so the scan clamps to it instead of gating (a box poking past
    /// the world's top must not starve a search forever).
    /// → [`HostRet::FoundBlocks`].
    FindBlocks {
        min: [i32; 3],
        max: [i32; 3],
        blocks: Vec<BlockId>,
    },
    /// Snapshot ONE live mob by its stable id — the single-mob sibling of
    /// [`HostCall::MobsInRadius`], for a handler that already holds an id
    /// (an event payload, a stored tag) and needs the mob's current state
    /// (pose to act on, species to branch on). `None` = no such live mob
    /// (dead mobs are gone to the ABI, as everywhere).
    /// → [`HostRet::Mob`].
    MobInfo {
        mob_id: u64,
    },
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
    /// → [`HostRet::Bool`].
    MobCanReach {
        mob_id: u64,
        cell: [i32; 3],
    },
    /// Resolve a block SHAPE-KIND registry key (`"petramond:fence"`,
    /// `"mymod:gate"`) to its session-local numeric id — the shape twin of
    /// [`HostCall::ResolveBlock`], for a Layer-3 mod branching on the
    /// `shape_kind` its bake calls carry. Registry-only (legal on any instance).
    /// `None` = no such shape kind. → [`HostRet::MaybeByte`].
    ResolveShape {
        key: String,
    },
    /// Pin `player_id` in a named POSE at the world-space `anchor` (rider
    /// feet origin), body facing `yaw` (player convention: yaw `0` faces
    /// `+Z`) — the static-seat primitive. The calling mod owns WHERE poses
    /// exist (its own seat layout) and WHO may take one; the engine owns the
    /// mechanism: one pose per player, no two players on one exact anchor,
    /// replication + the posed body, and every release valve (sneak gesture,
    /// death, spectator, leave). Pose vocabulary: [`crate::pose`] (`0` is
    /// reserved; unknown values pin the rest pose). Poses are TRANSIENT
    /// (never persisted) and NOT tied to any block — a mod whose furniture
    /// breaks releases the sitter itself ([`HostCall::MobDismount`]); a
    /// player a disabled mod leaves posed escapes through the engine valves.
    /// Occupancy is read back from the roster
    /// ([`crate::PlayerSnapshot::pose_anchor`]), never mirrored in mod
    /// state. `false` = already posed or mounted, anchor taken, reserved
    /// pose `0`, or a non-finite anchor/yaw. → [`HostRet::Bool`].
    PlayerPoseSet {
        player_id: PlayerId,
        anchor: [f32; 3],
        yaw: f32,
        pose: u8,
    },
    /// The placed MODEL-BLOCK group at `pos` (any of its cells): the group's
    /// base cell and placement facing — what block-local policy needs to map
    /// footprint-space data (a seat layout, a machine front) into the world.
    /// `None` = no model group there or the cell is unloaded.
    /// → [`HostRet::ModelGroup`].
    BlockModelGroup {
        pos: [i32; 3],
    },
}

/// Host → guest reply for a [`HostCall`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum HostRet {
    Unit,
    U64(u64),
    /// The call was rejected (e.g. registration outside `mod_init`). The SDK
    /// surfaces this as a guest panic — loud, and the mod gets disabled.
    Error(String),
    Bool(bool),
    /// [`HostCall::GetBlock`]: `None` = section unloaded / out of range.
    Block(Option<BlockId>),
    /// [`HostCall::GetBlocks`] / [`HostCall::ClientBlocksAt`], parallel to
    /// the request positions.
    Blocks(Vec<Option<BlockId>>),
    /// [`HostCall::LightAt`], all on the 6-bit `0..=63` scale. `None` =
    /// section unloaded / streamed content not final (never fabricated).
    Light(Option<LightData>),
    /// [`HostCall::MobsInRadius`] / [`HostCall::MobsWithTag`].
    Mobs(Vec<MobSnapshot>),
    /// [`HostCall::PlayerState`].
    Player(PlayerSnapshot),
    /// The KV gets: `None` = key absent (or target unloaded/missing).
    Bytes(#[serde(with = "serde_bytes")] Option<Vec<u8>>),
    /// [`HostCall::MobTagGet`]: the lookup outcome — a missing mob is told
    /// apart from an absent key (see [`MobTagLookup`]).
    MobTag(MobTagLookup),
    /// [`HostCall::GuiStateGet`]: `None` = key absent.
    GuiValue(Option<GuiValue>),
    /// [`HostCall::ContainerGet`]: every slot in index order; `None` = no
    /// container / unloaded.
    ContainerSlots(Option<Vec<Option<ItemStackData>>>),
    /// [`HostCall::ItemInfo`]: `None` = unknown item key.
    ItemInfo(Option<ItemInfoData>),
    /// [`HostCall::RecipeResult`]: `None` = no recipe for that input.
    ItemStack(Option<ItemStackData>),
    /// [`HostCall::EffectsActive`]: the player's active status effects.
    Effects(Vec<EffectStateData>),
    /// [`HostCall::ContainerGetMany`], parallel to the request positions
    /// (each entry as [`HostRet::ContainerSlots`]'s payload).
    Containers(Vec<Option<Vec<Option<ItemStackData>>>>),
    RuntimeSide(RuntimeSide),
    /// [`HostCall::ClientSurfaceColumns`], parallel to the request queries:
    /// `None` = column unknown to the replica; a reply without cell bytes =
    /// unchanged since the queried revision.
    ClientSurfaceColumns(Vec<Option<ClientSurfaceColumn>>),
    ClientTextSize([u16; 2]),
    ClientStorageValues(Vec<Option<serde_bytes::ByteBuf>>),
    /// [`HostCall::ResolveItem`]: `None` = unknown item name.
    Item(Option<ItemId>),
    /// [`HostCall::ClientStorageReadPoll`]: `None` = still in flight (poll
    /// again next frame); `Some` consumes the ticket.
    ClientStorageRead(Option<Vec<Option<serde_bytes::ByteBuf>>>),
    /// [`HostCall::MobRiders`]: `None` = no such live mob.
    Riders(Option<MobRidersData>),
    /// [`HostCall::BlockModelGroup`]: `None` = no model group / unloaded.
    ModelGroup(Option<crate::ModelGroupData>),
    /// [`HostCall::PlayerInput`]: `None` = no such player connected.
    PlayerInput(Option<PlayerInputData>),
    /// [`HostCall::MobAnimState`]: `None` = missing/dead mob or inactive anim.
    MobAnimState(Option<MobAnimStateData>),
    /// Byte-vocabulary answers (biome ids): `None` = unloaded/unknown.
    MaybeByte(Option<u8>),
    /// [`HostCall::SurfaceYAt`]: `None` = unloaded or all-air column.
    MaybeI32(Option<i32>),
    /// [`HostCall::Players`]: every connected player, session-id order.
    Players(Vec<PlayerListEntry>),
    /// [`HostCall::ClientEnvParams`], parallel to the request keys
    /// (`None` = param not present in the environment).
    EnvParams(Vec<Option<[f32; 4]>>),
    /// [`HostCall::BlocksByTag`]: the tag's members, id order (empty = no
    /// block carries it).
    BlockList(Vec<BlockId>),
    /// [`HostCall::ItemsByTag`]: the tag's members, id order (empty = no
    /// item carries it).
    ItemList(Vec<ItemId>),
    /// [`HostCall::BlockNames`] / [`HostCall::ItemNames`] /
    /// [`HostCall::MobNames`], parallel to the request ids (`None` =
    /// unregistered id).
    Names(Vec<Option<String>>),
    /// [`HostCall::ResolveMob`]: `None` = unregistered species key.
    MobKind(Option<MobId>),
    /// [`HostCall::CollisionShapeAt`]: `None` = section unloaded / streamed
    /// content not final.
    CollisionShape(Option<CollisionShape>),
    /// [`HostCall::MobTagsGet`]: the mob's full tag map, sorted by key;
    /// `None` = no such live mob.
    MobTags(Option<Vec<(String, MobTagValue)>>),
    /// [`HostCall::SpawnMob`]: the newborn's STABLE session id — the address
    /// every mob call speaks, so a spawner can immediately tag/configure what
    /// it created. `None` = unknown key, the mob cap, or a failed check.
    SpawnedMob(Option<u64>),
    /// [`HostCall::FindBlocks`]: matching cells in scan order; `None` = some
    /// cell in the box is unloaded / streamed content not yet final.
    FoundBlocks(Option<Vec<[i32; 3]>>),
    /// [`HostCall::MobInfo`]: the mob's snapshot; `None` = no such live mob.
    Mob(Option<MobSnapshot>),
}
