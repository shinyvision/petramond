//! The llamacraft mod ABI: shared types crossing the engine↔WASM boundary.
//!
//! Both sides speak postcard-serialized enums over two raw entry points
//! (`host_dispatch` on the host, `mod_dispatch` on the guest); everything in this
//! crate is the vocabulary of those calls. The engine depends on this crate
//! directly; mods reach it through `mod-sdk`, which re-exports it and hides the
//! raw ABI (`mod_alloc`/`mod_free`/pointer packing) behind safe wrappers.
//!
//! # APPEND-ONLY evolution
//!
//! postcard has no schema: enum variants encode as their **declaration index**
//! and struct fields encode **positionally**. A shipped mod keeps its compiled
//! copy of these types forever, so the wire contract is:
//!
//! - NEVER reorder, remove, or insert enum variants — new variants go at the END.
//! - NEVER add, remove, or reorder fields of a shipped variant — new capability
//!   means a NEW variant, not a wider old one.
//! - The same applies to every type reachable from [`HostCall`]/[`GuestCall`]
//!   ([`EventPayload`], [`Stage`], [`EventKind`], ...).
//!
//! An old mod then keeps decoding everything it registered for, and the host
//! rejects (disables, never crashes on) a mod speaking a newer dialect only when
//! it actually sends an unknown variant.

use serde::{Deserialize, Serialize};

/// A runtime block id — raw `u8` into the engine's registry. Dynamic content is
/// NAME-addressed (`mod_id:name` keys in the pack catalogs assign ids at load),
/// so numeric ids are stable within a session but never across sessions or
/// saves; mods must not persist them. Resolve ids from names at `mod_init` time
/// with [`HostCall::ResolveBlock`].
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u8);

impl BlockId {
    /// Air is engine id 0 — the one numeric id frozen by contract (worldgen
    /// and the save format both rely on it).
    pub const AIR: BlockId = BlockId(0);
}

/// A runtime item id — same contract as [`BlockId`].
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItemId(pub u8);

/// A runtime mob species id — same contract as [`BlockId`].
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MobId(pub u8);

/// A pre-event handler's verdict. The first `Cancel` wins; later handlers still
/// observe the (possibly mutated) payload.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Cancel,
}

/// The engine's fixed-tick stages, in execution order (mirrors the engine's
/// stage list — see WIKI/modding.md "Tick stages").
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Mining,
    Placement,
    Attack,
    Drops,
    Menu,
    PlayerDamage,
    WorldScheduled,
    NaturalBreaks,
    Pickup,
    Mobs,
    ItemPhysics,
    Spawning,
}

/// Which side of a [`Stage`] a tick system attaches to. At the boundary between
/// stage N and N+1, `After(N)` systems run before `Before(N+1)` systems.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum AttachSide {
    Before,
    After,
}

/// The worldgen pipeline's addressable stages, in execution order. APPEND-ONLY
/// like every ABI enum.
///
/// `Climate` assigns the per-column biome map; `Terrain` is the block fill plus
/// cave carve; `Underground` scatters ores/blobs; `Vegetation` places
/// single-block ground plants; `Trees` places the tree features. Features
/// ([`HostCall::RegisterWorldgenFeature`]) attach AFTER a stage (`Climate` is
/// not a valid feature attach point — it is column-level, before any blocks
/// exist); replacements ([`HostCall::RegisterStageReplacement`]) substitute the
/// engine stage itself.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum WorldgenStage {
    Climate,
    Terrain,
    Underground,
    Vegetation,
    Trees,
}

/// Every dispatchable event, pre and post (see the taxonomy in WIKI/modding.md).
/// Registration key for [`HostCall::RegisterEventHandler`].
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    BlockPlacePre,
    BlockBreakPre,
    BlockInteract,
    ItemUsePre,
    MobHurtPre,
    PlayerDamagePre,
    BlockPlaced,
    BlockBroken,
    ItemUsed,
    MobDied,
    MobSpawned,
    PlayerDamaged,
    PlayerDied,
    ContainerOpened,
    ContainerClosed,
    SectionGenerated,
    SectionLoaded,
}

/// Why the player is taking damage. APPEND-ONLY like every ABI enum.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DamageSource {
    Fall,
    /// A mob's melee strike; `key` is the attacking species' registry name
    /// (`"llama:owl"`, `"zombies:zombie"`).
    Mob {
        key: String,
    },
    /// A mod's [`HostCall::DamagePlayer`] / [`HostCall::KillPlayer`]; `mod_id`
    /// is the calling mod's pack id, so handlers can filter by origin.
    Mod {
        mod_id: String,
    },
}

/// Which container GUI opened/closed. APPEND-ONLY like every ABI enum.
/// (`Copy` was dropped when `Mod` gained its String payload — a Rust-trait
/// change, not a wire change.)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    Inventory,
    CraftingTable,
    Furnace,
    Chest,
    FurnitureWorkbench,
    /// A mod-defined GUI (Phase 5); `key` is its registered kind key
    /// (`"wheel:wheel"`).
    Mod {
        key: String,
    },
}

/// Player-derived placement facing.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum Facing {
    North,
    South,
    West,
    East,
}

/// One event's data, mirrored from the engine payloads (WIKI/modding.md
/// taxonomy). Pre events hand the payload to the guest `&mut`; the engine reads
/// back ONLY the fields the taxonomy marks mutable ([`MobHurtPre::amount`],
/// [`PlayerDamagePre::amount`]) — everything else is observational.
///
/// [`MobHurtPre::amount`]: EventPayload::MobHurtPre
/// [`PlayerDamagePre::amount`]: EventPayload::PlayerDamagePre
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum EventPayload {
    BlockPlacePre {
        pos: [i32; 3],
        block: BlockId,
        facing: Facing,
    },
    BlockBreakPre {
        pos: [i32; 3],
        block: BlockId,
        harvested: bool,
    },
    BlockInteract {
        pos: [i32; 3],
        block: BlockId,
    },
    ItemUsePre {
        item: ItemId,
        target: Option<[i32; 3]>,
    },
    MobHurtPre {
        /// Index into the live mob set, valid this tick only.
        mob: u32,
        kind: MobId,
        /// Mutable: written back by the engine after the dispatch.
        amount: f32,
        source: [f32; 3],
    },
    PlayerDamagePre {
        /// Mutable: written back by the engine after the dispatch.
        amount: i32,
        source: DamageSource,
    },
    BlockPlaced {
        pos: [i32; 3],
        block: BlockId,
    },
    BlockBroken {
        pos: [i32; 3],
        block: BlockId,
        harvested: bool,
        natural: bool,
    },
    ItemUsed {
        item: ItemId,
    },
    MobDied {
        kind: MobId,
        pos: [f32; 3],
    },
    MobSpawned {
        kind: MobId,
        pos: [f32; 3],
    },
    PlayerDamaged {
        amount: i32,
        new_health: i32,
    },
    PlayerDied,
    ContainerOpened {
        kind: ContainerKind,
        pos: Option<[i32; 3]>,
    },
    ContainerClosed {
        kind: ContainerKind,
        pos: Option<[i32; 3]>,
    },
    SectionGenerated {
        /// Section coordinates (16³ units).
        pos: [i32; 3],
    },
    SectionLoaded {
        pos: [i32; 3],
    },
}

impl EventPayload {
    pub fn kind(&self) -> EventKind {
        match self {
            EventPayload::BlockPlacePre { .. } => EventKind::BlockPlacePre,
            EventPayload::BlockBreakPre { .. } => EventKind::BlockBreakPre,
            EventPayload::BlockInteract { .. } => EventKind::BlockInteract,
            EventPayload::ItemUsePre { .. } => EventKind::ItemUsePre,
            EventPayload::MobHurtPre { .. } => EventKind::MobHurtPre,
            EventPayload::PlayerDamagePre { .. } => EventKind::PlayerDamagePre,
            EventPayload::BlockPlaced { .. } => EventKind::BlockPlaced,
            EventPayload::BlockBroken { .. } => EventKind::BlockBroken,
            EventPayload::ItemUsed { .. } => EventKind::ItemUsed,
            EventPayload::MobDied { .. } => EventKind::MobDied,
            EventPayload::MobSpawned { .. } => EventKind::MobSpawned,
            EventPayload::PlayerDamaged { .. } => EventKind::PlayerDamaged,
            EventPayload::PlayerDied => EventKind::PlayerDied,
            EventPayload::ContainerOpened { .. } => EventKind::ContainerOpened,
            EventPayload::ContainerClosed { .. } => EventKind::ContainerClosed,
            EventPayload::SectionGenerated { .. } => EventKind::SectionGenerated,
            EventPayload::SectionLoaded { .. } => EventKind::SectionLoaded,
        }
    }
}

/// One value of the open GUI session's state map (Phase 5). Written by mods
/// on the tick ([`HostCall::GuiStateSet`]); read per frame by the renderer to
/// drive `label` text, `rotimage` angles (radians, `F32`), and mod overlay
/// fractions. Keys are mod-local: the map belongs to one GUI session (cleared
/// on open/close), so no namespace prefix is enforced.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GuiValue {
    F32(f32),
    I32(i32),
    Str(String),
}

/// A live mob's snapshot for [`HostCall::MobsInRadius`]. `index` addresses the
/// mob in later calls ([`HostCall::HurtMob`], the mob KV calls) and is valid
/// THIS TICK ONLY — any engine mob removal (deaths finishing, despawns, section
/// unloads, [`HostCall::DespawnMob`]) renumbers; re-query, never store indices.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MobSnapshot {
    pub index: u32,
    /// The species' registry name (`"llama:owl"`, `"zombies:zombie"`).
    pub key: String,
    /// Feet position.
    pub pos: [f32; 3],
    pub health: f32,
    /// Stable session id for this live mob. Unlike `index`, this survives
    /// unrelated `swap_remove` renumbering; it is not a species id and is not
    /// promised stable across save/load.
    pub id: u64,
}

/// The player's state for [`HostCall::PlayerState`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerSnapshot {
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
}

/// Guest → host: what a mod asks the engine for through `host_dispatch`.
/// Phase 2b surface + the Phase 3b world/entity/player/KV calls (one match on
/// the host, room to append).
///
/// The world-touching calls are sim-scoped: legal wherever a `SimCtx` is
/// published (`mod_init`, tick systems, event handlers), [`HostRet::Error`]
/// outside any guest dispatch.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum HostCall {
    /// Log through the engine logger (mods have no stdout).
    Log { msg: String },
    /// The current game tick (20 per second). → [`HostRet::U64`].
    CurrentTick,
    /// Next value of the mod's named deterministic RNG stream (SplitMix64,
    /// seeded from world seed + mod id + key). → [`HostRet::U64`].
    RngU64 { stream_key: String },
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
    // --- Phase 3b: blocks -------------------------------------------------
    /// The block at a world cell: `Some` (air included) when its section is
    /// loaded, `None` when unloaded / outside the vertical range.
    /// → [`HostRet::Block`].
    GetBlock { pos: [i32; 3] },
    /// Batched [`HostCall::GetBlock`], one result per position in order.
    /// → [`HostRet::Blocks`].
    GetBlocks { positions: Vec<[i32; 3]> },
    /// Set one block through the engine's full edit path (relight, neighbour
    /// updates, `block` events' world state all hold). `false` = the cell is
    /// unloaded / out of range. → [`HostRet::Bool`].
    SetBlock { pos: [i32; 3], block: BlockId },
    /// Batched [`HostCall::SetBlock`]; applied in order, each through the full
    /// edit path. NOTE: every write pays its own relight/remesh of the 3×3×3
    /// section neighbourhood — huge batches are expensive; this batches the
    /// ABI crossing, not the world work. → [`HostRet::U64`] (cells actually set).
    SetBlocks { blocks: Vec<([i32; 3], BlockId)> },
    /// Run the cell's block behavior `scheduled_tick` in `delay` game ticks
    /// (first schedule per cell wins, like water's flow checks).
    /// → [`HostRet::Unit`].
    ScheduleTick { pos: [i32; 3], delay: u64 },
    /// Whether the section owning the cell is loaded. → [`HostRet::Bool`].
    IsLoaded { pos: [i32; 3] },
    /// Cached light at a cell on the renderer's 6-bit scale (`0..=63`):
    /// combined = max(sky, block). Unloaded cells read as open sky / no block
    /// light (the engine's own fallbacks). → [`HostRet::Light`].
    LightAt { pos: [i32; 3] },

    // --- Phase 3b: entities -----------------------------------------------
    /// Spawn a mob by species registry name at `pos` (feet) facing `yaw`.
    /// `false` = unknown key or the mob cap is reached. → [`HostRet::Bool`].
    SpawnMob {
        key: String,
        pos: [f32; 3],
        yaw: f32,
    },
    /// Snapshot the live mobs within `radius` (3-D, of feet positions) of
    /// `pos`. Deterministic order = the live set's storage order (spawn order,
    /// perturbed only by removals). Dead (ragdolling) mobs are excluded.
    /// → [`HostRet::Mobs`].
    MobsInRadius { pos: [f32; 3], radius: f32 },
    /// Hurt the mob at `index` (from attacker point `from`, which the
    /// knockback pushes away from), through the `mob_hurt_pre` pipeline
    /// exactly like a player attack. Applied at the next action drain point
    /// (same tick), so a handler cannot re-enter the bus. → [`HostRet::Unit`].
    HurtMob {
        index: u32,
        amount: f32,
        from: [f32; 3],
    },
    /// Remove the mob at `index` from the live world immediately (not saved,
    /// no death/loot). Renumbers later indices — re-query after use.
    /// `false` = no such mob. → [`HostRet::Bool`].
    DespawnMob { index: u32 },
    /// Spawn `count` of an item (by registry key) as a dropped-item entity at
    /// `pos`. `false` = unknown key / zero count. → [`HostRet::Bool`].
    SpawnItem {
        item_key: String,
        count: u8,
        pos: [f32; 3],
    },

    // --- Phase 3b: player ---------------------------------------------------
    /// The player's current state. → [`HostRet::Player`].
    PlayerState,
    /// Damage the player through the single engine funnel: `player_damage_pre`
    /// (other mods' i-frames) applies, with [`DamageSource::Mod`] carrying the
    /// calling mod's id. Queued; applied at the next action drain point (same
    /// tick, defined order). → [`HostRet::Unit`].
    DamagePlayer { amount: i32 },
    /// Add a knockback impulse to the player's velocity on the tick (spectator
    /// no-op; a positive-y impulse reads as a launch). Non-finite components
    /// are rejected with [`HostRet::Error`]. → [`HostRet::Unit`].
    ApplyKnockback { impulse: [f32; 3] },
    /// Give the player `count` of an item (by registry key) through the normal
    /// inventory fill; whatever doesn't fit drops at the player's feet like any
    /// other overflow. `false` = unknown key. → [`HostRet::Bool`].
    GiveItem { item_key: String, count: u8 },
    /// Kill the player: damage equal to current health, through the same
    /// funnel (and queue) as [`HostCall::DamagePlayer`] — i-frame handlers can
    /// still cancel it. → [`HostRet::Unit`].
    KillPlayer,
    /// Overwrite the player's health (clamped to `0..=20` half-hearts),
    /// BYPASSING the damage funnel — this is the heal/set primitive, not a
    /// damage source (no events fire). → [`HostRet::Unit`].
    SetHealth { value: i32 },
    /// Move the player's feet to `pos`, clearing fall tracking so the
    /// teleport can never land as fall damage. Non-finite components are
    /// rejected with [`HostRet::Error`]. → [`HostRet::Unit`].
    Teleport { pos: [f32; 3] },

    // --- Phase 3b: sound ----------------------------------------------------
    /// Play a sound by `sounds.json` key (namespaced for pack sounds), routed
    /// through the tick→presentation channel — the sim never touches audio.
    /// `pos` attenuates by the sound row's `attenuation_distance`; `None`
    /// plays at full volume. `false` = unknown key. → [`HostRet::Bool`].
    EmitSound { key: String, pos: Option<[f32; 3]> },

    // --- Phase 3b: persistent KV -------------------------------------------
    // Keys are namespaced. WRITES (set/delete) must use the calling mod's own
    // prefix or an exposed engine `llama:*` key; READS may cross namespaces —
    // that is the cross-mod interop surface (core day/night publishes, zombies
    // reads). Limits: key ≤ 256 bytes, value ≤ 64 KiB; violations return
    // `HostRet::Error`.
    /// World KV (persists in `level.dat`). → [`HostRet::Bytes`].
    WorldKvGet { key: String },
    /// → [`HostRet::Unit`].
    WorldKvSet { key: String, value: Vec<u8> },
    /// → [`HostRet::Bool`] (whether the key was present).
    WorldKvDelete { key: String },
    /// Per-cell KV riding the cell's section save record (`pos` is a world
    /// block position). `Bytes(None)` when absent OR the section is unloaded.
    /// → [`HostRet::Bytes`].
    SectionKvGet { pos: [i32; 3], key: String },
    /// `false` = the section is unloaded (nothing stored). → [`HostRet::Bool`].
    SectionKvSet {
        pos: [i32; 3],
        key: String,
        value: Vec<u8>,
    },
    /// → [`HostRet::Bool`] (whether the key was present).
    SectionKvDelete { pos: [i32; 3], key: String },
    /// Per-mob KV riding the mob's save record (`mob_index` as in
    /// [`MobSnapshot::index`] — valid this tick only). → [`HostRet::Bytes`].
    MobKvGet { mob_index: u32, key: String },
    /// `false` = no such mob. → [`HostRet::Bool`].
    MobKvSet {
        mob_index: u32,
        key: String,
        value: Vec<u8>,
    },
    /// → [`HostRet::Bool`] (whether the key was present).
    MobKvDelete { mob_index: u32, key: String },

    // --- Phase 4: worldgen hooks ---------------------------------------------
    /// Resolve a block registry key (`"llama:stone"`, `"smoke:smoke_block"`) to its
    /// session-scoped runtime id. Needs no simulation context — legal anywhere,
    /// including on worldgen instances. `None` = not registered (a typo'd or
    /// absent pack — degrade gracefully, don't panic). → [`HostRet::Block`].
    ResolveBlock { key: String },
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
    RegisterGenerator { callback_id: u32 },

    // --- Phase 5: mod GUIs ----------------------------------------------------
    /// Write a key of the open GUI session's state map (tick-owned; the
    /// renderer reads a snapshot per frame). Keys are mod-local — the map
    /// belongs to one GUI session and is cleared on open/close. Sim-scoped.
    /// → [`HostRet::Unit`].
    GuiStateSet { key: String, value: GuiValue },
    /// Read a key of the GUI state map (`None` = absent). Sim-scoped.
    /// → [`HostRet::GuiValue`].
    GuiStateGet { key: String },
    /// Ask the app shell to open the mod GUI registered under `kind_key`
    /// (`"wheel:wheel"` — a baked manifest or `open_gui` block row must have
    /// registered it). Queued like [`HostCall::DamagePlayer`]; the screen
    /// opens after this tick, only from gameplay (an already-open menu drops
    /// the request). `false` = unknown / non-mod kind. → [`HostRet::Bool`].
    GuiOpen { kind_key: String },
    /// Close the open mod GUI (a no-op if none is open — engine containers
    /// are not closable from mods). Queued like [`HostCall::GuiOpen`].
    /// → [`HostRet::Unit`].
    GuiClose,

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
    SoundStop { handle: u64 },

    // --- Bugfix round 1 (spawning): mod-visible full-block support --------
    /// Whether the loaded block at `pos` is valid full-cube spawn support:
    /// one full collision cube, not water, not leaves. Partial shapes such as
    /// stairs, doors, and model blocks return false, as do unloaded/out-of-range
    /// cells. → [`HostRet::Bool`].
    BlockIsFullSpawnSupport { pos: [i32; 3] },

    // --- Shader parameters ------------------------------------------------
    /// Set one named visual shader parameter (`vec4<f32>`). Mods may write
    /// their own `mod_id:name` keys or exposed engine `llama:*` keys; active
    /// shader packs map keys onto fixed GPU slots. Not persisted: re-apply it
    /// from mod state on load.
    /// → [`HostRet::Unit`].
    ShaderSetParam { key: String, value: [f32; 4] },

    // --- Hostile spawning -------------------------------------------------
    /// Register a hostile-spawn callback. The core engine supplies candidate
    /// sites and enforces caps/body fit; the callback returns a hostile mob key
    /// if this mod wants to spawn something there. Legal ONLY during `mod_init`.
    /// → [`HostRet::Unit`].
    RegisterHostileSpawner { callback_id: u32, priority: i32 },

    // --- Block behaviors (Phase 2b, landed 2026-07-06) ---------------------
    /// Register the reactive behavior for block rows whose `blocks.json`
    /// `behavior` field is `key` — a `mod_id:name` owned by THIS pack. The
    /// engine then dispatches [`GuestCall::BlockBehavior`] with `callback_id`
    /// for every hook that fires on such a block. Legal ONLY during
    /// `mod_init`. → [`HostRet::Unit`].
    RegisterBlockBehavior { key: String, callback_id: u32 },

    // --- Scripted AI nodes (landed 2026-07-06) ------------------------------
    /// Register the scripted AI node for `mobs.json` brain rows whose `node`
    /// key is `key` — a `mod_id:name` owned by THIS pack. The engine then
    /// dispatches [`GuestCall::AiNode`] with `callback_id` once per owning
    /// mob per game tick. Legal ONLY during `mod_init`. → [`HostRet::Unit`].
    RegisterAiNode { key: String, callback_id: u32 },
}

/// Which [`BlockBehavior`](GuestCall::BlockBehavior) hook fired — the mod-side
/// mirror of the engine `BlockBehavior` trait's methods.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockHookKind {
    /// The probabilistic per-section random tick (a few cells per section per
    /// game tick). Mod-behavior blocks always receive random ticks.
    RandomTick,
    /// A scheduled tick previously requested via [`HostCall::ScheduleTick`].
    ScheduledTick,
    /// The cell or one of its 6 neighbours changed (the ANNOUNCE phase).
    NeighborUpdate,
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
    /// [`HostCall::GetBlocks`], parallel to the request positions.
    Blocks(Vec<Option<BlockId>>),
    /// [`HostCall::LightAt`], all on the 6-bit `0..=63` scale.
    Light {
        combined: u8,
        sky: u8,
        block: u8,
    },
    /// [`HostCall::MobsInRadius`].
    Mobs(Vec<MobSnapshot>),
    /// [`HostCall::PlayerState`].
    Player(PlayerSnapshot),
    /// The KV gets: `None` = key absent (or target unloaded/missing).
    Bytes(Option<Vec<u8>>),
    /// [`HostCall::GuiStateGet`]: `None` = key absent.
    GuiValue(Option<GuiValue>),
}

/// One worldgen block write: `(world position, block)`. Applied by the engine
/// through a section-clipping sink — writes outside the dispatched section are
/// dropped (that clipping IS the seam mechanism, see [`GuestCall::GenFeature`]).
pub type GenWrite = ([i32; 3], BlockId);

/// Host → guest: what the engine asks a mod to run through `mod_dispatch`.
/// (`mod_init` is its own export and carries no payload.)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GuestCall {
    /// Run the tick system the mod registered under `id`.
    TickSystem { id: u32 },
    /// Handle one event with the handler registered under `id`. The guest
    /// returns the (possibly mutated) payload in [`GuestRet::Event`].
    HandleEvent {
        id: u32,
        kind: EventKind,
        payload: EventPayload,
    },

    // --- Phase 4: worldgen hooks ---------------------------------------------
    /// Generate one registered feature's writes for one 16³ section.
    /// → [`GuestRet::GenWrites`].
    ///
    /// DETERMINISM CONTRACT (binding — a violation shows up as world seams):
    /// the reply must be a pure function of this call's fields. Worldgen
    /// instances are SEPARATE wasm instances per worker thread sharing NOTHING
    /// with the tick instance; no sim-scoped host call works here, and any
    /// state carried between calls breaks (seed, section) reproducibility.
    /// A feature spanning section boundaries must derive identical per-origin
    /// decisions in EVERY section its writes touch (positional RNG over
    /// `(seed, origin)` + the column data below); the engine clips each call's
    /// writes to its own section, which makes consistent emission seamless.
    GenFeature {
        feature_id: u32,
        /// Section coordinates (16³ units; world origin = `pos * 16`).
        section_pos: [i32; 3],
        /// The world seed — feed it to the SDK's positional RNG.
        seed: u32,
        /// 4096-byte snapshot of the section as of this attach point (engine
        /// stages + earlier hooks applied), layout `y*256 + z*16 + x`.
        blocks: Vec<u8>,
        /// 256 entries (`z*16 + x`), the column's post-cave bare-ground top
        /// (world Y, before vegetation/trees; below `sea_level` = submerged
        /// or floorless). Identical for every section of one column.
        surface_heights: Vec<i32>,
        /// 256 biome ids (`z*16 + x`), identical for every section of a column.
        biomes: Vec<u8>,
        sea_level: i32,
    },
    /// Run a registered stage REPLACEMENT. Same field meanings and determinism
    /// contract as [`GuestCall::GenFeature`]. Expected reply by stage:
    /// `Climate` → [`GuestRet::GenBiomes`] (256 ids; `section_pos` is
    /// `[cx, 0, cz]`, `blocks` empty, `biomes` = the engine's proposal),
    /// `Terrain` → [`GuestRet::GenBlocks`] (the full 4096 fill; `blocks`
    /// empty), others → [`GuestRet::GenWrites`]. A wrong-shape reply disables
    /// the mod; the engine stage then runs as the fallback.
    GenStage {
        callback_id: u32,
        stage: WorldgenStage,
        section_pos: [i32; 3],
        seed: u32,
        blocks: Vec<u8>,
        surface_heights: Vec<i32>,
        biomes: Vec<u8>,
        sea_level: i32,
    },

    // --- Phase 5: mod GUIs ----------------------------------------------------
    /// A button of the mod's own GUI was clicked (dispatched on the tick, in
    /// click order, to the mod whose namespace `kind_key` carries). `pos` is
    /// the block the GUI was opened from (`None` for a programmatic
    /// [`HostCall::GuiOpen`]). → [`GuestRet::Unit`].
    GuiClick {
        kind_key: String,
        widget_id: String,
        pos: Option<[i32; 3]>,
    },

    // --- Hostile spawning -------------------------------------------------
    /// Ask a registered hostile spawner whether this candidate should produce
    /// a hostile species. → [`GuestRet::HostileSpawn`].
    HostileSpawnCandidate {
        callback_id: u32,
        candidate: HostileSpawnCandidate,
    },

    // --- Block behaviors (Phase 2b, landed 2026-07-06) ---------------------
    /// A hook fired on a block whose row's `behavior` the mod registered via
    /// [`HostCall::RegisterBlockBehavior`]. Dispatched on the game tick, in
    /// hook-fire order, right after the world's own scheduled/random ticks —
    /// so a handler edits the world through sim host calls one dispatch step
    /// later than an engine-compiled behavior would. → [`GuestRet::Unit`].
    BlockBehavior {
        callback_id: u32,
        kind: BlockHookKind,
        pos: [i32; 3],
    },

    // --- Scripted AI nodes (landed 2026-07-06) ------------------------------
    /// One AI decision for one mob, this tick — the node the mod registered
    /// via [`HostCall::RegisterAiNode`]. DECISION-ONLY: the dispatch runs
    /// inside the mob tick with NO simulation scope, so sim host calls
    /// (world edits, spawns, player state) error here; core calls (RNG, log,
    /// tick) work. Return desires in [`GuestRet::AiDecision`]; the engine's
    /// brain arbitration merges them by the brain row's priority.
    /// → [`GuestRet::AiDecision`].
    AiNode { callback_id: u32, ctx: AiNodeCtx },
}

/// The read-only mob snapshot an [`GuestCall::AiNode`] decision sees.
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
    /// Player body-centre (world space).
    pub player_pos: [f32; 3],
    /// True when the navigator has no active path ("the mob is idle").
    pub nav_idle: bool,
    /// True when the mob's body is in water.
    pub in_water: bool,
}

/// One scripted node's contribution to a mob's tick. Every field defaults to
/// "no opinion"; the engine keeps the highest-priority non-`None` value per
/// field across the whole brain (scripted and engine nodes alike).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct AiNodeDecision {
    /// A navigation destination (world voxel) to path toward.
    pub goal: Option<[i32; 3]>,
    /// A desired head orientation `[yaw, pitch]` relative to the body.
    pub head_look: Option<[f32; 2]>,
    /// An `idle_*` animation index to play.
    pub idle_anim: Option<u8>,
    /// A melee strike `[damage, knockback]` to land on the player this tick.
    pub attack: Option<[f32; 2]>,
}

/// Guest → host reply for a [`GuestCall`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GuestRet {
    Unit,
    /// Reply to [`GuestCall::HandleEvent`]: the verdict plus the payload echoed
    /// back so the engine can read the mutable fields.
    Event {
        outcome: Outcome,
        payload: EventPayload,
    },
    /// Reply to [`GuestCall::GenFeature`] and to non-climate/terrain
    /// [`GuestCall::GenStage`]: world-position block writes, applied in order
    /// through the engine's section clip. An unregistered block id disables
    /// the mod (never reaches world storage).
    GenWrites(Vec<GenWrite>),
    /// Reply to a `Terrain` [`GuestCall::GenStage`]: the complete 4096-block
    /// section fill (layout `y*256 + z*16 + x`). Must be exactly 4096
    /// registered ids.
    GenBlocks(Vec<u8>),
    /// Reply to a `Climate` [`GuestCall::GenStage`]: the 256-entry column
    /// biome map (`z*16 + x`). Must be exactly 256 valid biome ids.
    GenBiomes(Vec<u8>),
    /// Reply to [`GuestCall::HostileSpawnCandidate`]: `Some(registry_key)` to
    /// ask core to spawn that hostile species here, `None` to reject this site.
    HostileSpawn(Option<String>),
    /// Reply to [`GuestCall::AiNode`]: the node's desires for this mob this
    /// tick (`None` = no opinion on anything, same as the default decision).
    AiDecision(Option<AiNodeDecision>),
}

/// Pack a guest-memory buffer address for the `u64` return lane of
/// `mod_dispatch`/`host_dispatch`: `ptr << 32 | len`.
#[inline]
pub fn pack_ptr_len(ptr: u32, len: u32) -> u64 {
    ((ptr as u64) << 32) | len as u64
}

/// Inverse of [`pack_ptr_len`].
#[inline]
pub fn unpack_ptr_len(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

/// Encode any ABI value for the wire.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(value)
}

/// Decode any ABI value from the wire.
pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ABI contract both sides rely on: every call/reply enum round-trips
    /// through postcard, including nested payloads, and the pointer packing is
    /// lossless. (No wire-byte pinning — the encoding is postcard's contract;
    /// ours is that encode∘decode is identity.)
    #[test]
    fn abi_roundtrip_host_and_guest_calls() {
        fn roundtrip<T>(v: T)
        where
            T: Serialize + for<'de> Deserialize<'de> + PartialEq + core::fmt::Debug,
        {
            let bytes = encode(&v).expect("encode");
            let back: T = decode(&bytes).expect("decode");
            assert_eq!(back, v);
        }

        roundtrip(HostCall::Log {
            msg: "hello".into(),
        });
        roundtrip(HostCall::CurrentTick);
        roundtrip(HostCall::RngU64 {
            stream_key: "spawn".into(),
        });
        roundtrip(HostCall::RegisterTickSystem {
            stage: Stage::Spawning,
            attach: AttachSide::After,
            priority: -3,
            system_id: 42,
        });
        roundtrip(HostCall::RegisterEventHandler {
            event: EventKind::BlockPlaced,
            priority: 7,
            handler_id: 9,
        });
        roundtrip(HostCall::GetBlock { pos: [1, -64, 3] });
        roundtrip(HostCall::GetBlocks {
            positions: vec![[0, 0, 0], [1, 2, 3]],
        });
        roundtrip(HostCall::SetBlock {
            pos: [5, 70, -2],
            block: BlockId(3),
        });
        roundtrip(HostCall::SetBlocks {
            blocks: vec![([0, 64, 0], BlockId(1)), ([0, 65, 0], BlockId(0))],
        });
        roundtrip(HostCall::ScheduleTick {
            pos: [9, 60, 9],
            delay: 5,
        });
        roundtrip(HostCall::IsLoaded { pos: [8, 0, 8] });
        roundtrip(HostCall::LightAt { pos: [8, 64, 8] });
        roundtrip(HostCall::SpawnMob {
            key: "zombies:zombie".into(),
            pos: [0.5, 64.0, 0.5],
            yaw: 1.5,
        });
        roundtrip(HostCall::MobsInRadius {
            pos: [0.0, 64.0, 0.0],
            radius: 16.0,
        });
        roundtrip(HostCall::HurtMob {
            index: 3,
            amount: 2.5,
            from: [1.0, 64.0, 1.0],
        });
        roundtrip(HostCall::DespawnMob { index: 7 });
        roundtrip(HostCall::SpawnItem {
            item_key: "llama:stick".into(),
            count: 4,
            pos: [0.5, 64.0, 0.5],
        });
        roundtrip(HostCall::PlayerState);
        roundtrip(HostCall::DamagePlayer { amount: 4 });
        roundtrip(HostCall::ApplyKnockback {
            impulse: [1.0, 3.0, -1.0],
        });
        roundtrip(HostCall::GiveItem {
            item_key: "llama:diamond".into(),
            count: 1,
        });
        roundtrip(HostCall::KillPlayer);
        roundtrip(HostCall::SetHealth { value: 20 });
        roundtrip(HostCall::Teleport {
            pos: [10.5, 80.0, -4.5],
        });
        roundtrip(HostCall::EmitSound {
            key: "mymod:zap".into(),
            pos: Some([0.0, 64.0, 0.0]),
        });
        roundtrip(HostCall::WorldKvGet {
            key: "llama:time".into(),
        });
        roundtrip(HostCall::WorldKvSet {
            key: "llama:time".into(),
            value: vec![1, 2, 3],
        });
        roundtrip(HostCall::WorldKvDelete {
            key: "llama:time".into(),
        });
        roundtrip(HostCall::SectionKvGet {
            pos: [4, -60, 4],
            key: "farm:moisture".into(),
        });
        roundtrip(HostCall::SectionKvSet {
            pos: [4, -60, 4],
            key: "farm:moisture".into(),
            value: vec![7],
        });
        roundtrip(HostCall::SectionKvDelete {
            pos: [4, -60, 4],
            key: "farm:moisture".into(),
        });
        roundtrip(HostCall::MobKvGet {
            mob_index: 2,
            key: "zombies:target".into(),
        });
        roundtrip(HostCall::MobKvSet {
            mob_index: 2,
            key: "zombies:target".into(),
            value: vec![0xFF],
        });
        roundtrip(HostCall::MobKvDelete {
            mob_index: 2,
            key: "zombies:target".into(),
        });
        roundtrip(HostCall::ResolveBlock {
            key: "smoke:smoke_block".into(),
        });
        roundtrip(HostCall::RegisterWorldgenFeature {
            feature_id: 3,
            stage: WorldgenStage::Trees,
        });
        roundtrip(HostCall::RegisterStageReplacement {
            stage: WorldgenStage::Terrain,
            callback_id: 9,
        });
        roundtrip(HostCall::RegisterGenerator { callback_id: 1 });
        roundtrip(HostCall::GuiStateSet {
            key: "wheel:angle".into(),
            value: GuiValue::F32(1.25),
        });
        roundtrip(HostCall::GuiStateGet {
            key: "wheel:result".into(),
        });
        roundtrip(HostCall::GuiOpen {
            kind_key: "wheel:wheel".into(),
        });
        roundtrip(HostCall::GuiClose);
        roundtrip(HostCall::SoundPlayAt {
            key: "zombies:groan".into(),
            pos: [4.5, 64.0, -2.5],
            volume: 0.8,
            pitch: 0.95,
        });
        roundtrip(HostCall::SoundPlayOnMob {
            mob_id: 42,
            key: "zombies:groan".into(),
            volume: 0.7,
            pitch: 1.05,
        });
        roundtrip(HostCall::SoundStop { handle: 99 });
        roundtrip(HostCall::BlockIsFullSpawnSupport { pos: [8, 63, 8] });
        roundtrip(HostCall::ShaderSetParam {
            key: "llama:light".into(),
            value: [0.75, 0.0, 0.0, 1.0],
        });
        roundtrip(HostCall::RegisterHostileSpawner {
            callback_id: 7,
            priority: -1,
        });
        roundtrip(HostRet::GuiValue(Some(GuiValue::Str(
            "llama:diamond".into(),
        ))));
        roundtrip(HostRet::GuiValue(Some(GuiValue::I32(-3))));
        roundtrip(HostRet::GuiValue(None));
        roundtrip(GuestCall::GuiClick {
            kind_key: "wheel:wheel".into(),
            widget_id: "spin".into(),
            pos: Some([4, 65, -2]),
        });
        let candidate = HostileSpawnCandidate {
            pos: [10.5, 64.0, -2.5],
            cell: [10, 64, -3],
            combined_light: 12,
            sky_light: 8,
            block_light: 12,
        };
        roundtrip(GuestCall::HostileSpawnCandidate {
            callback_id: 7,
            candidate: candidate.clone(),
        });
        roundtrip(HostCall::RegisterBlockBehavior {
            key: "mymod:zapper".into(),
            callback_id: 3,
        });
        roundtrip(GuestCall::BlockBehavior {
            callback_id: 3,
            kind: BlockHookKind::ScheduledTick,
            pos: [4, 65, -2],
        });
        roundtrip(HostCall::RegisterAiNode {
            key: "mymod:levitate".into(),
            callback_id: 9,
        });
        roundtrip(GuestCall::AiNode {
            callback_id: 9,
            ctx: AiNodeCtx {
                mob_id: 42,
                pos: [1.5, 64.0, -3.5],
                cell: [1, 64, -4],
                yaw: 0.5,
                player_pos: [8.0, 65.0, 8.0],
                nav_idle: true,
                in_water: false,
            },
        });
        roundtrip(GuestRet::AiDecision(Some(AiNodeDecision {
            goal: Some([3, 64, 2]),
            head_look: None,
            idle_anim: Some(1),
            attack: Some([2.0, 6.0]),
        })));
        roundtrip(EventPayload::ContainerOpened {
            kind: ContainerKind::Mod {
                key: "wheel:wheel".into(),
            },
            pos: None,
        });
        roundtrip(GuestCall::GenFeature {
            feature_id: 3,
            section_pos: [-2, 4, 7],
            seed: 0x312,
            blocks: vec![0; 8],
            surface_heights: vec![63; 4],
            biomes: vec![1; 4],
            sea_level: 63,
        });
        roundtrip(GuestCall::GenStage {
            callback_id: 9,
            stage: WorldgenStage::Climate,
            section_pos: [5, 0, -1],
            seed: 1,
            blocks: Vec::new(),
            surface_heights: vec![70; 2],
            biomes: vec![2; 2],
            sea_level: 63,
        });
        roundtrip(GuestRet::GenWrites(vec![([1, 64, -3], BlockId(7))]));
        roundtrip(GuestRet::GenBlocks(vec![1, 0, 1]));
        roundtrip(GuestRet::GenBiomes(vec![4, 4, 5]));
        roundtrip(GuestRet::HostileSpawn(Some("zombies:zombie".into())));
        roundtrip(GuestRet::HostileSpawn(None));
        roundtrip(HostRet::Unit);
        roundtrip(HostRet::U64(u64::MAX));
        roundtrip(HostRet::Error("nope".into()));
        roundtrip(HostRet::Bool(true));
        roundtrip(HostRet::Block(Some(BlockId(9))));
        roundtrip(HostRet::Blocks(vec![None, Some(BlockId(0))]));
        roundtrip(HostRet::Light {
            combined: 63,
            sky: 63,
            block: 40,
        });
        roundtrip(HostRet::Mobs(vec![MobSnapshot {
            index: 0,
            key: "llama:owl".into(),
            pos: [1.5, 64.0, -3.5],
            health: 4.0,
            id: 123,
        }]));
        roundtrip(HostRet::Player(PlayerSnapshot {
            pos: [0.5, 80.0, 0.5],
            vel: [0.0, -1.0, 0.0],
            yaw: 0.5,
            pitch: -0.25,
            health: 17,
            on_ground: false,
            spectator: false,
        }));
        roundtrip(HostRet::Bytes(Some(vec![1, 2, 3])));
        roundtrip(GuestRet::Event {
            outcome: Outcome::Continue,
            payload: EventPayload::PlayerDamagePre {
                amount: 2,
                source: DamageSource::Mob {
                    key: "zombies:zombie".into(),
                },
            },
        });
        roundtrip(GuestCall::TickSystem { id: 3 });
        roundtrip(GuestCall::HandleEvent {
            id: 1,
            kind: EventKind::MobHurtPre,
            payload: EventPayload::MobHurtPre {
                mob: 5,
                kind: MobId(1),
                amount: 2.5,
                source: [1.0, -2.0, 0.5],
            },
        });
        roundtrip(GuestRet::Event {
            outcome: Outcome::Cancel,
            payload: EventPayload::PlayerDamagePre {
                amount: -4,
                source: DamageSource::Fall,
            },
        });
        roundtrip(EventPayload::ContainerOpened {
            kind: ContainerKind::Furnace,
            pos: Some([1, -64, 3]),
        });

        for (ptr, len) in [(0, 0), (1, u32::MAX), (u32::MAX, 17), (0x1234_5678, 9)] {
            assert_eq!(unpack_ptr_len(pack_ptr_len(ptr, len)), (ptr, len));
        }
    }

    #[test]
    fn every_payload_kind_is_registerable() {
        // kind() is the dispatch routing key: it must agree with the variant.
        let samples = [
            EventPayload::PlayerDied,
            EventPayload::ItemUsed { item: ItemId(3) },
            EventPayload::SectionLoaded { pos: [0, -2, 5] },
        ];
        for s in samples {
            let bytes = encode(&s).unwrap();
            let back: EventPayload = decode(&bytes).unwrap();
            assert_eq!(back.kind(), s.kind());
        }
    }
}
