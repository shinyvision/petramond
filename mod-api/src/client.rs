//! Presentation-only client module vocabulary ([`RuntimeSide::Client`]
//! instances): per-frame facts, overlays, canvases, pointer events, text runs.
//!
//! [`RuntimeSide::Client`]: crate::RuntimeSide::Client

use serde::{Deserialize, Serialize};

/// Bytes per packed surface cell: little-endian `i16` height followed by RGB.
pub const CLIENT_SURFACE_CELL_BYTES: usize = 5;
/// Bytes per packed surface column: 16×16 cells, row-major (z outer, x inner).
pub const CLIENT_SURFACE_COLUMN_BYTES: usize = 256 * CLIENT_SURFACE_CELL_BYTES;
/// Sentinel height marking a cell the replica does not currently know final.
pub const CLIENT_SURFACE_UNKNOWN_HEIGHT: i16 = i16::MIN;

/// One requested surface chunk column: chunk coordinates plus the host
/// revision the caller already holds COMPLETE data for (`0` = none). The host
/// skips re-encoding a column whose revision still matches, so callers must
/// only echo revisions from replies whose every cell was known.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClientSurfaceQuery {
    pub coord: [i32; 2],
    pub revision: u64,
}

/// One surface chunk column reply. The host derives each cell color from the
/// placed block's visible top texture, including the same 5×5 biome-blended
/// grass, foliage, or water tint used by terrain. `cells` is `None` when the
/// column is unchanged since the queried revision; otherwise it holds
/// [`CLIENT_SURFACE_COLUMN_BYTES`] packed cells ([`CLIENT_SURFACE_CELL_BYTES`]
/// each), with [`CLIENT_SURFACE_UNKNOWN_HEIGHT`] marking unknown cells.
/// Revisions are unique per replica session, including across column
/// unload/reload — equality means byte-identical surface content.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientSurfaceColumn {
    pub revision: u64,
    #[serde(with = "serde_bytes")]
    pub cells: Option<Vec<u8>>,
}

/// Read-only per-frame client facts. Client modules are presentation-only, so
/// this deliberately carries camera/player state without exposing sim-owned
/// mutation APIs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientFrameData {
    pub dt: f32,
    pub player_pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub screen: [u32; 2],
    pub open_gui: Option<String>,
    pub open_canvas: Option<String>,
}

/// Physical-screen anchor for an always-on client overlay image. Image texels
/// map one-to-one to screen pixels; GUI scale never applies.
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
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientPointerButton {
    Primary,
    Secondary,
}

/// Pointer event over a modal client canvas, in canvas-local logical pixels.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientCanvasEvent {
    pub phase: ClientPointerPhase,
    pub x: f32,
    pub y: f32,
    pub button: ClientPointerButton,
}

/// One retained element in a modal canvas scene. Coordinates live in the
/// canvas's logical pixel space and receive the canvas view offset at draw
/// time. Images scale with the canvas; sprites keep their native pixel size
/// while their centers follow the canvas transform.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientCanvasElement {
    Image { image_key: String, rect: [f32; 4] },
    Sprite { image_key: String, center: [f32; 2] },
}

/// One single-line text run drawn by the host's shared text subsystem.
/// Coordinates and the integer glyph scale are physical image pixels.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientTextRun {
    pub text: String,
    pub position: [i32; 2],
    pub scale: u8,
    pub color: [u8; 4],
}

/// A renderer-neutral event from one client GUI document.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientUiEvent {
    Click {
        id: String,
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
}
