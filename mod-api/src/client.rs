use serde::{Deserialize, Serialize};

pub const CLIENT_SURFACE_CELL_BYTES: usize = 5;
pub const CLIENT_SURFACE_COLUMN_BYTES: usize = 256 * CLIENT_SURFACE_CELL_BYTES;
pub const CLIENT_SURFACE_UNKNOWN_HEIGHT: i16 = i16::MIN;

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClientSurfaceQuery {
    pub coord: [i32; 2],
    pub revision: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientSurfaceColumn {
    pub revision: u64,
    #[serde(with = "serde_bytes")]
    pub cells: Option<Vec<u8>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientFrameData {
    pub dt: f32,
    pub player_pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub screen: [u32; 2],
    pub open_gui: Option<String>,
    pub open_canvas: Option<String>,
    pub gui_scale: u8,
    pub frozen: bool,
    pub wall_dt: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientContext {
    Shell,
    Local {
        name: String,
        shared: bool,
    },
    Remote {
        name: String,
        presentation_packs: bool,
    },
    Presentation {
        owner: String,
    },
}

impl ClientContext {
    pub fn has_world(&self) -> bool {
        !matches!(self, ClientContext::Shell)
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientEntitiesNear {
    pub center: [f64; 3],
    pub radius: f32,
    pub max: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientEntityData {
    pub id: crate::EntityRef,
    pub mob_kind: Option<crate::MobId>,
    pub name: Option<String>,
    pub feet: [f64; 3],
    pub eye: [f64; 3],
    pub look: [f32; 3],
    pub body_facing: [f32; 2],
    pub velocity: [f32; 3],
    pub size: [f32; 2],
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientStorageScope {
    World,
    Pack,
    Chosen(u32),
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientKeyMods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// Where a registered key action fires (`ClientRegisterKey`). A press
/// reaches the mod while the world takes gameplay input (`gameplay`) or while
/// one of `screens` — the mod's OWN document kinds and canvas keys — is the
/// open client screen; never while a text input has focus. A release always
/// reaches it, so an action cannot stay down.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientKeyContexts {
    pub gameplay: bool,
    pub screens: Vec<String>,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientOverlayAnchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientPointerPhase {
    Down,
    Move,
    Up,
    Leave,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientPointerButton {
    Primary,
    Secondary,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientCanvasEvent {
    pub phase: ClientPointerPhase,
    pub x: f32,
    pub y: f32,
    pub button: ClientPointerButton,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientCanvasElement {
    Image {
        image_key: String,
        rect: [f32; 4],
    },
    Sprite {
        image_key: String,
        center: [f32; 2],
    },
    Rect {
        rect: [f32; 4],
        color: [u8; 4],
        filled: bool,
    },
    Text {
        pos: [f32; 2],
        text: String,
        color: [u8; 4],
        small: bool,
        max_w: Option<f32>,
    },
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientViewStateData {
    pub third_person: bool,
    pub hud_visible: bool,
    pub hands_visible: bool,
    pub crosshair_visible: bool,
    pub camera_claimed: bool,
    pub fov_y: f32,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub frame_size: [u32; 2],
    pub subject: Option<crate::PlayerId>,
    pub anchor_missing: bool,
    pub world_settled: bool,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientEngineFactsData {
    pub capture_format: u16,
    pub protocol: u16,
    pub vocabulary: u64,
    pub tick_dt: f32,
    pub max_frame_side: u32,
    pub max_frame_bytes: u64,
    pub guest_memory_max: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientPackInfo {
    pub id: String,
    pub version: String,
    pub affects_world: bool,
    pub client_wasm: bool,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClientWallTime {
    pub unix_ms: i64,
    pub utc_offset_min: i16,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientSprite {
    Image { key: String },
    Theme { part: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientWorldMark {
    Line {
        from: [f64; 3],
        to: [f64; 3],
        color: [u8; 4],
        width: f32,
        occluded: u8,
    },
    Point {
        pos: [f64; 3],
        sprite: Option<ClientSprite>,
        size: f32,
        color: [u8; 4],
        label: Option<String>,
        occluded: u8,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientTextRun {
    pub text: String,
    pub position: [i32; 2],
    pub scale: u8,
    pub color: [u8; 4],
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientUiEvent {
    Click {
        id: String,
        item: Option<u32>,
    },
    TextChanged {
        id: String,
        text: String,
    },
    Submit {
        id: String,
        text: String,
    },
    ImagePointer {
        id: String,
        phase: ClientPointerPhase,
        x: f32,
        y: f32,
        button: ClientPointerButton,
    },
    Toggle {
        id: String,
        item: Option<u32>,
        on: bool,
    },
    Slider {
        id: String,
        item: Option<u32>,
        value: f32,
        committed: bool,
    },
    ListSelect {
        id: String,
        index: u32,
    },
    ListActivate {
        id: String,
        index: u32,
    },
    TabSelect {
        id: String,
        index: u32,
    },
    Dismiss,
    Hover {
        id: Option<String>,
        item: Option<u32>,
    },
    Blur {
        id: String,
        item: Option<u32>,
    },
    ListRange {
        id: String,
        first: u32,
        count: u32,
    },
    CanvasPointer {
        id: String,
        item: Option<u32>,
        phase: ClientPointerPhase,
        x: f32,
        y: f32,
        button: Option<ClientPointerButton>,
        mods: ClientKeyMods,
        clicks: u8,
    },
    CanvasScroll {
        id: String,
        item: Option<u32>,
        x: f32,
        y: f32,
        delta: f32,
        mods: ClientKeyMods,
    },
    CanvasSize {
        id: String,
        item: Option<u32>,
        w: u32,
        h: u32,
    },
}
