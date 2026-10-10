use crate::client::{
    ClientCanvasElement, ClientEntitiesNear, ClientKeyContexts, ClientKeyMods, ClientOverlayAnchor,
    ClientStorageScope, ClientSurfaceQuery, ClientTextRun, ClientWorldMark,
};
use crate::data::{EntityRef, GuiValue};
use crate::ids::PlayerId;
use crate::legality::prelude::*;

host_domain! {
    ClientCall {
        ClientRegisterOverlay {
            image_key: String,
            anchor: ClientOverlayAnchor,
            margin: [u16; 2],
            display_size: [u16; 2],
            hud: bool,
        } => legal(CLIENT_SHELL, Init, Write),
        ClientRegisterKey {
            id: String,
            label: String,
            key: String,
            mods: ClientKeyMods,
            contexts: ClientKeyContexts,
            action_id: u32,
        } => legal(CLIENT_SHELL, Init, Write),
        ClientSurfaceColumns {
            queries: Vec<ClientSurfaceQuery>,
        } => legal(CLIENT, Any, Read),
        ClientUiStateSet {
            key: String,
            value: GuiValue,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientUiStateGet {
            key: String,
        } => legal(CLIENT_SHELL, Any, Read),
        ClientImageSet {
            key: String,
            width: u16,
            height: u16,
            #[serde(with = "serde_bytes")]
            rgba: Vec<u8>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientTextMeasure {
            text: String,
            scale: u8,
        } => legal(CLIENT_SHELL, Any, Read),
        ClientImageDrawTexts {
            key: String,
            runs: Vec<ClientTextRun>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientGuiOpen {
            kind_key: String,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientGuiClose => legal(CLIENT_SHELL, Any, Write),
        ClientCanvasOpen {
            canvas_key: String,
            size: [u16; 2],
        } => legal(CLIENT_SHELL, Any, Write),
        ClientCanvasClose => legal(CLIENT_SHELL, Any, Write),
        ClientCanvasSceneSet {
            canvas_key: String,
            elements: Vec<ClientCanvasElement>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientCanvasViewSet {
            canvas_key: String,
            offset: [f32; 2],
        } => legal(CLIENT_SHELL, Any, Write),
        ClientStorageGetMany {
            scope: ClientStorageScope,
            keys: Vec<String>,
        } => legal(CLIENT_SHELL, Any, Read),
        ClientStorageSetMany {
            scope: ClientStorageScope,
            entries: Vec<(String, Option<serde_bytes::ByteBuf>)>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientImageBlit {
            key: String,
            origin: [u16; 2],
            size: [u16; 2],
            #[serde(with = "serde_bytes")]
            rgba: Vec<u8>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientStorageReadBegin {
            scope: ClientStorageScope,
            keys: Vec<String>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientStorageReadPoll {
            scope: ClientStorageScope,
            ticket: u64,
        } => legal(CLIENT_SHELL, Any, Read),
        ClientEnvParams {
            keys: Vec<String>,
        } => legal(CLIENT, Any, Read),
        ClientBiomeAt {
            pos: [i32; 2],
        } => legal(CLIENT, Any, Read),
        ClientAmbientSet {
            key: String,
            intensity: f32,
            wind: [f32; 2],
        } => legal(CLIENT, Any, Write),
        ClientLoopSet {
            key: String,
            gain: f32,
        } => legal(CLIENT, Any, Write),
        ClientMoodSet {
            darken: f32,
            desaturate: f32,
        } => legal(CLIENT, Any, Write),
        ClientBlocksAt {
            positions: Vec<[i32; 3]>,
        } => legal(CLIENT, Any, Read),
        ClientCellKvAt {
            key: String,
            cells: Vec<[i32; 3]>,
        } => legal(CLIENT, Any, Read),
        ClientViewCameraSet {
            pos: [f64; 3],
            yaw: f32,
            pitch: f32,
            roll: f32,
            fov_y: Option<f32>,
            anchor: Option<EntityRef>,
        } => legal(CLIENT, Any, Write),
        ClientViewCameraRelease => legal(CLIENT, Any, Write),
        ClientViewChromeSet {
            hud: Option<bool>,
            hands: Option<bool>,
            crosshair: Option<bool>,
        } => legal(CLIENT, Any, Write),
        ClientViewPerspectiveSet {
            third_person: Option<bool>,
        } => legal(CLIENT, Any, Write),
        ClientViewState => legal(CLIENT, Any, Read),
        ClientEnvSet {
            params: Vec<(String, [f32; 4])>,
        } => legal(CLIENT, Any, Write),
        ClientWorldMarksSet {
            set: String,
            marks: Vec<ClientWorldMark>,
        } => legal(CLIENT, Any, Write),
        ClientContext => legal(CLIENT_SHELL, Any, Read),
        ClientKeyLabels {
            ids: Vec<String>,
        } => legal(CLIENT_SHELL, Any, Read),
        ClientStorageWritePoll {
            scope: ClientStorageScope,
            ticket: u64,
        } => legal(CLIENT_SHELL, Any, Read),
        ClientUiFocus {
            id: String,
            item: Option<u32>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientPauseOpen => legal(CLIENT_SHELL, Any, Write),
        ClientViewFrameSet {
            size: Option<[u32; 2]>,
        } => legal(CLIENT, Any, Write),
        ClientViewSubjectSet {
            player: Option<PlayerId>,
        } => legal(CLIENT, Any, Write),
        ClientEntities {
            ids: Vec<EntityRef>,
            near: Option<ClientEntitiesNear>,
        } => legal(CLIENT, Any, Read),
        ClientEngineFacts => legal(CLIENT_SHELL, Any, Read),
        ClientPacks => legal(CLIENT_SHELL, Any, Read),
        ClientWallClock => legal(CLIENT_SHELL, Any, Read),
        /// The wind every cloth on this client blows in: XZ velocity in blocks per
        /// second, `None` to release. Among mods that set one, the last in load order
        /// wins; with none set the engine blows a gentle breeze toward -X (east to west).
        ClientClothWindSet {
            wind: Option<[f32; 2]>,
        } => legal(CLIENT, Any, Write),
        /// Sends one of this mod's own events to the session's server, the client-to-server
        /// mirror of [`CoreCall::EmitEventTo`] with the same guards: a key in the caller's own
        /// namespace and at most [`EVENT_MAX_DATA_BYTES`] of data. The server instance of the
        /// same mod gets an [`EventKind::ClientEvent`] naming the sending player; no other mod
        /// sees it. Returns [`HostRet::Bool`](crate::HostRet::Bool): true once it is queued
        /// for the server, false if nobody can hear it (playback, no live session) or this
        /// instance already holds as many unsent events as the server takes at once.
        ///
        /// It carries intent, not state: the bytes come from a client the server doesn't
        /// trust, so the server half validates them and decides. The server dispatches an
        /// event at the start of the first tick after it arrives, before the same client's
        /// queued input (menu clicks, close) is applied in that tick, and one client's events
        /// keep their send order. The server bounds how many it takes per session and drops
        /// the excess without an answer, so true is not delivery: send edges, not a
        /// continuous stream of state.
        ///
        /// [`CoreCall::EmitEventTo`]: crate::CoreCall::EmitEventTo
        /// [`EVENT_MAX_DATA_BYTES`]: crate::EVENT_MAX_DATA_BYTES
        /// [`EventKind::ClientEvent`]: crate::EventKind::ClientEvent
        ClientEmitEvent {
            key: String,
            #[serde(with = "serde_bytes")]
            data: Vec<u8>,
        } => legal(CLIENT, Any, Write),
        /// The menu the server has open for this player, as this client sees it;
        /// `None` unless that menu's kind is a mod document. Any mod may read it.
        ClientMenu => legal(CLIENT, Any, Read),
        /// The pixels of one atlas tile as the world shows it untinted: 16×16 RGBA, rows
        /// top to bottom, the first frame of an animated tile. Returns
        /// [`HostRet::Bytes`](crate::HostRet::Bytes), `None` for a name that is not a tile.
        ClientTilePixels {
            tile: String,
        } => legal(CLIENT_SHELL, Any, Read),
    }
}
