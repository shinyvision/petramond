//! The presentation-only client surface: overlays, keys, images, text, GUIs,
//! canvases, sandboxed storage, environment and ambience, and the read-only
//! replica queries. Client instances only.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::client::{ClientCanvasElement, ClientOverlayAnchor, ClientSurfaceQuery, ClientTextRun};
use crate::data::GuiValue;
use crate::legality::prelude::*;

host_domain! {
    /// The presentation-only client surface: overlays, keys, images, text, GUIs,
    /// canvases, sandboxed storage, environment and ambience, and the read-only
    /// replica queries. Client instances only.
    ClientCall {
        /// Register an always-on physical-pixel overlay image. Legal only from a
        /// client instance during `mod_init`; the image may be published later.
        /// `margin` and `display_size` are physical screen pixels. →
        /// [`HostRet::Unit`](crate::HostRet::Unit).
        ClientRegisterOverlay {
            image_key: String,
            anchor: ClientOverlayAnchor,
            margin: [u16; 2],
            display_size: [u16; 2],
        } => legal(CLIENT, Init, Write),
        /// Register one REMAPPABLE key action: a stable bare `id` (the player's
        /// remap persists under `mod_id:id`), a display `label` for the Options →
        /// Controls screen (listed under the pack's name), the DEFAULT physical
        /// key (`"key_m"`, `"digit_1"`, …), and the opaque `action_id` delivered
        /// back in ClientKey events. Defaults colliding with an engine default are
        /// rejected by the app. Legal only during client `mod_init`. →
        /// [`HostRet::Unit`](crate::HostRet::Unit).
        ClientRegisterKey {
            id: String,
            label: String,
            key: String,
            action_id: u32,
        } => legal(CLIENT, Init, Write),
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
        } => legal(CLIENT, Any, Write),
        ClientUiStateGet {
            key: String,
        } => legal(CLIENT, Any, Read),
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
        } => legal(CLIENT, Any, Write),
        /// Measure a single-line run with the host's shared text subsystem. The
        /// returned size uses physical pixels after applying `scale`. →
        /// [`HostRet::ClientTextSize`](crate::HostRet::ClientTextSize).
        ClientTextMeasure {
            text: String,
            scale: u8,
        } => legal(CLIENT, Any, Read),
        /// Draw ordered text runs into an existing namespaced client image. This
        /// is a generic image/text capability: canvases, overlays, and GUI-fed
        /// images all use the same host glyphs and metrics. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientImageDrawTexts {
            key: String,
            runs: Vec<ClientTextRun>,
        } => legal(CLIENT, Any, Write),
        /// Request a client-owned GUI document open/close. These screens release
        /// the cursor but keep the replicated world running. → [`HostRet::Bool`](crate::HostRet::Bool)
        /// / [`HostRet::Unit`](crate::HostRet::Unit).
        ClientGuiOpen {
            kind_key: String,
        } => legal(CLIENT, Any, Write),
        ClientGuiClose => legal(CLIENT, Any, Write),
        /// Open/close a modal, centered physical-pixel canvas. While open,
        /// the cursor is released, gameplay input is gated, and pointer events are
        /// dispatched through [`GuestCall::ClientCanvas`](crate::GuestCall::ClientCanvas). → [`HostRet::Bool`](crate::HostRet::Bool) /
        /// [`HostRet::Unit`](crate::HostRet::Unit).
        ClientCanvasOpen {
            canvas_key: String,
            size: [u16; 2],
        } => legal(CLIENT, Any, Write),
        ClientCanvasClose => legal(CLIENT, Any, Write),
        /// Replace one canvas's retained, ordered scene. Image keys and the canvas
        /// key must belong to the caller. Ordinary panning must use
        /// [`ClientCall::ClientCanvasViewSet`] instead. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientCanvasSceneSet {
            canvas_key: String,
            elements: Vec<ClientCanvasElement>,
        } => legal(CLIENT, Any, Write),
        /// Change only the retained scene's logical-pixel translation. This is the
        /// hot path for panning and never republishes image bytes or scene nodes.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientCanvasViewSet {
            canvas_key: String,
            offset: [f32; 2],
        } => legal(CLIENT, Any, Write),
        /// Read a bounded batch of exact sandboxed client-storage keys. Results
        /// are parallel to `keys`; `None` means absent. Storage is scoped by
        /// server/world + mod id and inaccessible to other mods. This exact-key
        /// shape lets large spatial stores page only their working set. →
        /// [`HostRet::ClientStorageValues`](crate::HostRet::ClientStorageValues).
        ClientStorageGetMany {
            keys: Vec<String>,
        } => legal(CLIENT, Any, Read),
        /// Write a batch of sandboxed client-storage entries, committing each
        /// entry atomically. This is the hot-loop shape for explored map tiles;
        /// never cross once per tile.
        /// Keys must use the caller's namespace. → [`HostRet::Bool`](crate::HostRet::Bool).
        ClientStorageSetMany {
            entries: Vec<(String, serde_bytes::ByteBuf)>,
        } => legal(CLIENT, Any, Write),
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
        } => legal(CLIENT, Any, Write),
        /// Begin an ASYNCHRONOUS read of a bounded batch of exact sandboxed
        /// client-storage keys: the filesystem work runs on the background
        /// storage worker, so a slow disk delays the result instead of the
        /// frame. Ordered after already-queued writes (read-your-writes). Key
        /// rules and caps match [`ClientCall::ClientStorageGetMany`]; a bounded
        /// number of tickets may be outstanding at once. This is the REQUIRED
        /// path for bulk spatial reads — the synchronous form is for small
        /// startup/edit reads. → [`HostRet::U64`](crate::HostRet::U64) (the ticket).
        ClientStorageReadBegin {
            keys: Vec<String>,
        } => legal(CLIENT, Any, Write),
        /// Poll an asynchronous read begun by
        /// [`ClientCall::ClientStorageReadBegin`]. `Some(values)` (parallel to the
        /// begun keys, `None` entry = absent) consumes the ticket; `None` means
        /// still in flight — poll again next frame. Polling an unknown or
        /// already-consumed ticket is an error.
        /// → [`HostRet::ClientStorageRead`](crate::HostRet::ClientStorageRead).
        ClientStorageReadPoll {
            ticket: u64,
        } => legal(CLIENT, Any, Read),
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
    }
}
