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
    }
}
