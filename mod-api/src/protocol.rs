//! The four wire enums — [`HostCall`]/[`HostRet`] (guest→host request/reply)
//! and [`GuestCall`]/[`GuestRet`] (host→guest) — plus the worldgen write
//! alias. Evolution rules live in the crate docs; the recorded encoding
//! lives in `wire_pin`.

use serde::{Deserialize, Serialize};

use crate::client::{
    ClientCanvasElement, ClientCanvasEvent, ClientFrameData, ClientOverlayAnchor,
    ClientSurfaceColumn, ClientSurfaceQuery, ClientTextRun, ClientUiEvent,
};
use crate::data::{
    AiNodeCtx, AiNodeDecision, BlockHookKind, EffectStateData, GuiValue, HostileSpawnCandidate,
    ItemInfoData, ItemStackData, MobSnapshot, PlayerSnapshot, RuntimeSide,
};
use crate::events::{EventKind, EventPayload, Outcome};
use crate::ids::{BlockId, ItemId};
use crate::sched::{AttachSide, Stage, WorldgenStage};

/// Guest → host: what a mod asks the engine for through `host_dispatch`.
/// Phase 2b surface + the Phase 3b world/entity/player/KV calls (one match on
/// the host).
///
/// The world-touching calls are sim-scoped: legal wherever a `SimCtx` is
/// published (`mod_init`, tick systems, event handlers), [`HostRet::Error`]
/// outside any guest dispatch.
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
    // --- Phase 3b: blocks -------------------------------------------------
    /// The block at a world cell: `Some` (air included) when its section is
    /// loaded, `None` when unloaded / outside the vertical range.
    /// → [`HostRet::Block`].
    GetBlock {
        pos: [i32; 3],
    },
    /// Batched [`HostCall::GetBlock`], one result per position in order.
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
    /// ABI crossing, not the world work. → [`HostRet::U64`] (cells actually set).
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
    /// combined = max(sky, block). Unloaded cells read as open sky / no block
    /// light (the engine's own fallbacks). → [`HostRet::Light`].
    LightAt {
        pos: [i32; 3],
    },

    // --- Phase 3b: entities -----------------------------------------------
    /// Spawn a mob by species key at `pos` (feet) facing `yaw`.
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
    MobsInRadius {
        pos: [f32; 3],
        radius: f32,
    },
    /// Damage the mob at `index` through its global engine-owned i-frames and
    /// the `mob_damage_pre` pipeline. Mod damage is not an attack, so default
    /// knockback is not applied; `origin` is only spatial context for
    /// feedback/handlers. Applied at the next action drain point (same tick),
    /// so a handler cannot re-enter the bus. → [`HostRet::Unit`].
    DamageMob {
        index: u32,
        amount: f32,
        origin: Option<[f32; 3]>,
    },
    /// Remove the mob at `index` from the live world immediately (not saved,
    /// no death/loot). Renumbers later indices — re-query after use.
    /// `false` = no such mob. → [`HostRet::Bool`].
    DespawnMob {
        index: u32,
    },
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
    /// Damage the player through the single engine funnel. The victim's global
    /// engine-owned i-frames and `player_damage_pre` apply, with
    /// [`DamageSource::Mod`] carrying the calling mod's id. Queued; applied at
    /// the next action drain point (same tick, defined order). →
    /// [`HostRet::Unit`].
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
    /// Give the player `count` of an item (by registry key) through the normal
    /// inventory fill; whatever doesn't fit drops at the player's feet like any
    /// other overflow. `false` = unknown key. → [`HostRet::Bool`].
    GiveItem {
        item_key: String,
        count: u8,
    },
    /// Kill the player: damage equal to current health, through the same
    /// funnel (and queue) as [`HostCall::DamagePlayer`] — global i-frames or a
    /// pre-event handler can still reject it. → [`HostRet::Unit`].
    KillPlayer,
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

    // --- Phase 3b: sound ----------------------------------------------------
    /// Play a sound by `sounds.json` key (namespaced for pack sounds), routed
    /// through the tick→presentation channel — the sim never touches audio.
    /// `pos` attenuates by the sound row's `attenuation_distance`; `None`
    /// plays at full volume. `false` = unknown key. → [`HostRet::Bool`].
    EmitSound {
        key: String,
        pos: Option<[f32; 3]>,
    },

    // --- Phase 3b: persistent KV -------------------------------------------
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
    /// Per-mob KV riding the mob's save record (`mob_index` as in
    /// [`MobSnapshot::index`] — valid this tick only). → [`HostRet::Bytes`].
    MobKvGet {
        mob_index: u32,
        key: String,
    },
    /// `false` = no such mob. → [`HostRet::Bool`].
    MobKvSet {
        mob_index: u32,
        key: String,
        value: Vec<u8>,
    },
    /// → [`HostRet::Bool`] (whether the key was present).
    MobKvDelete {
        mob_index: u32,
        key: String,
    },

    // --- Phase 4: worldgen hooks ---------------------------------------------
    /// Resolve a block registry key (`"petramond:stone"`, `"kitchen:oven"`) to its
    /// session-scoped runtime id. Needs no simulation context — legal anywhere,
    /// including on worldgen instances. `None` = not registered (a typo'd or
    /// absent pack — degrade gracefully, don't panic). → [`HostRet::Block`].
    ResolveBlock {
        key: String,
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

    // --- Phase 5: mod GUIs ----------------------------------------------------
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
    /// those player ids only (unknown / already-left ids are ignored). Empty
    /// / whitespace-only text is a no-op (`Bool(false)`). → [`HostRet::Bool`].
    ChatSend {
        text: String,
        targets: Option<Vec<u8>>,
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

    // --- Bugfix round 1 (spawning): mod-visible full-block support --------
    /// Whether the loaded block at `pos` is valid full-cube spawn support:
    /// one full collision cube, not water, not leaves. Partial shapes such as
    /// stairs, doors, and model blocks return false, as do unloaded/out-of-range
    /// cells. → [`HostRet::Bool`].
    BlockIsFullSpawnSupport {
        pos: [i32; 3],
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

    // --- Block behaviors (Phase 2b, landed 2026-07-06) ---------------------
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
    /// to it — size against `ItemInfo.max_stack` if the overflow matters. →
    /// [`HostRet::Bool`] (`false` = unloaded, or an unknown item key — the
    /// batch is not applied).
    ContainerSet {
        pos: [i32; 3],
        slots: Vec<(u32, Option<ItemStackData>)>,
    },
    /// Read one item's registry data: stack cap, fuel burn ticks, and tag
    /// names — the same rows engine mechanics read, so mod logic (a fuel-fired
    /// oven, a filtering hopper) composes with pack-added items for free.
    /// `None` = unknown key. → [`HostRet::ItemInfo`].
    ItemInfo {
        key: String,
    },
    /// The loaded machine-processing result for one input item key under a
    /// recipe `class` (`"petramond:smelting"` = the furnace's table; a mod machine
    /// names its own, e.g. `"kitchen:cooking"`), from the same layered
    /// `recipes.json` catalog engine machines cook from — any pack's rows for
    /// that class included. `None` = no recipe. → [`HostRet::ItemStack`].
    RecipeResult {
        class: String,
        key: String,
    },

    // --- Player status effects (landed 2026-07-07) --------------------------
    /// Grant the player the status effect registered under `key` (an
    /// `effects.json` row — engine `petramond:*` rows and every pack's rows alike)
    /// for `ticks` game ticks. An already-active effect is OVERWRITTEN with
    /// the new duration; `ticks == 0` removes it. Like `SetHealth` this is a
    /// state primitive: no events fire. → [`HostRet::Bool`] (`false` =
    /// unknown effect key).
    EffectApply {
        key: String,
        ticks: u32,
    },
    /// Remove the status effect `key` from the player if active. →
    /// [`HostRet::Bool`] (`false` = unknown effect key).
    EffectRemove {
        key: String,
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
    /// `ContainerGet` per machine — the per-block-per-tick hot-loop rule. →
    /// [`HostRet::Containers`], parallel to the positions.
    ContainerGetMany {
        positions: Vec<[i32; 3]>,
    },

    // --- Mob particle emitters (landed 2026-07-10) ---------------------------
    /// Toggle one KEYED particle-emitter bundle on the mob at `index` (a
    /// [`MobSnapshot::index`], valid this tick only). `key` names a
    /// `particle_emitters.json` catalog row (engine `petramond:*` rows —
    /// `petramond:burn_light`, `petramond:burn_great` — and every pack's rows
    /// alike, the same cross-namespace rule as effects): one or more particle
    /// rows plus an optional body tint. The active set (≤ 4 per mob) is
    /// presentation-only, replicates to every client, survives death (a corpse
    /// keeps its effect through the ragdoll), and is NOT persisted: the owning
    /// mod re-derives it, e.g. from its own per-mob state. →
    /// [`HostRet::Bool`] (`false` = bad index, unregistered key, or the mob's
    /// active set is full).
    MobEmitterSet {
        index: u32,
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
    /// (e.g. `item_use_pre`) without persisting numeric ids.
    /// → [`HostRet::Item`].
    ResolveItem {
        key: String,
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
    Bytes(#[serde(with = "serde_bytes")] Option<Vec<u8>>),
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
    TickSystem {
        id: u32,
    },
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
    AiNode {
        callback_id: u32,
        ctx: AiNodeCtx,
    },

    // --- Presentation-only client module ----------------------------------
    ClientFrame {
        frame: ClientFrameData,
    },
    ClientKey {
        action_id: u32,
        pressed: bool,
    },
    ClientUi {
        kind_key: String,
        event: ClientUiEvent,
    },
    ClientCanvas {
        canvas_key: String,
        event: ClientCanvasEvent,
    },
    /// Mouse-wheel travel over this module's open modal canvas. `x`/`y` are
    /// canvas-local logical pixels (the cursor position), `delta` is in wheel
    /// notches with positive = scrolled up / away from the user. The host
    /// coalesces wheel events to at most one call per app frame.
    ClientCanvasScroll {
        canvas_key: String,
        x: f32,
        y: f32,
        delta: f32,
    },
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
    GenBlocks(#[serde(with = "serde_bytes")] Vec<u8>),
    /// Reply to a `Climate` [`GuestCall::GenStage`]: the 256-entry column
    /// biome map (`z*16 + x`). Must be exactly 256 valid biome ids.
    GenBiomes(#[serde(with = "serde_bytes")] Vec<u8>),
    /// Reply to [`GuestCall::HostileSpawnCandidate`]: `Some(registry_key)` to
    /// ask core to spawn that hostile species here, `None` to reject this site.
    HostileSpawn(Option<String>),
    /// Reply to [`GuestCall::AiNode`]: the node's desires for this mob this
    /// tick (`None` = no opinion on anything, same as the default decision).
    AiDecision(Option<AiNodeDecision>),
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use crate::*;

    /// The ABI contract both sides rely on: every call/reply enum round-trips
    /// through postcard, including nested payloads. (No wire-byte pinning — the encoding is postcard's contract;
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
        roundtrip(HostCall::DamageMob {
            index: 3,
            amount: 2.5,
            origin: Some([1.0, 64.0, 1.0]),
        });
        roundtrip(HostCall::DespawnMob { index: 7 });
        roundtrip(HostCall::MobEmitterSet {
            index: 5,
            key: "petramond:burn_light".into(),
            active: true,
        });
        roundtrip(HostCall::EmitterBurst {
            key: "petramond:water_splash".into(),
            pos: [0.5, 64.0, 0.5],
            intensity: 4.5,
        });
        roundtrip(HostCall::SpawnItem {
            item_key: "petramond:stick".into(),
            count: 4,
            pos: [0.5, 64.0, 0.5],
        });
        roundtrip(HostCall::PlayerState);
        roundtrip(HostCall::DamagePlayer { amount: 4 });
        roundtrip(HostCall::ApplyKnockback {
            impulse: [1.0, 3.0, -1.0],
        });
        roundtrip(HostCall::GiveItem {
            item_key: "petramond:diamond".into(),
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
            key: "petramond:time".into(),
        });
        roundtrip(HostCall::WorldKvSet {
            key: "petramond:time".into(),
            value: vec![1, 2, 3],
        });
        roundtrip(HostCall::WorldKvDelete {
            key: "petramond:time".into(),
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
            key: "kitchen:oven".into(),
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
        roundtrip(HostCall::ChatSend {
            text: "$[fg=yellow]Hello".into(),
            targets: None,
        });
        roundtrip(HostCall::ChatSend {
            text: "whisper".into(),
            targets: Some(vec![0, 2]),
        });
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
            key: "petramond:light".into(),
            value: [0.75, 0.0, 0.0, 1.0],
        });
        roundtrip(HostCall::RegisterHostileSpawner {
            callback_id: 7,
            priority: -1,
        });
        roundtrip(HostCall::RuntimeSide);
        roundtrip(HostCall::ClientRegisterOverlay {
            image_key: "minimap:hud".into(),
            anchor: ClientOverlayAnchor::TopRight,
            margin: [8, 8],
            display_size: [256, 256],
        });
        roundtrip(HostCall::ClientRegisterKey {
            id: "open_map".into(),
            label: "Open World Map".into(),
            key: "key_m".into(),
            action_id: 1,
        });
        roundtrip(HostCall::ClientSurfaceColumns {
            queries: vec![
                ClientSurfaceQuery {
                    coord: [-12, 34],
                    revision: 0,
                },
                ClientSurfaceQuery {
                    coord: [3, -4],
                    revision: 17,
                },
            ],
        });
        roundtrip(HostCall::ClientImageBlit {
            key: "minimap:full_tile_0".into(),
            origin: [32, 64],
            size: [2, 1],
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
        });
        roundtrip(HostCall::ClientUiStateSet {
            key: "minimap:waypoint_name".into(),
            value: GuiValue::Str("Home".into()),
        });
        roundtrip(HostCall::ClientUiStateGet {
            key: "minimap:waypoint_name".into(),
        });
        roundtrip(HostCall::ClientImageSet {
            key: "minimap:hud".into(),
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
        });
        roundtrip(HostCall::ClientTextMeasure {
            text: "Waypoint".into(),
            scale: 2,
        });
        roundtrip(HostCall::ClientImageDrawTexts {
            key: "minimap:hud".into(),
            runs: vec![ClientTextRun {
                text: "W".into(),
                position: [4, 9],
                scale: 2,
                color: [255, 255, 255, 255],
            }],
        });
        roundtrip(HostCall::ClientGuiOpen {
            kind_key: "minimap:edit_waypoint".into(),
        });
        roundtrip(HostCall::ClientGuiClose);
        roundtrip(HostCall::ClientCanvasOpen {
            canvas_key: "minimap:full_map".into(),
            size: [640, 640],
        });
        roundtrip(HostCall::ClientCanvasClose);
        roundtrip(HostCall::ClientCanvasSceneSet {
            canvas_key: "minimap:full_map".into(),
            elements: vec![
                ClientCanvasElement::Image {
                    image_key: "minimap:tile_0".into(),
                    rect: [0.0, 0.0, 160.0, 160.0],
                },
                ClientCanvasElement::Sprite {
                    image_key: "minimap:player_arrow".into(),
                    center: [160.0, 160.0],
                },
            ],
        });
        roundtrip(HostCall::ClientCanvasViewSet {
            canvas_key: "minimap:full_map".into(),
            offset: [-80.0, 24.0],
        });
        roundtrip(HostCall::ClientStorageReadBegin {
            keys: vec!["minimap:tile:0:0".into(), "minimap:tile:1:0".into()],
        });
        roundtrip(HostCall::ClientStorageReadPoll { ticket: 7 });
        roundtrip(HostRet::ClientStorageRead(None));
        roundtrip(HostRet::ClientStorageRead(Some(vec![
            Some(ByteBuf::from(vec![1, 2, 3])),
            None,
        ])));
        roundtrip(HostCall::ClientStorageGetMany {
            keys: vec!["minimap:tile/-1/2".into(), "minimap:waypoints".into()],
        });
        roundtrip(HostCall::ClientStorageSetMany {
            entries: vec![("minimap:tile/-1/2".into(), ByteBuf::from(vec![7, 8, 9]))],
        });
        roundtrip(HostRet::RuntimeSide(RuntimeSide::Client));
        roundtrip(HostRet::ClientSurfaceColumns(vec![
            None,
            Some(ClientSurfaceColumn {
                revision: 9,
                cells: None,
            }),
            Some(ClientSurfaceColumn {
                revision: 12,
                cells: Some(vec![71, 0, 42, 96, 31]),
            }),
        ]));
        roundtrip(HostRet::ClientStorageValues(vec![
            Some(ByteBuf::from(vec![3, 1, 4])),
            None,
        ]));
        roundtrip(GuestCall::ClientFrame {
            frame: ClientFrameData {
                dt: 1.0 / 60.0,
                player_pos: [4.5, 72.0, -8.5],
                yaw: 1.25,
                pitch: -0.1,
                screen: [1920, 1080],
                open_gui: None,
                open_canvas: Some("minimap:full_map".into()),
            },
        });
        roundtrip(GuestCall::ClientKey {
            action_id: 2,
            pressed: true,
        });
        roundtrip(GuestCall::ClientUi {
            kind_key: "minimap:edit_waypoint".into(),
            event: ClientUiEvent::ImagePointer {
                id: "map".into(),
                phase: ClientPointerPhase::Move,
                x: 120.5,
                y: 64.25,
                button: ClientPointerButton::Primary,
            },
        });
        roundtrip(GuestCall::ClientCanvas {
            canvas_key: "minimap:full_map".into(),
            event: ClientCanvasEvent {
                phase: ClientPointerPhase::Move,
                x: 120.5,
                y: 64.25,
                button: ClientPointerButton::Primary,
            },
        });
        roundtrip(GuestCall::ClientCanvasScroll {
            canvas_key: "minimap:full_map".into(),
            x: 120.5,
            y: 64.25,
            delta: -2.0,
        });
        roundtrip(HostRet::GuiValue(Some(GuiValue::Str(
            "petramond:diamond".into(),
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
            nearest_player_dist: 40.0,
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
            key: "petramond:owl".into(),
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
                source: DamageSource::MobAttack {
                    key: "zombies:zombie".into(),
                },
                origin: Some([0.0, 80.0, 0.0]),
            },
        });
        roundtrip(GuestCall::TickSystem { id: 3 });
        roundtrip(GuestCall::HandleEvent {
            id: 1,
            kind: EventKind::MobDamagePre,
            payload: EventPayload::MobDamagePre {
                mob: 5,
                kind: MobId(1),
                amount: 2.5,
                source: DamageSource::PlayerAttack { id: 0 },
                origin: Some([1.0, -2.0, 0.5]),
                feedback: MobDamageFeedback::default(),
            },
        });
        roundtrip(GuestRet::Event {
            outcome: Outcome::Cancel,
            payload: EventPayload::PlayerDamagePre {
                amount: -4,
                source: DamageSource::Fall,
                origin: None,
            },
        });
        roundtrip(EventPayload::ContainerOpened {
            kind: ContainerKind::Furnace,
            pos: Some([1, -64, 3]),
        });
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
