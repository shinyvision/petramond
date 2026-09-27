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
/// mutation APIs. In the [`ClientContext::Shell`] there is no player: the
/// player fields are zero and only `dt`, `screen`, `gui_scale`, `frozen` and
/// the open surfaces say anything.
///
/// `gui_scale` is the WINDOW's UI scale — what an overlay drawn in physical
/// pixels scales by to sit beside the engine's own UI. `frozen` is `true` on
/// a frame that does not tick the presented world (a pause menu over a
/// world that stops, the shell): `dt` is still the wall's, the world's time
/// is not moving.
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
    /// Wall seconds since the previous `ClientFrame`. Equal to `dt` on the
    /// wall clock; a stepped presentation clock (`ClientClockSet`) owns `dt`,
    /// and this still says how much real time passed.
    pub wall_dt: f32,
}

/// Where a client instance runs right now, and what that session is
/// ([`ClientCall::ClientContext`]).
///
/// [`ClientCall::ClientContext`]: crate::ClientCall::ClientContext
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientContext {
    /// On the app's shell with no world at all: the instance was started from
    /// its pack's title-screen launch entry. Only calls that read or write no
    /// world answer here; every other client call is refused with an error.
    Shell,
    /// A world this machine hosts. `shared` = open to LAN: other players are
    /// in it, so pausing does not stop it.
    Local { name: String, shared: bool },
    /// A server this client joined. `name` is the address this client
    /// joined it by, as the player gave it: the same server reached by
    /// another address has another name. `presentation_packs` is what the
    /// server consents to: whether presentation-only packs it does not run
    /// may load here. Consent is signalled, not enforced; the honest client
    /// obeys it.
    Remote {
        name: String,
        presentation_packs: bool,
    },
    /// A world-less presentation of mod-file byte ranges
    /// (`ClientPresentationOpen`), opened from the shell by `owner`. No
    /// server, no save and no simulation stand behind it.
    Presentation { owner: String },
}

impl ClientContext {
    /// Whether this instance runs beside a presented world at all.
    pub fn has_world(&self) -> bool {
        !matches!(self, ClientContext::Shell)
    }
}

/// A [`ClientCall::ClientEntities`] search around a point: up to `max` of the
/// entities within `radius` (infinite = every presented one).
///
/// [`ClientCall::ClientEntities`]: crate::ClientCall::ClientEntities
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientEntitiesNear {
    pub center: [f64; 3],
    pub radius: f32,
    pub max: u32,
}

/// One replicated player or mob as the last presented frame drew it
/// ([`ClientCall::ClientEntities`]). Positions are interpolated exactly as
/// drawn; directions are unit vectors, so no yaw convention leaks.
///
/// [`ClientCall::ClientEntities`]: crate::ClientCall::ClientEntities
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientEntityData {
    pub id: crate::EntityRef,
    pub mob_kind: Option<crate::MobId>,
    /// A player's name, or a mob's custom name.
    pub name: Option<String>,
    pub feet: [f64; 3],
    pub eye: [f64; 3],
    pub look: [f32; 3],
    /// Horizontal unit direction the body faces, `[x, z]`.
    pub body_facing: [f32; 2],
    pub velocity: [f32; 3],
    /// `[width, height]` in blocks.
    pub size: [f32; 2],
}

/// Which sandboxed bucket a client-storage call addresses.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientStorageScope {
    /// The presented session's own bucket, keyed by world (or server) and
    /// mod: exploration, waypoints — data that belongs to ONE world. There
    /// is none in the [`ClientContext::Shell`], and writes are refused while
    /// a presentation presents.
    World,
    /// The mod's own bucket, keyed by mod alone: it follows the pack across
    /// every world, the shell and every presentation — projects, preferences.
    Pack,
}

/// The modifier chord a registered key action's DEFAULT requires held
/// (`ClientRegisterKey`). All `false` = the bare key.
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
    /// An uncaptured pointer left a document canvas or viewport.
    Leave,
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
///
/// [`Rect`](Self::Rect) and [`Text`](Self::Text) are the primitives a canvas
/// needs to draw its own structure — rules, panels, tick marks, labels —
/// without a mod having to publish an image for every line of it. They are
/// GEOMETRY and GLYPHS, nothing more: no widget vocabulary, no layout, no
/// notion of what the canvas is for.
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
    /// An axis-aligned `[x, y, w, h]` rectangle in RGBA: `filled` paints it,
    /// otherwise it is a one-pixel outline.
    Rect {
        rect: [f32; 4],
        color: [u8; 4],
        filled: bool,
    },
    /// One line of text with its top-left at `pos`, at most
    /// [`CLIENT_CANVAS_TEXT_MAX`](crate::CLIENT_CANVAS_TEXT_MAX) bytes.
    /// `small` picks the host's compact glyph size over its normal one — the
    /// only two sizes the shared text subsystem offers, so a canvas never
    /// carries a font. `max_w` ellipsizes it into `[x, x + max_w]`; `None`
    /// runs it to the canvas's right edge.
    Text {
        pos: [f32; 2],
        text: String,
        color: [u8; 4],
        small: bool,
        max_w: Option<f32>,
    },
}

/// What the client is PRESENTING this frame, as `ClientViewState` answers it:
/// the resolved view, after every mod's view claims and the player's own
/// controls. A mod reads it to tell whether its claim reached the screen and
/// to leave alone what it does not own.
///
/// This is a SNAPSHOT of presentation, not a timeline, a shot or a take: the
/// engine has no opinion about what a mod is doing with the view.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientViewStateData {
    pub third_person: bool,
    pub hud_visible: bool,
    pub hands_visible: bool,
    pub crosshair_visible: bool,
    /// Whether the presented camera comes from a mod's view claim rather than
    /// the player's own eye.
    pub camera_claimed: bool,
    /// The presented vertical field of view, radians.
    pub fov_y: f32,
    /// The presented camera: where it stood and how it looked, whoever
    /// placed it (the eye, the boom, a claim).
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    /// The size the world frame was rendered at (a frame-size claim;
    /// otherwise the window's).
    pub frame_size: [u32; 2],
    /// The player whose view presents (`ClientViewSubjectSet`), if claimed.
    pub subject: Option<crate::PlayerId>,
    /// The claimed camera's anchor was not in the presented frame: its
    /// offset was taken from the anchor's last presented feet.
    pub anchor_missing: bool,
    /// The world on screen is settled: no terrain being meshed, relit or
    /// streamed, nothing waiting to upload — the rule a `Settled` frame
    /// capture is taken by.
    pub world_settled: bool,
}

/// What THIS build and machine are (`ClientEngineFacts`). The constants a
/// mod was compiled against may differ from the running engine; this answers
/// the truth.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientEngineFactsData {
    /// The capture format this build writes and reads.
    pub capture_format: u16,
    /// The opaque capture bodies' version this build writes and reads.
    pub protocol: u16,
    /// This build's id vocabulary, as a capture piece names it.
    pub vocabulary: u64,
    /// Seconds per tick (0.05 as `f32`: NOT exactly 1/20).
    pub tick_dt: f32,
    /// The rendering device's largest 2D texture side (0 = no rendering device).
    pub max_frame_side: u32,
    /// The rendering device's largest buffer, a capture's readback (0 = no rendering device).
    pub max_frame_bytes: u64,
    /// How far the calling instance's linear memory can grow: its module's
    /// declared maximum, else wasm32's 4 GiB.
    pub guest_memory_max: u64,
}

/// One installed id-bearing pack (`ClientPacks`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientPackInfo {
    pub id: String,
    pub version: String,
    pub affects_world: bool,
    pub client_wasm: bool,
}

/// The machine's wall clock (`ClientWallClock`), read-only.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClientWallTime {
    pub unix_ms: i64,
    /// The machine's UTC offset now, in minutes.
    pub utc_offset_min: i16,
}

/// Art a client prim draws: one of the calling mod's published images, by
/// key, or a part of the host's UI theme, by part key (`"icon.plus"`) — drawn
/// with the part's resting face.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientSprite {
    Image { key: String },
    Theme { part: String },
}

/// One world-space mark a client mod retains (`ClientWorldMarksSet`): world
/// points are `f64`, sizes are the window's pixels, so a mark reads the same
/// at any distance and never jitters far from the origin.
///
/// `occluded` is the mark's opacity where the world stands in front of it:
/// `0` hides it behind terrain, `255` draws it straight through, and anything
/// between shows the hidden part faded. A line is judged along its length; a
/// point, label included, by its world point alone.
///
/// Marks are presentation on THIS window only. They are never part of a
/// captured frame (a `Scene` capture holds the scene, not the window's chrome).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientWorldMark {
    /// A straight segment between two world points, `width` pixels wide.
    Line {
        from: [f64; 3],
        to: [f64; 3],
        color: [u8; 4],
        width: f32,
        occluded: u8,
    },
    /// A camera-facing `sprite` centred on a world point, `size` pixels tall
    /// (its width keeps the sprite's aspect); no sprite draws a square dot of
    /// `color`, and `size == 0` draws no body at all. `label` is one line of
    /// text under the body, in the theme's font at the window's GUI scale.
    /// `color` tints the sprite and colours the label.
    Point {
        pos: [f64; 3],
        sprite: Option<ClientSprite>,
        size: f32,
        color: [u8; 4],
        label: Option<String>,
        occluded: u8,
    },
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

/// A renderer-neutral event from one client GUI document. `item` is the
/// list-item index when the widget was stamped from a list template.
/// Button, checkbox and toggle presses are the primary button's only (the
/// engine's one activation rule for every document).
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
    /// A checkbox or toggle was pressed; `on` is the value it asks for.
    Toggle {
        id: String,
        item: Option<u32>,
        on: bool,
    },
    /// A slider moved: `committed == false` while dragging (live preview),
    /// `true` once on release — one drag, one committed value.
    Slider {
        id: String,
        item: Option<u32>,
        value: f32,
        committed: bool,
    },
    /// A list row was pressed.
    ListSelect {
        id: String,
        index: u32,
    },
    /// A list row was double-clicked or Enter was pressed on it.
    ListActivate {
        id: String,
        index: u32,
    },
    /// A tab of a tab bar was pressed.
    TabSelect {
        id: String,
        index: u32,
    },
    /// Escape, on a document whose `dismiss` is `event`: the document stays
    /// open, and its owner decides what Escape unwinds.
    Dismiss,
    /// The topmost named widget under the pointer changed (`id: None` =
    /// none). Sent only on change.
    Hover {
        id: Option<String>,
        item: Option<u32>,
    },
    /// A text input lost focus, however it lost it.
    Blur {
        id: String,
        item: Option<u32>,
    },
    /// The stamps of a list that are at least partly in view changed —
    /// items `first..first + count`. What a mod loads per-row content for.
    ListRange {
        id: String,
        first: u32,
        count: u32,
    },
    /// The pointer over a document `canvas` or `viewport` node with
    /// `interactive`, in logical px local to its rect: presses with `clicks`
    /// (the press's count in a double-click streak), moves (at most one per
    /// frame; `button` while held — a press captures the pointer until its
    /// release), releases, and `Leave` when an uncaptured pointer leaves.
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
    /// Wheel notches over such a node (positive = up), never scrolling an
    /// enclosing scroll.
    CanvasScroll {
        id: String,
        item: Option<u32>,
        x: f32,
        y: f32,
        delta: f32,
        mods: ClientKeyMods,
    },
    /// Such a node's solved size in logical px, on its first frame and
    /// whenever the layout changes it.
    CanvasSize {
        id: String,
        item: Option<u32>,
        w: u32,
        h: u32,
    },
}
