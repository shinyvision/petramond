//! Presentation-only client instance calls: overlays, registered keys,
//! replica surface sampling, document state, images and text, GUI/canvas
//! lifecycle, world marks, sandboxed client storage, and where the instance runs.

use mod_api::{
    BlockId, ClientCanvasElement, ClientContext, ClientEngineFactsData, ClientKeyContexts,
    ClientKeyMods, ClientOverlayAnchor, ClientPackInfo, ClientStorageScope, ClientSurfaceColumn,
    ClientSurfaceQuery, ClientTextRun, ClientWallTime, ClientWorldMark, GuiValue, HostRet,
};

// Imported for intra-doc links only.
#[allow(unused_imports)]
use crate::Mod;

use crate::__rt::{host_fn, recoverable, try_host_fn};

host_fn! {
    /// Register an always-on physical-pixel overlay image during [`Mod::init`].
    /// `hud: true` = part of the HUD (a hidden HUD hides it); `false` = a
    /// tool's own status, shown whatever the HUD claim. Never recorded.
    pub fn client_register_overlay(
        image_key: &str,
        anchor: ClientOverlayAnchor,
        margin: [u16; 2],
        display_size: [u16; 2],
        hud: bool
    ) => ClientRegisterOverlay {
        image_key: image_key.into(),
        anchor,
        margin,
        display_size,
        hud,
    }
}

host_fn! {
    /// Register a REMAPPABLE client key action during init: a stable bare `id`
    /// (the player's remap persists as `mod_id:id`), a display `label` for the
    /// Options → Controls screen, the DEFAULT physical `key` (any key, in
    /// snake_case: `"key_m"`, `"f9"`, `"arrow_left"`, `"space"`) with the chord
    /// `mods` it needs, where it fires (`contexts`: gameplay and/or your own
    /// document kinds and canvas keys), and the `action_id` your `client_key`
    /// handler matches on.
    pub fn client_register_key(
        id: &str,
        label: &str,
        key: &str,
        mods: ClientKeyMods,
        contexts: ClientKeyContexts,
        action_id: u32
    ) => ClientRegisterKey {
        id: id.into(),
        label: label.into(),
        key: key.into(),
        mods,
        contexts,
        action_id,
    }
}

host_fn! {
    /// What each of your key actions (by bare id) is bound to right now, as
    /// Options → Controls prints it — for tooltips that name the real key.
    /// Parallel to `ids`; `None` = no such action of yours.
    pub fn client_key_labels(ids: Vec<String>) -> Vec<Option<String>>
        => ClientKeyLabels { ids } => Names
}

host_fn! {
    /// Read whole surface chunk columns from the client replica, revision gated:
    /// the reply is parallel to `queries`, `None` = column unknown, and a reply
    /// without cell bytes = unchanged since the queried revision. Only echo a
    /// revision back once a reply for it had every cell known (see
    /// [`mod_api::ClientSurfaceColumn`]).
    pub fn client_surface_columns(queries: Vec<ClientSurfaceQuery>) -> Vec<Option<ClientSurfaceColumn>>
        => ClientSurfaceColumns { queries } => ClientSurfaceColumns
}

host_fn! {
    /// Read replica block ids at world positions, reply parallel to
    /// `positions` (at most 512 per call). `None` = cell unknown to the
    /// replica (unloaded, or streamed content not yet final) — treat it like
    /// an unloaded server-side read: state frozen, retry later. Resolve ids
    /// to compare against with [`crate::resolve_block`].
    pub fn client_blocks_at(positions: Vec<[i32; 3]>) -> Vec<Option<BlockId>>
        => ClientBlocksAt { positions } => Blocks
}

host_fn! {
    /// Read one per-cell KV `key` at each of `cells` from the REPLICA, reply
    /// parallel to `cells` (at most 512 per call) — the cell-KV twin of
    /// [`client_blocks_at`], the read side of the cell-KV replication lane
    /// (a server-side [`crate::section_kv_set`] on a loaded section streams
    /// to every client and lands here). `None` = key absent or cell unknown.
    /// Reads may cross namespaces like every KV read.
    pub fn client_cell_kv_at(key: &str, cells: Vec<[i32; 3]>) -> Vec<Option<Vec<u8>>>
        => ClientCellKvAt { key: key.into(), cells } => BytesMany
}

host_fn! {
    /// Overwrite one rectangle of an existing published client image in place:
    /// `origin`/`size` in image pixels, `rgba` = exactly `size` pixels of RGBA8.
    pub fn client_image_blit(key: &str, origin: [u16; 2], size: [u16; 2], rgba: Vec<u8>)
        => ClientImageBlit { key: key.into(), origin, size, rgba }
}

host_fn! {
    pub fn client_ui_state_set(key: &str, value: GuiValue)
        => ClientUiStateSet { key: key.into(), value }
}

host_fn! {
    pub fn client_ui_state_get(key: &str) -> Option<GuiValue>
        => ClientUiStateGet { key: key.into() } => GuiValue
}

host_fn! {
    /// Publish one host-fed RGBA8 document/overlay/canvas image.
    pub fn client_image_set(key: &str, width: u16, height: u16, rgba: Vec<u8>)
        => ClientImageSet { key: key.into(), width, height, rgba }
}

host_fn! {
    /// Measure a single-line run using the host's shared text subsystem.
    pub fn client_text_measure(text: &str, scale: u8) -> [u16; 2]
        => ClientTextMeasure { text: text.into(), scale } => ClientTextSize
}

host_fn! {
    /// Draw ordered text runs into an already-published client image.
    pub fn client_image_draw_texts(key: &str, runs: Vec<ClientTextRun>)
        => ClientImageDrawTexts { key: key.into(), runs }
}

host_fn! {
    pub fn client_gui_open(kind_key: &str) -> bool
        => ClientGuiOpen { kind_key: kind_key.into() } => Bool
}

host_fn! {
    pub fn client_gui_close() => ClientGuiClose
}

host_fn! {
    /// Put the caret (at the end) in text input `id` (`item` for one stamped
    /// in a list) of your OPEN document. `false` = no such enabled input.
    pub fn client_ui_focus(id: &str, item: Option<u32>) -> bool
        => ClientUiFocus { id: id.into(), item } => Bool
}

host_fn! {
    /// Open the engine's pause menu over your open document or canvas; its
    /// Resume comes back to it. `false` where there is no pause menu (the
    /// shell with no world) or your UI is not on screen.
    pub fn client_pause_open() -> bool => ClientPauseOpen => Bool
}

host_fn! {
    pub fn client_canvas_open(canvas_key: &str, size: [u16; 2]) -> bool
        => ClientCanvasOpen { canvas_key: canvas_key.into(), size } => Bool
}

host_fn! {
    pub fn client_canvas_close() => ClientCanvasClose
}

host_fn! {
    pub fn client_canvas_scene_set(canvas_key: &str, elements: Vec<ClientCanvasElement>)
        => ClientCanvasSceneSet { canvas_key: canvas_key.into(), elements }
}

host_fn! {
    pub fn client_canvas_view_set(canvas_key: &str, offset: [f32; 2])
        => ClientCanvasViewSet { canvas_key: canvas_key.into(), offset }
}

host_fn! {
    /// Read exact keys from the `scope` bucket: [`ClientStorageScope::World`]
    /// is the presented session's (none on the shell — an error there),
    /// [`ClientStorageScope::Pack`] is this mod's own, in every context.
    pub fn client_storage_get_many(scope: ClientStorageScope, keys: Vec<String>) -> Vec<Option<Vec<u8>>>
        => ClientStorageGetMany { scope, keys }
        => HostRet::ClientStorageValues(values) => values
            .into_iter()
            .map(|value| value.map(mod_api::ByteBuf::into_vec))
            .collect()
}

try_host_fn! {
    /// Write entries into the `scope` bucket: `Some(bytes)` stores, `None`
    /// deletes. `Ok(ticket)` = queued ([`client_storage_write_poll`] says when
    /// it reached the disk); `Err(why)` = refused, nothing queued — the World
    /// bucket while a presentation presents. The Pack bucket keeps what it is
    /// given everywhere.
    pub fn client_storage_set_many(
        scope: ClientStorageScope,
        entries: Vec<(String, Option<Vec<u8>>)>
    ) -> u64
        => ClientStorageSetMany {
            scope,
            entries: entries
                .into_iter()
                .map(|(key, value)| (key, value.map(mod_api::ByteBuf::from)))
                .collect(),
        } => ClientStorageWrite
}

/// Where a write queued in `scope` stands: `None` = still queued,
/// `Some(Ok(()))` = on disk, `Some(Err(why))` = the disk refused it
/// ([`mod_api::ErrorCode::Refused`]).
pub fn client_storage_write_poll(
    scope: ClientStorageScope,
    ticket: u64,
) -> Option<Result<(), mod_api::HostError>> {
    let ret = crate::__rt::host_call(&mod_api::HostCall::from(
        mod_api::calls::ClientStorageWritePoll { scope, ticket },
    ));
    match recoverable("ClientStorageWritePoll", ret) {
        Ok(mod_api::HostRet::ClientStorageWritten(landed)) => landed.then_some(Ok(())),
        Err(refused) => Some(Err(refused)),
        Ok(other) => panic!("ClientStorageWritePoll returned {other:?}"),
    }
}

host_fn! {
    /// Begin an asynchronous storage read on the host's background worker; the
    /// returned ticket resolves through [`client_storage_read_poll`] in the same
    /// `scope`, usually on a later frame. The REQUIRED path for bulk spatial
    /// reads — a slow disk delays the data, never the frame. Ordered after
    /// already-issued writes.
    pub fn client_storage_read_begin(scope: ClientStorageScope, keys: Vec<String>) -> u64
        => ClientStorageReadBegin { scope, keys } => U64
}

host_fn! {
    /// Poll an asynchronous storage read begun in `scope`: `Some(values)`
    /// (parallel to the begun keys, `None` entry = absent) consumes the ticket,
    /// `None` = still in flight. Polling an unknown or consumed ticket disables
    /// the mod.
    pub fn client_storage_read_poll(scope: ClientStorageScope, ticket: u64) -> Option<Vec<Option<Vec<u8>>>>
        => ClientStorageReadPoll { scope, ticket }
        => HostRet::ClientStorageRead(values) => values.map(|values| {
            values
                .into_iter()
                .map(|value| value.map(mod_api::ByteBuf::into_vec))
                .collect()
        })
}

host_fn! {
    /// CLIENT: read named shader params from the replica's replicated visual
    /// environment — the same values the renderer sees (a sim-side mod
    /// publishes them with [`crate::shader_set_param`]). At most 16 keys per
    /// call; the reply is parallel (`None` = param not present).
    pub fn client_env_params(keys: &[&str]) -> Vec<Option<[f32; 4]>>
        => ClientEnvParams { keys: keys.iter().map(|k| (*k).into()).collect() }
        => EnvParams
}

host_fn! {
    /// CLIENT: the replica column's biome id at world `pos = [x, z]`
    /// (vocabulary: [`mod_api::biome`]), or `None` when the column is unknown
    /// to the replica.
    pub fn client_biome_at(pos: [i32; 2]) -> Option<u8> => ClientBiomeAt { pos } => MaybeByte
}

host_fn! {
    /// CLIENT: drive an `ambient` particle bundle (a camera-following
    /// precipitation/ambience volume from `particle_emitters.json`) at
    /// `intensity` (clamped to `0..=1`; `0` retires it; changes are eased
    /// engine-side so weather never pops), advected by `wind` blocks/s.
    /// Per-client presentation only. `false` = unknown key or not an
    /// ambient bundle (forgiving, like a disabled pack).
    pub fn client_ambient_set(key: &str, intensity: f32, wind: [f32; 2]) -> bool
        => ClientAmbientSet { key: key.into(), intensity, wind } => Bool
}

host_fn! {
    /// CLIENT: play this mod's looping sound `key` (a `sounds.json` key) at
    /// `gain` (`0` eases it to silence and stops it). Non-spatial ambience —
    /// a rain bed, a wind howl. `false` = unknown sound key.
    pub fn client_loop_set(key: &str, gain: f32) -> bool
        => ClientLoopSet { key: key.into(), gain } => Bool
}

host_fn! {
    /// CLIENT: set this mod's post-process MOOD — a subtle whole-screen
    /// darken and desaturate (each clamped to `0..=0.5`), applied by the
    /// grade pass and eased engine-side. Pure presentation: light values
    /// (and so mob spawning) never change. Mods combine by max.
    pub fn client_mood_set(darken: f32, desaturate: f32) -> bool
        => ClientMoodSet { darken, desaturate } => Bool
}

host_fn! {
    /// CLIENT: replace this mod's retained world mark set `set` (a name of
    /// lowercase letters, digits and `_`) — lines and camera-facing points at
    /// world positions, drawn over the world on this window until that set is
    /// set again; an empty `marks` clears it. Sets are replaced independently.
    /// A set with an invalid mark is refused whole. Marks are never part of
    /// a frame capture.
    pub fn client_world_marks_set(set: &str, marks: Vec<ClientWorldMark>)
        => ClientWorldMarksSet { set: set.into(), marks }
}

host_fn! {
    /// Where this instance runs right now: beside a world, or on the shell
    /// with none (started from the pack's title-screen launch entry). It moves
    /// to `Presentation` while a presentation it opened from the shell
    /// presents, and back.
    pub fn client_context() -> ClientContext => ClientContext => ClientContext
}

host_fn! {
    /// CLIENT: what THIS build and machine are — the capture format and
    /// protocol it reads, its id vocabulary, seconds per tick, the rendering
    /// device's frame limits, and how far this instance's memory can grow.
    /// The constants a mod was compiled against may differ; this is the truth.
    pub fn client_engine_facts() -> ClientEngineFactsData
        => ClientEngineFacts => ClientEngineFacts
}

host_fn! {
    /// CLIENT: every installed id-bearing pack, as a presentation opened now
    /// could host it.
    pub fn client_packs() -> Vec<ClientPackInfo> => ClientPacks => ClientPacks
}

host_fn! {
    /// CLIENT: the machine's wall clock and its UTC offset. Read-only; a
    /// frame's real elapsed time is [`ClientFrameData::wall_dt`](mod_api::ClientFrameData::wall_dt).
    pub fn client_wall_clock() -> ClientWallTime => ClientWallClock => ClientWallClock
}
