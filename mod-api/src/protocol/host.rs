use serde::{Deserialize, Serialize};

pub use super::guest::GuestCall;
use crate::client::{
    ClientCanvasElement, ClientOverlayAnchor, ClientSurfaceColumn, ClientSurfaceQuery,
    ClientTextRun,
};
use crate::data::{
    EffectStateData, GuiValue, ItemInfoData, ItemStackData, MobAnimStateData, MobRidersData,
    MobSnapshot, PlayerInputData, PlayerListEntry, PlayerSnapshot, RuntimeSide,
};
use crate::events::EventKind;
use crate::ids::{BlockId, ItemId};
use crate::sched::{AttachSide, Stage, WorldgenStage};

/// Guest → host: what a mod asks the engine for through `host_dispatch`.
/// One exhaustive match on the host routes each variant to its domain
/// handler (`src/modding/host/`).
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
    // --- blocks -------------------------------------------------------------
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

    // --- entities -----------------------------------------------------------
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
    ///
    /// `feedback` composes the damage pipeline for THIS request; `None` uses
    /// the species' resolved `damage_feedback`. A pipeline without the
    /// `Immunity` component is damage-over-time (burn): neither blocked by
    /// the victim's active i-frame window nor granting one.
    DamageMob {
        index: u32,
        amount: f32,
        origin: Option<[f32; 3]>,
        feedback: Option<crate::events::MobDamageFeedback>,
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

    // --- player ---------------------------------------------------------------
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

    // --- worldgen hooks --------------------------------------------------------
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
    /// Seat player `player_id` in `seat` of the live mob `mob_id` (stable
    /// id). Validated by the engine: the mob is alive and its species row
    /// declares that seat (`seats` in `mobs.json`), the seat is free, and the
    /// player is not already mounted. WHO may sit WHERE is the calling mod's
    /// policy — usually decided in its `mob_interact` handler. From this tick
    /// the engine slaves the rider to the seat; every detach path announces
    /// [`EventKind::PlayerDismounted`]. → [`HostRet::Bool`].
    ///
    /// [`EventKind::PlayerDismounted`]: crate::EventKind::PlayerDismounted
    MobMount {
        mob_id: u64,
        player_id: u8,
        seat: u8,
    },
    /// Unseat `player_id` from whatever they ride (the mod-initiated detach;
    /// the engine's own valves — sneak gesture, death, despawn — detach
    /// without this call). `false` = they were not mounted.
    /// → [`HostRet::Bool`].
    MobDismount {
        player_id: u8,
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
        player_id: u8,
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
    /// Spawn a mob only when its COMPLETE declared body fits at `pos`/`yaw`:
    /// every covered section is loaded and stream-final, no terrain collision
    /// shape overlaps, and no live solid mob overlaps. The validation and
    /// insertion are one atomic sim operation. `false` = unknown key, blocked,
    /// unloaded or unresolved pose, or the mob cap is reached. → [`HostRet::Bool`].
    ///
    /// Appended for wire compatibility; conceptually this is the checked
    /// sibling of [`HostCall::SpawnMob`].
    SpawnMobChecked {
        key: String,
        pos: [f32; 3],
        yaw: f32,
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
    /// [`HostCall::MobRiders`]: `None` = no such live mob.
    Riders(Option<MobRidersData>),
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
}
