//! Guest-side SDK for llamacraft mods.
//!
//! A mod implements [`Mod`], calls [`register_mod!`], and builds with plain
//! `cargo build --target wasm32-unknown-unknown` (see `mods-src/`). The SDK
//! owns the raw ABI — the `mod_alloc`/`mod_free`/`mod_dispatch` exports, the
//! `host_dispatch` import, postcard framing, pointer packing — so mod code
//! only ever sees `mod-api` types (re-exported here) and the safe wrappers
//! ([`log`], [`current_tick`], [`rng_u64`], [`register_tick_system`],
//! [`register_event_handler`]).
//!
//! Determinism contract (WIKI/modding.md): mod code runs only inside
//! `mod_init`, tick systems, and event handlers; randomness only through
//! [`rng_u64`]'s seeded host streams; no clock, no filesystem, no entropy.
//! A panic aborts the guest (the host logs it, disables the mod for the
//! session, and keeps ticking).

pub use mod_api::*;

/// A mod's logic. One instance lives for the whole session (state persists
/// between dispatches — in-memory only; persistent storage is Phase 3).
///
/// # Worldgen instances are separate
///
/// If the mod registers worldgen hooks, the engine ALSO instantiates it on
/// each worldgen worker thread (and lazily on any thread that generates
/// terrain). Those instances share NOTHING with the tick instance — separate
/// wasm memories, separate `Self` state. Their `init` runs too, so keep `init`
/// PURE: registrations are accepted (and simply ignored off the main
/// instance), [`resolve_block`]/[`log`]/[`rng_u64`] work everywhere, but any
/// sim-scoped call (world/entity/player/KV/env) returns an error there — and
/// the SDK wrappers turn that into a panic that disables the instance.
pub trait Mod: Default {
    /// The registration window: call [`register_tick_system`] /
    /// [`register_event_handler`] / [`register_worldgen_feature`] /
    /// [`register_stage_replacement`] / [`register_generator`] here (they are
    /// rejected anywhere else).
    fn init(&mut self);

    /// A tick system registered under `system_id` is due. Runs once per game
    /// tick (20/s) at its registered stage attachment.
    fn tick_system(&mut self, _system_id: u32) {}

    /// An event the mod registered for. For pre events the payload is echoed
    /// back mutated — the engine applies the taxonomy's mutable fields (e.g.
    /// damage `amount`) — and the returned [`Outcome`] can cancel; post events
    /// are observe-only (the outcome is ignored).
    fn handle_event(&mut self, _handler_id: u32, _payload: &mut EventPayload) -> Outcome {
        Outcome::Continue
    }

    /// A worldgen feature registered under `feature_id`, dispatched once per
    /// generated 16³ section. Return the feature's block writes in world
    /// coordinates; the engine clips them to the dispatched section. MUST be a
    /// pure function of `ctx` — see [`GenCtx`] for the full seam/determinism
    /// contract and the helpers that get it right by default.
    fn gen_feature(&mut self, _feature_id: u32, _ctx: &GenCtx) -> Vec<GenWrite> {
        Vec::new()
    }

    /// A registered `Climate` stage replacement: return the 256-entry column
    /// biome map (`z*16 + x`; `ctx.biomes()` carries the engine's proposal).
    /// Anything but exactly 256 valid biome ids disables the mod and the
    /// engine's climate runs instead.
    fn gen_climate(&mut self, _callback_id: u32, _ctx: &GenCtx) -> Vec<u8> {
        Vec::new()
    }

    /// A registered `Terrain` stage replacement: return the section's complete
    /// 4096-block fill (layout `y*256 + z*16 + x`). Anything but exactly 4096
    /// registered block ids disables the mod and the engine terrain runs
    /// instead.
    fn gen_terrain(&mut self, _callback_id: u32, _ctx: &GenCtx) -> Vec<u8> {
        Vec::new()
    }

    /// A registered `Underground`/`Vegetation`/`Trees` stage replacement:
    /// like [`Mod::gen_feature`], but the write list runs INSTEAD of the
    /// engine stage.
    fn gen_stage(
        &mut self,
        _callback_id: u32,
        _stage: WorldgenStage,
        _ctx: &GenCtx,
    ) -> Vec<GenWrite> {
        Vec::new()
    }

    /// A button of this mod's own GUI was clicked (dispatched on the tick, in
    /// click order). `kind_key` is the GUI's registered kind, `widget_id` the
    /// manifest button id, and `pos` the block the GUI was opened from
    /// (`None` for a programmatic [`gui_open`]). Typical handling: update the
    /// session's state map via [`gui_state_set`] so the GUI's `label` /
    /// `rotimage` widgets redraw.
    fn gui_click(&mut self, _kind_key: &str, _widget_id: &str, _pos: Option<[i32; 3]>) {}

    /// Core is asking whether this candidate should spawn one of this mod's
    /// hostile species. Return a mob registry key to request a spawn, or `None`
    /// to let core keep searching. Core still validates category, caps, and
    /// physical body fit before spawning.
    fn hostile_spawn_candidate(
        &mut self,
        _callback_id: u32,
        _candidate: &HostileSpawnCandidate,
    ) -> Option<String> {
        None
    }
}

/// Log a line through the engine's logger.
pub fn log(msg: &str) {
    __rt::host_call(&HostCall::Log { msg: msg.into() });
}

/// The current game tick (20 per second).
pub fn current_tick() -> u64 {
    match __rt::host_call(&HostCall::CurrentTick) {
        HostRet::U64(tick) => tick,
        other => panic!("CurrentTick returned {other:?}"),
    }
}

/// Next value of the named deterministic RNG stream (seeded per world seed +
/// mod id + key; use distinct keys for independent streams).
pub fn rng_u64(stream_key: &str) -> u64 {
    let call = HostCall::RngU64 {
        stream_key: stream_key.into(),
    };
    match __rt::host_call(&call) {
        HostRet::U64(v) => v,
        other => panic!("RngU64 returned {other:?}"),
    }
}

/// Attach a tick system at a stage seam. Only legal during [`Mod::init`];
/// `system_id` is echoed to [`Mod::tick_system`]. Systems at one seam run in
/// `(priority ascending, registration order)`.
pub fn register_tick_system(stage: Stage, attach: AttachSide, priority: i32, system_id: u32) {
    __rt::expect_unit(
        "RegisterTickSystem",
        __rt::host_call(&HostCall::RegisterTickSystem {
            stage,
            attach,
            priority,
            system_id,
        }),
    );
}

/// Register an event handler. Only legal during [`Mod::init`]; `handler_id`
/// is echoed to [`Mod::handle_event`].
pub fn register_event_handler(event: EventKind, priority: i32, handler_id: u32) {
    __rt::expect_unit(
        "RegisterEventHandler",
        __rt::host_call(&HostCall::RegisterEventHandler {
            event,
            priority,
            handler_id,
        }),
    );
}

/// Register a callback that core may ask for hostile spawns. Only legal during
/// [`Mod::init`]; callbacks run in `(priority ascending, registration order)`.
pub fn register_hostile_spawner(priority: i32, callback_id: u32) {
    __rt::expect_unit(
        "RegisterHostileSpawner",
        __rt::host_call(&HostCall::RegisterHostileSpawner {
            callback_id,
            priority,
        }),
    );
}

/// Set one named visual shader parameter (`vec4<f32>`). `key` must be in this
/// mod's namespace (`mod_id:name`) or an exposed engine `llama:*` key. Shader
/// packs map names onto fixed GPU slots.
pub fn shader_set_param(key: &str, value: [f32; 4]) {
    __rt::expect_unit(
        "ShaderSetParam",
        __rt::host_call(&HostCall::ShaderSetParam {
            key: key.into(),
            value,
        }),
    );
}

// --- Phase 3b: blocks -------------------------------------------------------

/// The block at a world cell, or `None` when its section is unloaded / the
/// cell is outside the world's vertical range. Air is `Some(BlockId(0))`.
pub fn get_block(pos: [i32; 3]) -> Option<BlockId> {
    match __rt::host_call(&HostCall::GetBlock { pos }) {
        HostRet::Block(b) => b,
        other => panic!("GetBlock returned {other:?}"),
    }
}

/// Batched [`get_block`]: one result per position, in order.
pub fn get_blocks(positions: Vec<[i32; 3]>) -> Vec<Option<BlockId>> {
    match __rt::host_call(&HostCall::GetBlocks { positions }) {
        HostRet::Blocks(b) => b,
        other => panic!("GetBlocks returned {other:?}"),
    }
}

/// Set one block through the engine's full edit path (relight, neighbour
/// updates). Returns `false` when the cell is unloaded / out of range.
pub fn set_block(pos: [i32; 3], block: BlockId) -> bool {
    match __rt::host_call(&HostCall::SetBlock { pos, block }) {
        HostRet::Bool(ok) => ok,
        other => panic!("SetBlock returned {other:?}"),
    }
}

/// Batched [`set_block`]; returns how many cells were actually set. Each write
/// still pays its own relight/remesh — batch the ABI crossing, not a floodfill.
pub fn set_blocks(blocks: Vec<([i32; 3], BlockId)>) -> u64 {
    match __rt::host_call(&HostCall::SetBlocks { blocks }) {
        HostRet::U64(n) => n,
        other => panic!("SetBlocks returned {other:?}"),
    }
}

/// Run the cell's block behavior `scheduled_tick` in `delay` game ticks (first
/// schedule per cell wins).
pub fn schedule_tick(pos: [i32; 3], delay: u64) {
    __rt::expect_unit(
        "ScheduleTick",
        __rt::host_call(&HostCall::ScheduleTick { pos, delay }),
    );
}

/// Whether the section owning the cell is currently loaded.
pub fn is_loaded(pos: [i32; 3]) -> bool {
    match __rt::host_call(&HostCall::IsLoaded { pos }) {
        HostRet::Bool(loaded) => loaded,
        other => panic!("IsLoaded returned {other:?}"),
    }
}

/// Cached light at a cell as `(combined, sky, block)` on the 6-bit `0..=63`
/// scale (combined = max of the two channels).
pub fn light_at(pos: [i32; 3]) -> (u8, u8, u8) {
    match __rt::host_call(&HostCall::LightAt { pos }) {
        HostRet::Light {
            combined,
            sky,
            block,
        } => (combined, sky, block),
        other => panic!("LightAt returned {other:?}"),
    }
}

/// Whether the loaded block at `pos` is valid full-cube support for
/// programmatic mob spawns. Rejects unloaded cells, water, leaves, and partial
/// collision shapes such as stairs.
pub fn block_is_full_spawn_support(pos: [i32; 3]) -> bool {
    match __rt::host_call(&HostCall::BlockIsFullSpawnSupport { pos }) {
        HostRet::Bool(ok) => ok,
        other => panic!("BlockIsFullSpawnSupport returned {other:?}"),
    }
}

// --- Phase 3b: entities -------------------------------------------------------

/// Spawn a mob by species registry name at `pos` (feet) facing `yaw`. `false`
/// = unknown species or the mob cap is reached.
pub fn spawn_mob(key: &str, pos: [f32; 3], yaw: f32) -> bool {
    match __rt::host_call(&HostCall::SpawnMob {
        key: key.into(),
        pos,
        yaw,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("SpawnMob returned {other:?}"),
    }
}

/// Snapshot the live mobs within `radius` of `pos` (3-D, feet positions), in
/// the deterministic live-set storage order. Indices are valid this tick only.
pub fn mobs_in_radius(pos: [f32; 3], radius: f32) -> Vec<MobSnapshot> {
    match __rt::host_call(&HostCall::MobsInRadius { pos, radius }) {
        HostRet::Mobs(mobs) => mobs,
        other => panic!("MobsInRadius returned {other:?}"),
    }
}

/// Hurt a mob through the `mob_hurt_pre` pipeline, exactly like a player
/// attack (applied at the next in-tick drain point; a cancelling handler
/// blocks it).
pub fn hurt_mob(index: u32, amount: f32, from: [f32; 3]) {
    __rt::expect_unit(
        "HurtMob",
        __rt::host_call(&HostCall::HurtMob {
            index,
            amount,
            from,
        }),
    );
}

/// Remove a mob from the live world immediately (no death, no loot, not
/// saved). Renumbers later indices — re-query after use.
pub fn despawn_mob(index: u32) -> bool {
    match __rt::host_call(&HostCall::DespawnMob { index }) {
        HostRet::Bool(ok) => ok,
        other => panic!("DespawnMob returned {other:?}"),
    }
}

/// Spawn `count` of an item (registry key) as a dropped-item entity at `pos`.
/// `false` = unknown key or zero count.
pub fn spawn_item(item_key: &str, count: u8, pos: [f32; 3]) -> bool {
    match __rt::host_call(&HostCall::SpawnItem {
        item_key: item_key.into(),
        count,
        pos,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("SpawnItem returned {other:?}"),
    }
}

// --- Phase 3b: player ---------------------------------------------------------

/// The player's current state (position, velocity, look, health, flags).
pub fn player_state() -> PlayerSnapshot {
    match __rt::host_call(&HostCall::PlayerState) {
        HostRet::Player(p) => p,
        other => panic!("PlayerState returned {other:?}"),
    }
}

/// Damage the player through the engine funnel — `player_damage_pre` (other
/// mods' i-frames) applies, with `DamageSource::Mod` carrying this mod's id.
/// Queued; applied at the next in-tick drain point.
pub fn damage_player(amount: i32) {
    __rt::expect_unit(
        "DamagePlayer",
        __rt::host_call(&HostCall::DamagePlayer { amount }),
    );
}

/// Add a knockback impulse to the player's velocity (spectator no-op).
pub fn apply_knockback(impulse: [f32; 3]) {
    __rt::expect_unit(
        "ApplyKnockback",
        __rt::host_call(&HostCall::ApplyKnockback { impulse }),
    );
}

/// Give the player items through the normal inventory fill; overflow drops at
/// the player's feet. `false` = unknown item key.
pub fn give_item(item_key: &str, count: u8) -> bool {
    match __rt::host_call(&HostCall::GiveItem {
        item_key: item_key.into(),
        count,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("GiveItem returned {other:?}"),
    }
}

/// Kill the player: current-health damage through the same funnel (and queue)
/// as [`damage_player`] — i-frame handlers can still cancel it.
pub fn kill_player() {
    __rt::expect_unit("KillPlayer", __rt::host_call(&HostCall::KillPlayer));
}

/// Overwrite the player's health (clamped to `0..=20` half-hearts), bypassing
/// the damage funnel — the heal/set primitive, no events fire.
pub fn set_health(value: i32) {
    __rt::expect_unit("SetHealth", __rt::host_call(&HostCall::SetHealth { value }));
}

/// Move the player's feet to `pos`; fall tracking is cleared so a teleport can
/// never land as fall damage.
pub fn teleport(pos: [f32; 3]) {
    __rt::expect_unit("Teleport", __rt::host_call(&HostCall::Teleport { pos }));
}

// --- Phase 3b: sound ----------------------------------------------------------

/// Play a sound by `sounds.json` key. `pos` attenuates by the sound row's
/// `attenuation_distance`; `None` plays at full volume. `false` = unknown key.
pub fn emit_sound(key: &str, pos: Option<[f32; 3]>) -> bool {
    match __rt::host_call(&HostCall::EmitSound {
        key: key.into(),
        pos,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("EmitSound returned {other:?}"),
    }
}

/// Start a spatial sound at a fixed world position. Returns a deterministic
/// session handle, or `0` if the key/parameters were rejected. Travel distance
/// comes from the sound row's `attenuation_distance`.
pub fn sound_play_at(key: &str, pos: [f32; 3], volume: f32, pitch: f32) -> u64 {
    match __rt::host_call(&HostCall::SoundPlayAt {
        key: key.into(),
        pos,
        volume,
        pitch,
    }) {
        HostRet::U64(handle) => handle,
        other => panic!("SoundPlayAt returned {other:?}"),
    }
}

/// Start a spatial sound pinned to a stable mob id from [`MobSnapshot::id`].
/// If that mob despawns, the engine lets the sound finish at its last known
/// position. Returns `0` if the key/mob/parameters were rejected. Travel
/// distance comes from the sound row's `attenuation_distance`.
pub fn sound_play_on_mob(mob_id: u64, key: &str, volume: f32, pitch: f32) -> u64 {
    match __rt::host_call(&HostCall::SoundPlayOnMob {
        mob_id,
        key: key.into(),
        volume,
        pitch,
    }) {
        HostRet::U64(handle) => handle,
        other => panic!("SoundPlayOnMob returned {other:?}"),
    }
}

/// Stop a spatial sound handle. Unknown handles are ignored.
pub fn sound_stop(handle: u64) {
    __rt::expect_unit(
        "SoundStop",
        __rt::host_call(&HostCall::SoundStop { handle }),
    );
}

// --- Phase 3b: persistent KV ----------------------------------------------------
//
// Keys are namespaced. Writes must use this mod's own prefix or an exposed
// engine `llama:*` key (enforced by the host; a violation panics = disables the
// mod); reads may cross namespaces — the cross-mod interop surface. Key ≤ 256
// bytes, value ≤ 64 KiB.

/// Read a world KV entry (persists in the save's `level.dat`).
pub fn world_kv_get(key: &str) -> Option<Vec<u8>> {
    match __rt::host_call(&HostCall::WorldKvGet { key: key.into() }) {
        HostRet::Bytes(v) => v,
        other => panic!("WorldKvGet returned {other:?}"),
    }
}

/// Write a world KV entry (own namespace or exposed `llama:*` key required).
pub fn world_kv_set(key: &str, value: Vec<u8>) {
    __rt::expect_unit(
        "WorldKvSet",
        __rt::host_call(&HostCall::WorldKvSet {
            key: key.into(),
            value,
        }),
    );
}

/// Delete a world KV entry (own namespace or exposed `llama:*` key required);
/// `false` = absent.
pub fn world_kv_delete(key: &str) -> bool {
    match __rt::host_call(&HostCall::WorldKvDelete { key: key.into() }) {
        HostRet::Bool(present) => present,
        other => panic!("WorldKvDelete returned {other:?}"),
    }
}

/// Read a per-cell KV entry (`pos` = world block position). `None` when the
/// key is absent or the owning section is unloaded.
pub fn section_kv_get(pos: [i32; 3], key: &str) -> Option<Vec<u8>> {
    match __rt::host_call(&HostCall::SectionKvGet {
        pos,
        key: key.into(),
    }) {
        HostRet::Bytes(v) => v,
        other => panic!("SectionKvGet returned {other:?}"),
    }
}

/// Write a per-cell KV entry (own-namespace key required). `false` = the
/// owning section is unloaded (nothing stored).
pub fn section_kv_set(pos: [i32; 3], key: &str, value: Vec<u8>) -> bool {
    match __rt::host_call(&HostCall::SectionKvSet {
        pos,
        key: key.into(),
        value,
    }) {
        HostRet::Bool(stored) => stored,
        other => panic!("SectionKvSet returned {other:?}"),
    }
}

/// Delete a per-cell KV entry (own-namespace key required); `false` = absent.
pub fn section_kv_delete(pos: [i32; 3], key: &str) -> bool {
    match __rt::host_call(&HostCall::SectionKvDelete {
        pos,
        key: key.into(),
    }) {
        HostRet::Bool(present) => present,
        other => panic!("SectionKvDelete returned {other:?}"),
    }
}

/// Read a per-mob KV entry (`mob_index` valid this tick only). `None` when
/// the key is absent or there is no such mob.
pub fn mob_kv_get(mob_index: u32, key: &str) -> Option<Vec<u8>> {
    match __rt::host_call(&HostCall::MobKvGet {
        mob_index,
        key: key.into(),
    }) {
        HostRet::Bytes(v) => v,
        other => panic!("MobKvGet returned {other:?}"),
    }
}

/// Write a per-mob KV entry (own-namespace key required); persists with the
/// mob's save record. `false` = no such mob.
pub fn mob_kv_set(mob_index: u32, key: &str, value: Vec<u8>) -> bool {
    match __rt::host_call(&HostCall::MobKvSet {
        mob_index,
        key: key.into(),
        value,
    }) {
        HostRet::Bool(stored) => stored,
        other => panic!("MobKvSet returned {other:?}"),
    }
}

/// Delete a per-mob KV entry (own-namespace key required); `false` = absent.
pub fn mob_kv_delete(mob_index: u32, key: &str) -> bool {
    match __rt::host_call(&HostCall::MobKvDelete {
        mob_index,
        key: key.into(),
    }) {
        HostRet::Bool(present) => present,
        other => panic!("MobKvDelete returned {other:?}"),
    }
}

// --- Phase 4: worldgen hooks ----------------------------------------------------

/// Resolve a block registry key (`"llama:stone"`, `"mymod:gadget"`) to its
/// session-scoped runtime id. Works everywhere, worldgen instances included —
/// resolve once in [`Mod::init`] and keep the id in mod state (but NEVER
/// persist it: ids can change between sessions; names are the stable identity).
pub fn resolve_block(key: &str) -> Option<BlockId> {
    match __rt::host_call(&HostCall::ResolveBlock { key: key.into() }) {
        HostRet::Block(b) => b,
        other => panic!("ResolveBlock returned {other:?}"),
    }
}

/// Register a worldgen feature that runs after `stage` (use
/// [`WorldgenStage::Trees`] — the end of the pipeline — unless the feature
/// must see pre-vegetation ground). Only legal during [`Mod::init`];
/// `Climate` is not a valid attach point. `feature_id` is echoed to
/// [`Mod::gen_feature`].
pub fn register_worldgen_feature(stage: WorldgenStage, feature_id: u32) {
    __rt::expect_unit(
        "RegisterWorldgenFeature",
        __rt::host_call(&HostCall::RegisterWorldgenFeature { feature_id, stage }),
    );
}

/// Replace one engine worldgen stage. Only legal during [`Mod::init`];
/// `callback_id` is echoed to [`Mod::gen_climate`] / [`Mod::gen_terrain`] /
/// [`Mod::gen_stage`] depending on the stage. Last mod in load order wins a
/// conflict; a failing replacement falls back to the engine stage.
pub fn register_stage_replacement(stage: WorldgenStage, callback_id: u32) {
    __rt::expect_unit(
        "RegisterStageReplacement",
        __rt::host_call(&HostCall::RegisterStageReplacement { stage, callback_id }),
    );
}

/// Replace the whole generator: every stage dispatches to `callback_id` (your
/// `gen_climate`/`gen_terrain`/`gen_stage` switch on the stage). Same rules as
/// [`register_stage_replacement`], applied per stage.
pub fn register_generator(callback_id: u32) {
    __rt::expect_unit(
        "RegisterGenerator",
        __rt::host_call(&HostCall::RegisterGenerator { callback_id }),
    );
}

// --- Phase 5: mod GUIs ------------------------------------------------------

/// Write a key of the open GUI session's state map (labels bound to the key
/// redraw; `rotimage` reads its angle in radians from an `F32`). Keys are
/// mod-local — the map belongs to one GUI session and clears on open/close.
pub fn gui_state_set(key: &str, value: GuiValue) {
    __rt::expect_unit(
        "GuiStateSet",
        __rt::host_call(&HostCall::GuiStateSet {
            key: key.into(),
            value,
        }),
    );
}

/// Read a key of the GUI state map (`None` = absent).
pub fn gui_state_get(key: &str) -> Option<GuiValue> {
    match __rt::host_call(&HostCall::GuiStateGet { key: key.into() }) {
        HostRet::GuiValue(v) => v,
        other => panic!("GuiStateGet returned {other:?}"),
    }
}

/// Ask the app shell to open the mod GUI registered under `kind_key` (a baked
/// manifest or an `open_gui` block row must have registered it). The screen
/// opens after this tick, only from gameplay. `false` = unknown/non-mod kind.
pub fn gui_open(kind_key: &str) -> bool {
    match __rt::host_call(&HostCall::GuiOpen {
        kind_key: kind_key.into(),
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("GuiOpen returned {other:?}"),
    }
}

/// Close the open mod GUI (a no-op if none is open).
pub fn gui_close() {
    __rt::expect_unit("GuiClose", __rt::host_call(&HostCall::GuiClose));
}

/// One worldgen dispatch's inputs, with the accessors a well-behaved feature
/// needs. See the seam/determinism contract below — the engine cannot check it
/// for you; a violation shows up as features cut off at section borders.
///
/// # The worldgen determinism & seam contract
///
/// Sections generate independently, in any order, on any thread. The engine
/// dispatches your feature once per section and CLIPS the returned writes to
/// that section. A feature whose blocks span sections therefore only comes out
/// seamless if every section's call re-derives the SAME decisions for a shared
/// origin. That holds automatically when a per-origin decision uses only:
///
/// - positional RNG: [`GenRng::positional`] over `(ctx.seed(), your own salt,
///   origin coords)` — never a stateful stream, never state kept in `self`;
/// - the column data ([`GenCtx::surface_y`], [`GenCtx::biome`],
///   [`GenCtx::sea_level`]), which is IDENTICAL for every section of a column
///   — so a column-anchored feature may span any number of VERTICAL sections;
/// - per-cell occupancy predicates via [`GenCtx::block`] applied only to cells
///   inside the current section (out-of-section cells return `None`; emit
///   nothing for them — the owning section's call emits its own cells).
///
/// Column data covers only this section's own 16×16 footprint. An origin in a
/// HORIZONTAL margin (a neighbouring column) has no surface/biome data here,
/// so cross-column reach is safe only for decisions that are purely positional
/// (e.g. underground blobs at absolute Y, iterated via
/// [`GenCtx::for_each_origin`] with a margin equal to the feature's horizontal
/// reach). Surface-anchored features should keep margin 0 and write only in
/// the origin's own column.
pub struct GenCtx {
    section_pos: [i32; 3],
    seed: u32,
    blocks: Vec<u8>,
    surface_heights: Vec<i32>,
    biomes: Vec<u8>,
    sea_level: i32,
}

impl GenCtx {
    /// Section coordinates (16³ units).
    pub fn section_pos(&self) -> [i32; 3] {
        self.section_pos
    }

    /// The section's world origin (minimum corner).
    pub fn origin_world(&self) -> [i32; 3] {
        [
            self.section_pos[0] * 16,
            self.section_pos[1] * 16,
            self.section_pos[2] * 16,
        ]
    }

    /// The world seed — feed it to [`GenRng::positional`].
    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// Sea level (world Y of the waterline).
    pub fn sea_level(&self) -> i32 {
        self.sea_level
    }

    /// The column's post-cave bare-ground surface (world Y, before
    /// vegetation/trees) at world `(wx, wz)`, or `None` outside this section's
    /// 16×16 footprint. Below [`GenCtx::sea_level`] = submerged or floorless.
    /// Identical for every section of the column.
    pub fn surface_y(&self, wx: i32, wz: i32) -> Option<i32> {
        Some(self.surface_heights[self.column_index(wx, wz)?])
    }

    /// The biome id at world `(wx, wz)`, or `None` outside the footprint.
    /// Identical for every section of the column.
    pub fn biome(&self, wx: i32, wz: i32) -> Option<u8> {
        Some(self.biomes[self.column_index(wx, wz)?])
    }

    /// The engine's proposed biome map (`z*16 + x`) — only meaningful inside
    /// [`Mod::gen_climate`], where it is the map you are replacing.
    pub fn biomes(&self) -> &[u8] {
        &self.biomes
    }

    /// The block currently at world `p`, or `None` when `p` is outside this
    /// section (or the call carries no snapshot: `Climate`/`Terrain` stages).
    /// Use it for per-cell occupancy predicates ("only place over air") on the
    /// cells you emit — each section checks exactly the cells it owns.
    pub fn block(&self, p: [i32; 3]) -> Option<BlockId> {
        if self.blocks.len() != 4096 {
            return None;
        }
        let o = self.origin_world();
        let (lx, ly, lz) = (p[0] - o[0], p[1] - o[1], p[2] - o[2]);
        if !(0..16).contains(&lx) || !(0..16).contains(&ly) || !(0..16).contains(&lz) {
            return None;
        }
        Some(BlockId(
            self.blocks[(ly as usize) * 256 + (lz as usize) * 16 + lx as usize],
        ))
    }

    /// Iterate candidate feature origins over this section's XZ footprint plus
    /// `margin` extra columns on every side, in the engine's canonical
    /// `(wz, wx)` order — the same loop the engine's own features use. Use
    /// margin 0 for column-anchored features; a positive margin only for
    /// purely positional ones (see the contract on [`GenCtx`]).
    pub fn for_each_origin(&self, margin: i32, mut f: impl FnMut(i32, i32)) {
        let o = self.origin_world();
        for wz in (o[2] - margin)..(o[2] + 16 + margin) {
            for wx in (o[0] - margin)..(o[0] + 16 + margin) {
                f(wx, wz);
            }
        }
    }

    /// `z*16 + x` index for a world column inside the footprint.
    fn column_index(&self, wx: i32, wz: i32) -> Option<usize> {
        let o = self.origin_world();
        let (lx, lz) = (wx - o[0], wz - o[2]);
        if (0..16).contains(&lx) && (0..16).contains(&lz) && self.surface_heights.len() == 256 {
            Some((lz as usize) * 16 + lx as usize)
        } else {
            None
        }
    }
}

/// Deterministic positional RNG for worldgen hooks — the guest-side mirror of
/// the engine's frozen positional seeding contract (same SplitMix64 finalizer,
/// same xorshift64 stepper), so mod features get engine-grade order
/// independence by default. Derive every independent stream from
/// `(world seed, your own salt, world coords)`; NEVER carry RNG state between
/// dispatches. Pick a salt unique to your mod/feature (any constant — hash
/// your feature name) so your stream is decorrelated from the engine's and
/// from other mods'.
pub struct GenRng {
    state: u64,
}

impl GenRng {
    /// Seed from `(seed, salt, world coords)` — a pure function of the inputs,
    /// bit-identical across platforms.
    pub fn positional(seed: u32, salt: u64, wx: i32, wy: i32, wz: i32) -> Self {
        let mut z = (seed as u64)
            ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (wx as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
            ^ (wy as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9)
            ^ (wz as i64 as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self {
            state: if z == 0 { 0xDEAD_BEEF } else { z },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform integer in `[lo, hi]` (inclusive).
    pub fn next_i32(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_u64() % (hi - lo + 1).max(1) as u64) as i32
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.next_f32() < p
    }
}

#[cfg(test)]
mod tests {
    use super::GenRng;

    /// [`GenRng`] mirrors the ENGINE's frozen positional seeding contract
    /// (`src/worldgen/rng.rs` pins the same vectors) — if this drifts, mod
    /// features lose engine-grade determinism. Never "fix" these numbers;
    /// fix the generator.
    #[test]
    fn positional_stream_matches_the_engine_contract() {
        let mut rng = GenRng::positional(0x1234_5678, 0x0000_7a3e_0ac0_ffee, 12, 0, -34);
        assert_eq!(
            [
                rng.next_u64(),
                rng.next_u64(),
                rng.next_u64(),
                rng.next_u64()
            ],
            [
                0x6ac6_a985_c496_4f45,
                0x44e3_bbfd_0652_129b,
                0x75f9_7613_ca75_707e,
                0xa90a_c427_548e_451e,
            ],
        );
        let mut zero = GenRng::positional(0, 0, 0, 0, 0);
        assert_eq!(zero.next_u64(), 0x37c5_9ca7_bf06_be52);
    }
}

/// Define the raw wasm exports for a [`Mod`] implementation. Exactly one call
/// per mod crate:
///
/// ```ignore
/// #[derive(Default)]
/// struct MyMod;
/// impl mod_sdk::Mod for MyMod { /* ... */ }
/// mod_sdk::register_mod!(MyMod);
/// ```
#[macro_export]
macro_rules! register_mod {
    ($ty:ty) => {
        static __LLAMACRAFT_MOD: $crate::__rt::ModSlot<$ty> = $crate::__rt::ModSlot::new();

        #[no_mangle]
        pub extern "C" fn mod_init() {
            $crate::__rt::init(&__LLAMACRAFT_MOD)
        }

        #[no_mangle]
        pub extern "C" fn mod_alloc(len: u32) -> u32 {
            $crate::__rt::alloc(len)
        }

        #[no_mangle]
        pub extern "C" fn mod_free(ptr: u32, len: u32) {
            $crate::__rt::free(ptr, len)
        }

        #[no_mangle]
        pub extern "C" fn mod_dispatch(ptr: u32, len: u32) -> u64 {
            $crate::__rt::dispatch(&__LLAMACRAFT_MOD, ptr, len)
        }
    };
}

/// ABI plumbing for [`register_mod!`]. Not mod-facing API — everything here is
/// `#[doc(hidden)]` and may change with the SDK.
#[doc(hidden)]
pub mod __rt {
    use core::cell::UnsafeCell;

    use mod_api::{GuestCall, GuestRet, HostCall, HostRet};

    #[cfg(target_arch = "wasm32")]
    #[link(wasm_import_module = "env")]
    extern "C" {
        fn host_dispatch(ptr: u32, len: u32) -> u64;
    }

    /// Host-target stub so the SDK itself type-checks off-wasm (mods only ever
    /// build for wasm32; the engine never links this crate).
    #[cfg(not(target_arch = "wasm32"))]
    unsafe fn host_dispatch(_ptr: u32, _len: u32) -> u64 {
        unreachable!("mod-sdk host calls only exist inside the wasm guest")
    }

    /// The single mod instance behind the raw exports.
    ///
    /// SAFETY: the wasm guest is single-threaded by construction (the host
    /// disables wasm threads), so unsynchronized interior mutability is sound
    /// there; the `Sync` impl exists only to allow the `static`.
    pub struct ModSlot<T>(UnsafeCell<Option<T>>);

    unsafe impl<T> Sync for ModSlot<T> {}

    impl<T> ModSlot<T> {
        #[allow(clippy::new_without_default)]
        pub const fn new() -> Self {
            Self(UnsafeCell::new(None))
        }
    }

    /// Guest-side buffers cross the ABI as raw byte allocations with align 1;
    /// `alloc`/`free` are the exported allocator the HOST also uses to hand
    /// buffers in (requests) and reclaim buffers it read (replies).
    pub fn alloc(len: u32) -> u32 {
        if len == 0 {
            return 4; // non-null, never dereferenced nor freed (len 0)
        }
        let layout = core::alloc::Layout::from_size_align(len as usize, 1).unwrap();
        let ptr = unsafe { std::alloc::alloc(layout) };
        assert!(!ptr.is_null(), "guest allocation of {len} bytes failed");
        ptr as u32
    }

    pub fn free(ptr: u32, len: u32) {
        if len == 0 {
            return;
        }
        let layout = core::alloc::Layout::from_size_align(len as usize, 1).unwrap();
        unsafe { std::alloc::dealloc(ptr as *mut u8, layout) };
    }

    /// Encode a byte buffer into a fresh allocation and pack its address for
    /// the `u64` return lane. The receiver frees it via [`free`].
    fn to_wire(bytes: &[u8]) -> u64 {
        let ptr = alloc(bytes.len() as u32);
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len()) };
        mod_api::pack_ptr_len(ptr, bytes.len() as u32)
    }

    /// One host call: encode, dispatch, decode the reply (the host allocated
    /// it in our memory through `mod_alloc`; we own and free it).
    pub fn host_call(call: &HostCall) -> HostRet {
        let request = mod_api::encode(call).expect("encode host call");
        let packed = unsafe { host_dispatch(request.as_ptr() as u32, request.len() as u32) };
        let (ptr, len) = mod_api::unpack_ptr_len(packed);
        let reply = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        let ret = mod_api::decode(reply).expect("malformed host reply");
        free(ptr, len);
        ret
    }

    /// Registration replies must be `Unit`; an [`HostRet::Error`] (e.g.
    /// registering outside `mod_init`) is a mod bug — panic loudly, which
    /// traps and disables the mod.
    pub fn expect_unit(what: &str, ret: HostRet) {
        match ret {
            HostRet::Unit => {}
            HostRet::Error(e) => panic!("{what} rejected: {e}"),
            other => panic!("{what} returned {other:?}"),
        }
    }

    pub fn init<T: crate::Mod>(slot: &ModSlot<T>) {
        // Panics abort the guest (a trap); surface the message through the
        // host log first so the disable line has a cause next to it.
        std::panic::set_hook(Box::new(|info| {
            let _ = host_call(&HostCall::Log {
                msg: format!("PANIC: {info}"),
            });
        }));
        let slot = unsafe { &mut *slot.0.get() };
        slot.insert(T::default()).init();
    }

    pub fn dispatch<T: crate::Mod>(slot: &ModSlot<T>, ptr: u32, len: u32) -> u64 {
        let call: GuestCall = {
            let request = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
            let call = mod_api::decode(request).expect("malformed engine call");
            free(ptr, len); // the guest owns request buffers once dispatched
            call
        };
        let mod_ = unsafe { (*slot.0.get()).as_mut() }.expect("mod_dispatch before mod_init");
        let ret = match call {
            GuestCall::TickSystem { id } => {
                mod_.tick_system(id);
                GuestRet::Unit
            }
            GuestCall::HandleEvent {
                id,
                kind: _,
                mut payload,
            } => {
                let outcome = mod_.handle_event(id, &mut payload);
                GuestRet::Event { outcome, payload }
            }
            GuestCall::GenFeature {
                feature_id,
                section_pos,
                seed,
                blocks,
                surface_heights,
                biomes,
                sea_level,
            } => {
                let ctx = crate::GenCtx {
                    section_pos,
                    seed,
                    blocks,
                    surface_heights,
                    biomes,
                    sea_level,
                };
                GuestRet::GenWrites(mod_.gen_feature(feature_id, &ctx))
            }
            GuestCall::GenStage {
                callback_id,
                stage,
                section_pos,
                seed,
                blocks,
                surface_heights,
                biomes,
                sea_level,
            } => {
                let ctx = crate::GenCtx {
                    section_pos,
                    seed,
                    blocks,
                    surface_heights,
                    biomes,
                    sea_level,
                };
                match stage {
                    mod_api::WorldgenStage::Climate => {
                        GuestRet::GenBiomes(mod_.gen_climate(callback_id, &ctx))
                    }
                    mod_api::WorldgenStage::Terrain => {
                        GuestRet::GenBlocks(mod_.gen_terrain(callback_id, &ctx))
                    }
                    other => GuestRet::GenWrites(mod_.gen_stage(callback_id, other, &ctx)),
                }
            }
            GuestCall::GuiClick {
                kind_key,
                widget_id,
                pos,
            } => {
                mod_.gui_click(&kind_key, &widget_id, pos);
                GuestRet::Unit
            }
            GuestCall::HostileSpawnCandidate {
                callback_id,
                candidate,
            } => GuestRet::HostileSpawn(
                mod_.hostile_spawn_candidate(callback_id, &candidate),
            ),
        };
        to_wire(&mod_api::encode(&ret).expect("encode guest reply"))
    }
}
