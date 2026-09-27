//! The presentation-only client surface: overlays, keys, images, text, GUIs,
//! canvases, sandboxed storage, environment and ambience, the view claims,
//! world marks, the read-only replica queries, and the facts of the build and
//! machine. Client instances only; the calls that need no world also answer on
//! the shell.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::client::{
    ClientCanvasElement, ClientEntitiesNear, ClientKeyContexts, ClientKeyMods, ClientOverlayAnchor,
    ClientStorageScope, ClientSurfaceQuery, ClientTextRun, ClientWorldMark,
};
use crate::data::{EntityRef, GuiValue};
use crate::ids::PlayerId;
use crate::legality::prelude::*;

host_domain! {
    /// The presentation-only client surface: overlays, keys, images, text, GUIs,
    /// canvases, sandboxed storage, environment and ambience, the view claims,
    /// world marks, the read-only replica queries, and the facts of the build
    /// and machine. Client instances only.
    ClientCall {
        /// Register an always-on physical-pixel overlay image. Legal only from a
        /// client instance during `mod_init`; the image may be published later.
        /// `margin` and `display_size` are physical screen pixels. `hud: true`
        /// makes it part of the HUD (a hidden-HUD view claim hides it with the
        /// rest); `false` draws it whatever the HUD claim — a tool's own status,
        /// not the player's HUD. Either way it is the window's, never part of a
        /// captured frame. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientRegisterOverlay {
            image_key: String,
            anchor: ClientOverlayAnchor,
            margin: [u16; 2],
            display_size: [u16; 2],
            hud: bool,
        } => legal(CLIENT_SHELL, Init, Write),
        /// Register one REMAPPABLE key action: a stable bare `id` (the player's
        /// remap persists under `mod_id:id`), a display `label` for the Options →
        /// Controls screen (listed under the pack's name), the DEFAULT physical
        /// key — any key, named as the snake_case of its position (`"key_m"`,
        /// `"digit_1"`, `"f9"`, `"arrow_left"`, `"bracket_left"`, `"space"`,
        /// `"numpad_0"`) — with the modifier chord `mods` it needs held, where
        /// the action fires ([`ClientKeyContexts`]), and the opaque `action_id`
        /// delivered back in ClientKey events.
        ///
        /// A default that could fire in gameplay is rejected by the app when it
        /// equals an engine default binding (key AND chord) or a bare fixed
        /// control; a default that fires only over the mod's own screens is
        /// rejected only on Escape, which always belongs to the engine. Legal
        /// only during client `mod_init`. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientRegisterKey {
            id: String,
            label: String,
            key: String,
            mods: ClientKeyMods,
            contexts: ClientKeyContexts,
            action_id: u32,
        } => legal(CLIENT_SHELL, Init, Write),
        /// Read whole surface chunk columns from the client replica, revision
        /// gated: a column whose host revision still equals the query's `revision`
        /// replies without cell bytes, so a steady-state resample costs near
        /// nothing. The reply is parallel to `queries`; `None` = column unknown to
        /// the replica. Query count is host-capped. →
        /// [`HostRet::ClientSurfaceColumns`](crate::HostRet::ClientSurfaceColumns).
        ClientSurfaceColumns {
            queries: Vec<ClientSurfaceQuery>,
        } => legal(CLIENT, Any, Read),
        /// Write/read the client module's document-binding state. Keys must use
        /// the caller's namespace. → [`HostRet::Unit`](crate::HostRet::Unit) / [`HostRet::GuiValue`](crate::HostRet::GuiValue).
        ClientUiStateSet {
            key: String,
            value: GuiValue,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientUiStateGet {
            key: String,
        } => legal(CLIENT_SHELL, Any, Read),
        /// Publish an RGBA8 image for document nodes, physical overlays, or modal
        /// canvases. The key is namespaced and the host caps dimensions/bytes.
        /// Re-publishing the same key replaces it atomically for the next frame.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientImageSet {
            key: String,
            width: u16,
            height: u16,
            #[serde(with = "serde_bytes")]
            rgba: Vec<u8>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// Measure a single-line run with the host's shared text subsystem. The
        /// returned size uses physical pixels after applying `scale`. →
        /// [`HostRet::ClientTextSize`](crate::HostRet::ClientTextSize).
        ClientTextMeasure {
            text: String,
            scale: u8,
        } => legal(CLIENT_SHELL, Any, Read),
        /// Draw ordered text runs into an existing namespaced client image. This
        /// is a generic image/text capability: canvases, overlays, and GUI-fed
        /// images all use the same host glyphs and metrics. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientImageDrawTexts {
            key: String,
            runs: Vec<ClientTextRun>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// Request a client-owned GUI document open/close. These screens release
        /// the cursor but keep the replicated world running. → [`HostRet::Bool`](crate::HostRet::Bool)
        /// / [`HostRet::Unit`](crate::HostRet::Unit).
        ClientGuiOpen {
            kind_key: String,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientGuiClose => legal(CLIENT_SHELL, Any, Write),
        /// Open/close a modal, centered physical-pixel canvas. While open,
        /// the cursor is released, gameplay input is gated, and pointer events are
        /// dispatched through [`GuestCall::ClientCanvas`](crate::GuestCall::ClientCanvas). → [`HostRet::Bool`](crate::HostRet::Bool) /
        /// [`HostRet::Unit`](crate::HostRet::Unit).
        ClientCanvasOpen {
            canvas_key: String,
            size: [u16; 2],
        } => legal(CLIENT_SHELL, Any, Write),
        ClientCanvasClose => legal(CLIENT_SHELL, Any, Write),
        /// Replace one canvas's retained, ordered scene. Image keys and the canvas
        /// key must belong to the caller. Ordinary panning must use
        /// [`ClientCall::ClientCanvasViewSet`] instead. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientCanvasSceneSet {
            canvas_key: String,
            elements: Vec<ClientCanvasElement>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// Change only the retained scene's logical-pixel translation. This is the
        /// hot path for panning and never republishes image bytes or scene nodes.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientCanvasViewSet {
            canvas_key: String,
            offset: [f32; 2],
        } => legal(CLIENT_SHELL, Any, Write),
        /// Read a batch of exact sandboxed client-storage keys from the `scope`
        /// bucket. Results are parallel to `keys`; `None` means absent. An answer
        /// larger than the calling instance's
        /// [`guest_memory_max`](crate::ClientEngineFactsData::guest_memory_max)
        /// could never be delivered and is [`HostRet::Err`](crate::HostRet::Err):
        /// read fewer keys. Every bucket is the calling mod's alone. This
        /// exact-key shape lets large spatial stores page only their working set.
        /// → [`HostRet::ClientStorageValues`](crate::HostRet::ClientStorageValues);
        /// [`HostRet::Err`](crate::HostRet::Err) for the
        /// [`ClientStorageScope::World`] bucket on the shell.
        ClientStorageGetMany {
            scope: ClientStorageScope,
            keys: Vec<String>,
        } => legal(CLIENT_SHELL, Any, Read),
        /// Write a batch of sandboxed client-storage entries, committing each
        /// entry atomically: `Some(bytes)` stores them (empty bytes are a stored
        /// value like any other), `None` DELETES the key — reads then answer
        /// `None`. This is the hot-loop shape for explored map tiles; never cross
        /// once per tile. Keys must use the caller's namespace.
        ///
        /// → [`HostRet::ClientStorageWrite`](crate::HostRet::ClientStorageWrite):
        /// the ticket, queued in order; [`Self::ClientStorageWritePoll`] says
        /// when it reached the disk and whether the disk took it. REFUSED
        /// ([`ErrorCode::Refused`](crate::ErrorCode::Refused), recoverable),
        /// nothing queued: the scope is [`ClientStorageScope::World`] while a
        /// presentation presents (its world is not this session's to keep).
        /// The mod's own bugs (a key outside its namespace) answer a code that
        /// is not recoverable. Keys, values and batches are as large
        /// as the mod passes; a key whose hex file name the OS refuses fails its
        /// ticket in the OS's words. The [`ClientStorageScope::Pack`] bucket
        /// takes writes everywhere.
        ClientStorageSetMany {
            scope: ClientStorageScope,
            entries: Vec<(String, Option<serde_bytes::ByteBuf>)>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// Overwrite one rectangle of an existing namespaced client image in
        /// place (`origin`/`size` in image pixels, `rgba` = `size` pixels of
        /// RGBA8). The partial-update companion to [`ClientCall::ClientImageSet`]:
        /// spatial clients refresh an invalidated region without re-publishing
        /// the whole image. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientImageBlit {
            key: String,
            origin: [u16; 2],
            size: [u16; 2],
            #[serde(with = "serde_bytes")]
            rgba: Vec<u8>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// Begin an ASYNCHRONOUS read of a batch of exact sandboxed
        /// client-storage keys: the filesystem work runs on the background
        /// storage worker, so a slow disk delays the result instead of the
        /// frame. Ordered after already-queued writes (read-your-writes). Key
        /// rules match [`ClientCall::ClientStorageGetMany`], and so does the
        /// answer's size rule, applied when it is polled. This is the REQUIRED
        /// path for bulk spatial reads — the synchronous form is for small
        /// startup/edit reads. Tickets belong to the `scope` bucket.
        /// → [`HostRet::U64`](crate::HostRet::U64) (the ticket).
        ClientStorageReadBegin {
            scope: ClientStorageScope,
            keys: Vec<String>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// Poll an asynchronous read begun by
        /// [`ClientCall::ClientStorageReadBegin`] in the same `scope`.
        /// `Some(values)` (parallel to the begun keys, `None` entry = absent)
        /// consumes the ticket; `None` means still in flight — poll again next
        /// frame. Polling an unknown or already-consumed ticket is an error.
        /// → [`HostRet::ClientStorageRead`](crate::HostRet::ClientStorageRead).
        ClientStorageReadPoll {
            scope: ClientStorageScope,
            ticket: u64,
        } => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: read named shader params from the replica's replicated visual
        /// environment (the state sim mods publish with
        /// [`CoreCall::ShaderSetParam`](crate::CoreCall::ShaderSetParam)) — how a client instance sees the same
        /// values the renderer does. At most 16 keys per call; the reply is
        /// parallel (`None` = param not present). → [`HostRet::EnvParams`](crate::HostRet::EnvParams).
        ClientEnvParams {
            keys: Vec<String>,
        } => legal(CLIENT, Any, Read),
        /// CLIENT: the replica column's biome id at world `pos = [x, z]`
        /// (vocabulary: [`crate::biome`]). `None` = column unknown to the
        /// replica. → [`HostRet::MaybeByte`](crate::HostRet::MaybeByte).
        ClientBiomeAt {
            pos: [i32; 2],
        } => legal(CLIENT, Any, Read),
        /// CLIENT: drive an `ambient` particle bundle (a camera-following
        /// precipitation/ambience volume from `particle_emitters.json`) at
        /// `intensity` (clamped to `0..=1` — 1 is the bundle's full `max_count`
        /// density; `0` retires it; the engine eases changes so weather never
        /// pops) advected by `wind` (blocks/s). Per-client presentation only —
        /// never simulated, never replicated. `false` = unknown key or not an
        /// ambient bundle. → [`HostRet::Bool`](crate::HostRet::Bool).
        ClientAmbientSet {
            key: String,
            intensity: f32,
            wind: [f32; 2],
        } => legal(CLIENT, Any, Write),
        /// CLIENT: play this mod's looping sound `key` (a `sounds.json` key) at
        /// `gain` (`0` stops it; the engine eases changes so ambience never
        /// pops). Non-spatial, client-local. `false` = unknown sound key.
        /// → [`HostRet::Bool`](crate::HostRet::Bool).
        ClientLoopSet {
            key: String,
            gain: f32,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: set this mod's post-process MOOD — a subtle whole-screen
        /// `darken` and `desaturate` (each clamped to `0..=0.5`; deliberately
        /// incapable of blacking out the screen) applied by the grade pass and
        /// EASED engine-side, so weather/ambience moods breathe instead of
        /// popping. Pure presentation: no light value changes, so light-driven
        /// gameplay (mob spawning) is untouched. Multiple mods combine by MAX
        /// per component. Rides the grade pass, so it is invisible in the
        /// grade-off configuration. → [`HostRet::Bool`](crate::HostRet::Bool) (always `true`).
        ClientMoodSet {
            darken: f32,
            desaturate: f32,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: read replica block ids at world `positions`, reply parallel
        /// to the request. `None` = cell unknown to the replica (section
        /// unloaded, or its streamed content not yet final) — treat exactly like
        /// an unloaded server-side read: state frozen, retry later. Bounded
        /// batch (512 positions per call). → [`HostRet::Blocks`](crate::HostRet::Blocks).
        ClientBlocksAt {
            positions: Vec<[i32; 3]>,
        } => legal(CLIENT, Any, Read),
        /// CLIENT: read one per-cell KV `key` at each of `cells` from the REPLICA
        /// — the cell-KV twin of [`ClientCall::ClientBlocksAt`], and the read side
        /// of the cell-KV replication lane (a server mod's `SectionKvSet` on a
        /// loaded section streams to every client and lands here). Reply parallel
        /// to `cells`; `None` = key absent or cell unknown to the replica.
        /// Presentation-only state derivation (a render bake tinting from
        /// replicated fluid state); reads may cross namespaces like every KV
        /// read. Bounded batch (512 cells per call). → [`HostRet::BytesMany`](crate::HostRet::BytesMany).
        ClientCellKvAt {
            key: String,
            cells: Vec<[i32; 3]>,
        } => legal(CLIENT, Any, Read),
        /// CLIENT: claim where the view looks from — a world point, a look
        /// (`yaw`/`pitch`/`roll`, radians; positive `roll` banks the view to the
        /// right) and optionally a vertical field of view (`None` keeps the
        /// player's live one). RETAINED until
        /// [`ClientViewCameraRelease`](Self::ClientViewCameraRelease), like a held
        /// pose, and folded across mods in mod-id order.
        ///
        /// This is ONE camera placement per frame and nothing else: it is not a
        /// keyframe, a path, an easing or a shot. A mod that wants movement writes
        /// a new claim every frame, which is where interpolation belongs.
        ///
        /// A claimed camera is not the player's eye, so the eye's chrome (the
        /// crosshair with its aim outline, and the first-person hands) defaults
        /// off under it and the player's body draws instead;
        /// [`ClientViewChromeSet`](Self::ClientViewChromeSet) with `Some(true)`
        /// brings the chrome back. → [`HostRet::Unit`](crate::HostRet::Unit).
        ///
        /// With `anchor`, `pos` is an OFFSET from that player's or mob's presented
        /// feet, resolved when the frame presents — so a camera that follows a
        /// body never lags the body by the frame the mod read it on. An anchor
        /// not in the presented frame keeps the offset from its last presented
        /// feet, and [`ClientViewStateData::anchor_missing`](crate::ClientViewStateData::anchor_missing)
        /// says so.
        ClientViewCameraSet {
            pos: [f64; 3],
            yaw: f32,
            pitch: f32,
            roll: f32,
            fov_y: Option<f32>,
            anchor: Option<EntityRef>,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: drop this mod's camera claim; the player's own eye (or another
        /// mod's remaining claim) presents again from the next frame.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientViewCameraRelease => legal(CLIENT, Any, Write),
        /// CLIENT: this mod's opinion about the on-screen chrome. `None` per field
        /// = no opinion (the engine's default), and HIDING WINS when several mods
        /// have one — asking for a clean frame can never be overruled by a mod
        /// that merely likes the HUD on. Retained until the mod writes `None`
        /// again.
        ///
        /// Three switches, not a chrome layout: what the HUD, hands and crosshair
        /// ARE stays the engine's. `crosshair` also covers the aimed block's
        /// outline and the held tool's marks; a hidden `hud` still shows chat
        /// while the player has it open. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientViewChromeSet {
            hud: Option<bool>,
            hands: Option<bool>,
            crosshair: Option<bool>,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: claim which perspective presents — `Some(true)` third person,
        /// `Some(false)` first person, `None` releases the claim and the player's
        /// own toggle stands again. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientViewPerspectiveSet {
            third_person: Option<bool>,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: what the client is actually presenting this frame, after every
        /// mod's claims and the player's controls — the query-the-snapshot twin of
        /// the view claims. → [`HostRet::ClientViewState`](crate::HostRet::ClientViewState).
        ClientViewState => legal(CLIENT, Any, Read),
        /// CLIENT: override named shader params for this client only — the WRITE
        /// counterpart of [`ClientEnvParams`](Self::ClientEnvParams)'s read, so a
        /// mod can present an environment the server never replicated. At most
        /// [`CLIENT_ENV_OVERRIDE_MAX`](crate::CLIENT_ENV_OVERRIDE_MAX) keys, all
        /// finite; the call REPLACES this mod's whole override set, so an empty
        /// `params` clears it and every cleared key falls back to the replicated
        /// value.
        ///
        /// Overriding the engine's `petramond:time` (`[day fraction, _, moon
        /// phase, _]`) re-derives the sky's light from that fraction with the
        /// engine's own sky model, unless `petramond:light` is overridden too.
        ///
        /// Presentation only: light values, and so mob spawning, never change.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientEnvSet {
            params: Vec<(String, [f32; 4])>,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: replace this mod's retained world mark SET `set` — lines and
        /// camera-facing points at world positions, drawn over the world on this
        /// window until the mod sets it again; an empty `marks` clears it. A mod
        /// keeps as many named sets as it likes (a static path and a live
        /// marker, each replaced on its own); every mod's marks draw, in mod-id
        /// order and each mod's sets in name order (a later one's over an
        /// earlier's).
        ///
        /// `set` is lowercase letters, digits and `_`, never empty. Every point
        /// finite, every width positive and finite, every size finite and not
        /// negative, labels one line, image sprites in the mod's own namespace:
        /// anything else is refused whole, never drawn in part. A sprite that is
        /// not there (an image not yet published, a part this theme lacks) draws
        /// nothing.
        ///
        /// Marks are the window's, not the scene's: a frame capture never holds
        /// them. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientWorldMarksSet {
            set: String,
            marks: Vec<ClientWorldMark>,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: where this instance runs right now — beside a world, or on the
        /// shell with none, started from its pack's title-screen launch entry. It
        /// can change during an instance's life: a shell instance that opens a
        /// presentation runs beside its world until it closes.
        /// Legal in every context. → [`HostRet::ClientContext`](crate::HostRet::ClientContext).
        ClientContext => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: what each of this mod's key actions is bound to RIGHT NOW —
        /// the player's remap or the registered default — exactly as Options →
        /// Controls prints it (`"CTRL + Z"`, `"F9"`), so a tooltip names the key
        /// the player actually presses. `ids` are the bare ids the mod
        /// registered; the reply is parallel, `None` = no such action of this
        /// mod's. → [`HostRet::Names`](crate::HostRet::Names).
        ClientKeyLabels {
            ids: Vec<String>,
        } => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: where a write queued by [`Self::ClientStorageSetMany`] in the
        /// same `scope` stands: `true` = on disk, `false` = still queued;
        /// [`ErrorCode::Refused`](crate::ErrorCode::Refused) = the disk refused
        /// it (the reason is the filesystem's). Writes land in ticket order, and an answer never
        /// changes once given, so polling twice is harmless. A ticket this
        /// bucket never issued is an error.
        /// → [`HostRet::ClientStorageWritten`](crate::HostRet::ClientStorageWritten).
        ClientStorageWritePoll {
            scope: ClientStorageScope,
            ticket: u64,
        } => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: put the caret (at the end) in text input `id` (`item` for one
        /// stamped in a list) of this mod's OPEN document, as of the last frame.
        /// → [`HostRet::Bool`](crate::HostRet::Bool): `false` = no such enabled
        /// input is on screen.
        ClientUiFocus {
            id: String,
            item: Option<u32>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: open the engine's pause menu over this mod's open document or
        /// canvas; Resume returns to it. → [`HostRet::Bool`](crate::HostRet::Bool):
        /// `false` where there is no pause menu (the shell with no world), or this
        /// mod's UI is not what is on screen.
        ClientPauseOpen => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: claim the size the world frame renders at — `Some([w, h])`
        /// renders the world, and the HUD the view claims leave up, at exactly
        /// that size and shows it scaled to fit wherever it presents (the window,
        /// or a document `viewport` node); `None` releases the claim. The frame's
        /// HUD is laid out for the frame's size, so it keeps its proportions at
        /// any size. Last claimant in mod-id order. Sides from 1 to the device's
        /// [`ClientEngineFactsData::max_frame_side`](crate::ClientEngineFactsData::max_frame_side),
        /// else [`HostRet::Err`](crate::HostRet::Err).
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientViewFrameSet {
            size: Option<[u32; 2]>,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: present `player`'s view — their eye and head look, their
        /// first-person hands and held items, their aim — whenever no camera is
        /// claimed; a third-person perspective claim puts the engine's boom
        /// behind them. `None` releases it. Last claimant in mod-id order.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientViewSubjectSet {
            player: Option<PlayerId>,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: the replicated players and mobs the last presented frame drew
        /// — those in `ids`, and with `near` up to `max` of those within `radius`
        /// of `center` (an infinite radius: anywhere), nearest first. An id not
        /// in the frame has no row. → [`HostRet::ClientEntities`](crate::HostRet::ClientEntities).
        ClientEntities {
            ids: Vec<EntityRef>,
            near: Option<ClientEntitiesNear>,
        } => legal(CLIENT, Any, Read),
        /// CLIENT: what this build and machine are.
        /// → [`HostRet::ClientEngineFacts`](crate::HostRet::ClientEngineFacts).
        ClientEngineFacts => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: every installed id-bearing pack.
        /// → [`HostRet::ClientPacks`](crate::HostRet::ClientPacks).
        ClientPacks => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: the machine's wall clock, read-only. Client instances only:
        /// server mods stay clockless.
        /// → [`HostRet::ClientWallClock`](crate::HostRet::ClientWallClock).
        ClientWallClock => legal(CLIENT_SHELL, Any, Read),
    }
}
