//! The GUI document model: a tree of themed, layout-managed nodes.
//!
//! A document is the single authored artifact for one GUI — a shell screen, a
//! slot container, the HUD, or a mod GUI. The builder edits documents; the game
//! runtime interprets them directly. There is no bake step: visuals come from
//! the theme kit, geometry from the layout solver, behavior from the widget
//! state machines.
//!
//! Documents deliberately know nothing about the game: slots are identified by
//! role *strings* + in-role index (the host maps them to its own slot
//! identities), dynamic content flows through named state-key bindings, and
//! host-drawn regions (item icons, hearts) reserve space via `hook` nodes.

use serde::{Deserialize, Serialize};

/// The document format version this crate reads and writes.
pub const FORMAT_VERSION: u32 = 1;

/// One GUI document: its kind key (namespaced, e.g. `petramond:furnace` or
/// `somemod:wheel`), its class, and the node tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub format: u32,
    /// The kind key the host registers/opens this document under.
    pub kind: String,
    pub class: DocClass,
    /// Compact breakpoint: when the solve viewport is narrower than this many
    /// logical px, every node with a `compact_layout` arranges by it instead
    /// of `layout`. Same tree, same widgets, different arrangement — slots and
    /// bindings can never diverge between the two forms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_below_w: Option<i32>,
    /// What Escape does to this document: `close` it (the default), or
    /// deliver a dismiss EVENT to its owner and stay open — for a document
    /// that has its own layers to unwind first (a popup, an edit in
    /// progress, a selection).
    #[serde(default, skip_serializing_if = "Dismiss::is_close")]
    pub dismiss: Dismiss,
    pub root: Node,
}

/// What Escape does to a document ([`Document::dismiss`]).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dismiss {
    #[default]
    Close,
    Event,
}

impl Dismiss {
    pub fn is_close(&self) -> bool {
        *self == Dismiss::Close
    }
}

impl Document {
    /// Whether a viewport this many logical px wide arranges by the compact
    /// layouts.
    pub fn compact_active(&self, viewport_w: i32) -> bool {
        self.compact_below_w.is_some_and(|w| viewport_w < w)
    }
}

/// What kind of surface a document is. The host uses this to decide input
/// routing (containers get the cursor-stack/slot click path; screens get
/// controller dispatch) and backdrop dimming.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocClass {
    /// An app-shell screen (title, world select, pause…).
    Screen,
    /// A slot-bearing menu over gameplay (inventory, chest, furnace…).
    Container,
    /// Always-on overlay chrome (hotbar).
    Hud,
}

/// One node of the document tree.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Node {
    /// Stable id within the document: the key for events, named rects, and
    /// per-widget ephemeral state. Required on event-bearing widgets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(flatten)]
    pub kind: NodeKind,
    #[serde(default, skip_serializing_if = "LayoutProps::is_default")]
    pub layout: LayoutProps,
    /// COMPLETE replacement layout used while the document's compact
    /// breakpoint is active (see [`Document::compact_below_w`]); it does not
    /// inherit from `layout`, so restate every field the node needs. Absent =
    /// the same layout at every size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_layout: Option<Box<LayoutProps>>,
    /// Theme part key (e.g. `button.danger`). `None` = the widget's default
    /// part for its type, or unskinned for plain containers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Paint this subtree in the OVERLAY tier: after the base tier's chrome
    /// AND the host's base-tier content (item icons), before tooltips. For a
    /// widget that must sit on TOP of host-drawn content — the anvil's
    /// augment slot over its enlarged tool view. Unlike a tooltip, the node
    /// stays in flow and stays fully interactive (hit testing is unchanged).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overlay: bool,
    /// `label`, `button` and `badge`: when the text had to be ellipsized to
    /// fit and the pointer rests on it, the engine shows the whole text in a
    /// standard tooltip. Nothing shows when nothing was cut.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ellipsis_tip: bool,
    #[serde(default, skip_serializing_if = "Bindings::is_empty")]
    pub bind: Bindings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

// A node's kind is flattened when written, but deserialization splits the
// common fields from the tagged kind. Each half rejects unknown keys, so a
// misspelled property cannot silently vanish through serde(flatten).
impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;

        #[derive(Default, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Common {
            id: Option<String>,
            layout: LayoutProps,
            compact_layout: Option<Box<LayoutProps>>,
            style: Option<String>,
            overlay: bool,
            ellipsis_tip: bool,
            bind: Bindings,
            children: Vec<Node>,
        }

        let mut value = serde_json::Value::deserialize(deserializer)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| D::Error::custom("a document node must be an object"))?;
        let mut common = serde_json::Map::new();
        for key in [
            "id",
            "layout",
            "compact_layout",
            "style",
            "overlay",
            "ellipsis_tip",
            "bind",
            "children",
        ] {
            if let Some(value) = object.remove(key) {
                common.insert(key.to_owned(), value);
            }
        }
        let common: Common =
            serde_json::from_value(serde_json::Value::Object(common)).map_err(D::Error::custom)?;
        // Serde's internally tagged unit variants accept stray object keys
        // even with deny_unknown_fields on the enum. These kinds have no
        // kind-specific fields, so only the tag may remain here.
        if matches!(
            object.get("type").and_then(serde_json::Value::as_str),
            Some("frame" | "row" | "column" | "spacer" | "checkbox" | "hook")
        ) {
            if let Some(key) = object.keys().find(|key| key.as_str() != "type") {
                return Err(D::Error::custom(format!("unknown node field '{key}'")));
            }
        }
        let kind = serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(Node {
            id: common.id,
            kind,
            layout: common.layout,
            compact_layout: common.compact_layout,
            style: common.style,
            overlay: common.overlay,
            ellipsis_tip: common.ellipsis_tip,
            bind: common.bind,
            children: common.children,
        })
    }
}

/// The node type plus its type-specific properties. Serialized internally
/// tagged as `"type"` so documents read naturally.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NodeKind {
    /// Generic container; lays children out along `layout.dir`.
    Frame,
    /// Container fixed to horizontal flow.
    Row,
    /// Container fixed to vertical flow.
    Column,
    /// Empty flexible space (give it `grow` or a fixed size).
    Spacer,
    Label {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        wrap: bool,
        /// Glyph-size multiplier for headings (1 = the base pixel font).
        /// `wrap` is ignored when > 1.
        #[serde(default = "default_text_scale", skip_serializing_if = "is_one")]
        scale: u32,
        /// Draw one GUI-SCALE STEP smaller — secondary text (a tooltip's
        /// name, a caption) without a second font.
        ///
        /// A bitmap font has exactly one crisp size, so text cannot simply be
        /// set smaller; but the whole UI is drawn at an integer scale, so
        /// dropping this run from `scale` to `scale - 1` physical px per font
        /// pixel keeps it exactly on the pixel grid. It has no effect at gui
        /// scale 1, where there is no smaller step.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        small: bool,
        /// A `wrap` label stops at this many lines, the last one ellipsized:
        /// a bound description can never push its row off the screen.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_lines: Option<u32>,
    },
    /// An image beside the document (path relative to the document; a bound
    /// `image` key overrides the name per instance — list-row icons).
    Image {
        image: String,
        #[serde(default, skip_serializing_if = "ImageFit::is_stretch")]
        fit: ImageFit,
        /// Sprite-sheet grid `[cols, rows]`: the image is a sheet of
        /// `cols × rows` equal frames, row-major, and only ONE frame draws
        /// (and sizes the node). `bind.frame` picks the frame; `fps` cycles.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frames: Option<[u32; 2]>,
        /// Animation rate in frames per second from `FrameState.now`. Only a
        /// positive finite value animates; a bound `frame` wins over it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<f32>,
        /// Emit local pointer down/move/up events for this image. This is the
        /// renderer-neutral canvas seam used by host-fed maps and other
        /// interactive raster surfaces.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        interactive: bool,
    },
    /// A textured quad rotated at draw time by the radians bound at
    /// `bind.value`. `pivot` is logical px from the rect's top-left;
    /// `None` = rect centre.
    Rotimage {
        image: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pivot: Option<[f32; 2]>,
    },
    Button {
        /// Inline label for a leaf button. A compound button uses `children`
        /// instead and leaves `text`/`icon` unset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// The button's icon (centred alone, or left of the label): a theme
        /// part (`icon.edit`), or a `.png` beside the document drawn on the
        /// themed face. `bind.icon` swaps it per frame and per list stamp.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
        /// A document image drawn as the button's face INSTEAD of the theme
        /// chrome (an image-backed button: no text/icon/children — validation
        /// rejects them). Click behavior is unchanged; the enabled/hover/
        /// pressed affordance is a tint on the image. A bound `image` key
        /// overrides the name per instance.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<String>,
        /// Sprite-sheet grid `[cols, rows]` for the button image (see
        /// [`NodeKind::Image`]); the natural size is ONE frame.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frames: Option<[u32; 2]>,
        /// Animation rate for the button image (see [`NodeKind::Image`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<f32>,
    },
    Checkbox,
    Toggle {
        /// Icon drawn centred on the toggle face — an on/off icon button
        /// (the crafting browser's craftable-only filter): a theme part, or a
        /// `.png` beside the document; `bind.icon` swaps it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
    },
    Slider {
        min: f32,
        max: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f32>,
    },
    TextInput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        #[serde(default = "default_max_chars")]
        max_chars: usize,
        /// Draw every character as `*` instead of itself (a password field).
        /// Presentation only: the editor, the bound value and the clipboard all
        /// still hold the real text, so masking cannot lose what was typed.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        masked: bool,
    },
    /// Clipping scroll region around its children.
    Scroll {
        #[serde(default)]
        axis: ScrollAxis,
    },
    /// Repeats its single template child once per item in `bind.items`.
    ///
    /// `cols` > 1 arranges the stamps as a fixed-column, row-major grid
    /// instead of a single flow line: cells share the content width evenly
    /// (integer remainder to the leading columns) and every cell is as tall as
    /// the tallest stamp, so an icon grid stays aligned at any width. The
    /// column count is fixed rather than wrapped-to-fit because a
    /// width-dependent height cannot be measured before arrange.
    List {
        #[serde(default = "default_cols", skip_serializing_if = "is_one")]
        cols: u32,
    },
    /// One host-mapped slot (item cell) of `role`. `accepts` and `take_only`
    /// are host-interpreted slot semantics carried verbatim (the document
    /// runtime ignores them): which item groups quick-moves may route into
    /// this slot, and whether clicks may only remove from it (an output).
    Slot {
        role: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        accepts: Vec<Accept>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        take_only: bool,
    },
    /// A `cols`×`rows` grid of `role` slots, generated row-major — the in-role
    /// index ↔ cell order contract holds by construction. `accepts`/`take_only`
    /// apply to every cell of the grid (see [`NodeKind::Slot`]).
    SlotGrid {
        role: String,
        cols: u32,
        rows: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        accepts: Vec<Accept>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        take_only: bool,
    },
    /// A 0..=1 fill gauge (furnace arrow/flame; mod gauges).
    Gauge {
        mode: GaugeMode,
    },
    Badge {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Alert {
        level: AlertLevel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// A horizontal bar of selectable tabs. Selection is host-bound state
    /// (`bind.selected`, an `I32` index) — the bar only *requests* a change
    /// (`UiEvent::TabSelect`, fired on pointer down like list rows); pages are
    /// plain sibling frames the host shows/hides via `visible` binds.
    TabBar {
        tabs: Vec<TabSpec>,
    },
    /// Layout-reserved, host-drawn region (hearts, item previews). The host
    /// reads its solved rect from the frame output by id.
    Hook,
    /// A panel that floats at the pointer instead of taking part in flow: it
    /// arranges at its natural size, offset from the cursor by `layout.abs`
    /// (flipping to the other side rather than running off-screen), is never
    /// clipped by an ancestor, never receives pointer input, and paints above
    /// everything — including host content (see [`crate::DrawList`]).
    ///
    /// Visibility is ordinary host-bound state: bind `visible` and the tooltip
    /// exists only on the frames the host wants it. Content is ordinary nodes,
    /// so a tooltip carries no knowledge of what it describes.
    /// A host-kept 2-D scene painted into the node's solved rect, clipped to
    /// it: `bind.scene` names the scene, and one scene unit is one logical
    /// pixel. `interactive` reports pointer and wheel events over it, local
    /// to the rect, and its size whenever the layout changes it.
    Canvas {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        interactive: bool,
    },
    /// Where the host presents its world frame: a layout-reserved region the
    /// host draws into (like a `hook`), with a canvas's pointer, wheel and
    /// size events when `interactive`.
    Viewport {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        interactive: bool,
    },
    Tooltip {
        /// Optional widget anchor: the tooltip expands only while a widget
        /// with this `id` is under the cursor (one frame of lag, the same
        /// contract as hover-revealed list content). Absent = hover of
        /// anything (or nothing) shows it whenever `visible` holds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hover: Option<String>,
    },
}

/// One tab of a [`NodeKind::TabBar`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabSpec {
    /// Stable host-facing name for the tab (not displayed).
    pub key: String,
    /// Theme part drawn as the tab's icon (e.g. `icon.tab_world`), centred
    /// alone or left of the label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Text drawn on the tab face (icon-only tabs omit it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl NodeKind {
    /// Insertable node kinds in palette order.
    pub const TYPE_NAMES: &'static [&'static str] = &[
        "frame",
        "row",
        "column",
        "spacer",
        "label",
        "image",
        "rotimage",
        "button",
        "checkbox",
        "toggle",
        "slider",
        "text_input",
        "scroll",
        "list",
        "tab_bar",
        "slot",
        "slot_grid",
        "gauge",
        "badge",
        "alert",
        "hook",
        "canvas",
        "viewport",
        "tooltip",
    ];

    /// A document-ready default of the named node kind.
    pub fn default_for(type_name: &str) -> Option<Self> {
        Some(match type_name {
            "frame" => NodeKind::Frame,
            "row" => NodeKind::Row,
            "column" => NodeKind::Column,
            "spacer" => NodeKind::Spacer,
            "label" => NodeKind::Label {
                text: Some("Label".into()),
                wrap: false,
                scale: 1,
                small: false,
                max_lines: None,
            },
            "image" => NodeKind::Image {
                image: "image.png".into(),
                fit: Default::default(),
                frames: None,
                fps: None,
                interactive: false,
            },
            "rotimage" => NodeKind::Rotimage {
                image: "image.png".into(),
                pivot: None,
            },
            "button" => NodeKind::Button {
                text: Some("BUTTON".into()),
                icon: None,
                image: None,
                frames: None,
                fps: None,
            },
            "checkbox" => NodeKind::Checkbox,
            "toggle" => NodeKind::Toggle { icon: None },
            "slider" => NodeKind::Slider {
                min: 0.0,
                max: 100.0,
                step: None,
            },
            "text_input" => NodeKind::TextInput {
                placeholder: None,
                max_chars: 64,
                masked: false,
            },
            "scroll" => NodeKind::Scroll {
                axis: ScrollAxis::Vertical,
            },
            "list" => NodeKind::List { cols: 1 },
            "slot" => NodeKind::Slot {
                role: "storage".into(),
                accepts: Vec::new(),
                take_only: false,
            },
            "slot_grid" => NodeKind::SlotGrid {
                role: "storage".into(),
                cols: 9,
                rows: 3,
                accepts: Vec::new(),
                take_only: false,
            },
            "gauge" => NodeKind::Gauge {
                mode: GaugeMode::GrowLr,
            },
            "badge" => NodeKind::Badge {
                text: Some("badge".into()),
            },
            "alert" => NodeKind::Alert {
                level: AlertLevel::Info,
                text: Some("Alert text".into()),
            },
            "tab_bar" => NodeKind::TabBar {
                tabs: vec![
                    TabSpec {
                        key: "one".into(),
                        icon: None,
                        label: Some("One".into()),
                    },
                    TabSpec {
                        key: "two".into(),
                        icon: None,
                        label: Some("Two".into()),
                    },
                ],
            },
            "hook" => NodeKind::Hook,
            "canvas" => NodeKind::Canvas { interactive: true },
            "viewport" => NodeKind::Viewport { interactive: true },
            "tooltip" => NodeKind::Tooltip { hover: None },
            _ => return None,
        })
    }

    /// Whether this node type may carry children.
    pub fn is_container(&self) -> bool {
        matches!(
            self,
            NodeKind::Frame
                | NodeKind::Row
                | NodeKind::Column
                | NodeKind::Scroll { .. }
                | NodeKind::List { .. }
                | NodeKind::Button { .. }
                | NodeKind::Tooltip { .. }
        )
    }

    /// Whether this node type emits events and therefore requires an `id`.
    pub fn needs_id(&self) -> bool {
        matches!(
            self,
            NodeKind::Button { .. }
                | NodeKind::Checkbox
                | NodeKind::Toggle { .. }
                | NodeKind::Slider { .. }
                | NodeKind::TextInput { .. }
                | NodeKind::Image {
                    interactive: true,
                    ..
                }
                | NodeKind::List { .. }
                | NodeKind::TabBar { .. }
                | NodeKind::Hook
                | NodeKind::Canvas { .. }
                | NodeKind::Viewport { .. }
        )
    }

    /// Whether this node reports canvas pointer, wheel and size events.
    pub fn is_surface(&self) -> bool {
        matches!(
            self,
            NodeKind::Canvas { interactive: true } | NodeKind::Viewport { interactive: true }
        )
    }

    /// The stable name of this node type (matches the serialized `type` tag).
    pub fn type_name(&self) -> &'static str {
        match self {
            NodeKind::Frame => "frame",
            NodeKind::Row => "row",
            NodeKind::Column => "column",
            NodeKind::Spacer => "spacer",
            NodeKind::Label { .. } => "label",
            NodeKind::Image { .. } => "image",
            NodeKind::Rotimage { .. } => "rotimage",
            NodeKind::Button { .. } => "button",
            NodeKind::Checkbox => "checkbox",
            NodeKind::Toggle { .. } => "toggle",
            NodeKind::Slider { .. } => "slider",
            NodeKind::TextInput { .. } => "text_input",
            NodeKind::Scroll { .. } => "scroll",
            NodeKind::List { .. } => "list",
            NodeKind::Slot { .. } => "slot",
            NodeKind::SlotGrid { .. } => "slot_grid",
            NodeKind::Gauge { .. } => "gauge",
            NodeKind::Badge { .. } => "badge",
            NodeKind::Alert { .. } => "alert",
            NodeKind::TabBar { .. } => "tab_bar",
            NodeKind::Hook => "hook",
            NodeKind::Canvas { .. } => "canvas",
            NodeKind::Viewport { .. } => "viewport",
            NodeKind::Tooltip { .. } => "tooltip",
        }
    }

    /// The fixed column count a `list` arranges its stamps in (1 = flow).
    pub fn list_cols(&self) -> u32 {
        match self {
            NodeKind::List { cols } => (*cols).max(1),
            _ => 1,
        }
    }
}

fn default_max_chars() -> usize {
    64
}

fn default_cols() -> u32 {
    1
}

fn default_text_scale() -> u32 {
    1
}

fn is_one(v: &u32) -> bool {
    *v == 1
}

/// How an `image` node maps its texture onto its rect.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    /// Stretch the whole texture over the rect.
    #[default]
    Stretch,
    /// Preserve aspect ratio while filling the rect, cropping the overflow.
    Cover,
    /// Repeat the texture at its natural (1x art) size.
    Tile,
    /// 9-slice with insets `[l, t, r, b]` (1x-art px).
    Slice([i32; 4]),
}

impl ImageFit {
    pub fn is_stretch(&self) -> bool {
        *self == ImageFit::Stretch
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollAxis {
    #[default]
    Vertical,
    Horizontal,
}

/// How a gauge clips against its 0..=1 bound fraction. The two modes are the
/// classic furnace fill directions, kept as data.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GaugeMode {
    /// Grows left→right with the fraction (smelt arrow).
    GrowLr,
    /// Depletes top→down: the bottom `frac` stays visible (burn flame).
    DepleteTd,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertLevel {
    Info,
    Warning,
    Success,
    Danger,
}

// ---- layout properties ------------------------------------------------------

/// A node's size along one axis, in logical pixels.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Size {
    /// Size to content (plus padding).
    #[default]
    Auto,
    /// Fixed logical px.
    Px(i32),
    /// Natural size plus a weighted share of the parent's free space.
    Grow(u32),
}

impl Serialize for Size {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Size::Auto => s.serialize_str("auto"),
            Size::Px(px) => s.serialize_i32(*px),
            Size::Grow(w) => {
                use serde::ser::SerializeMap;
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("grow", w)?;
                m.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Size {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Px(i32),
            Word(String),
            Grow { grow: u32 },
        }
        match Raw::deserialize(d)? {
            Raw::Px(px) => Ok(Size::Px(px)),
            Raw::Grow { grow } => Ok(Size::Grow(grow)),
            Raw::Word(w) if w == "auto" => Ok(Size::Auto),
            Raw::Word(w) => Err(serde::de::Error::custom(format!(
                "size must be an integer, \"auto\", or {{\"grow\": n}}; got \"{w}\""
            ))),
        }
    }
}

/// Flow direction of a container's children.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    Row,
    #[default]
    Column,
}

/// Cross-axis placement of a child within its flow line.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

/// Main-axis distribution of children within leftover space.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

/// One edge of screen/parent-relative anchoring.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorEdge {
    Start,
    #[default]
    Center,
    End,
}

/// Where the root node sits on the screen (logical px space). Menus centre;
/// the hotbar HUD anchors `v: end`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    #[serde(default)]
    pub h: AnchorEdge,
    #[serde(default)]
    pub v: AnchorEdge,
}

/// Absolute placement inside the parent frame's padded rect — the escape hatch
/// for decorative overlap; `abs` children leave the flow.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AbsPos {
    pub x: i32,
    pub y: i32,
}

/// A node's layout inputs. All lengths are logical px integers; physical px =
/// logical × the host's integer gui scale, so the 1x pixel grid survives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutProps {
    pub w: Size,
    pub h: Size,
    /// Interior padding `[l, t, r, b]`.
    pub pad: [i32; 4],
    /// Exterior margin `[l, t, r, b]`.
    pub margin: [i32; 4],
    /// Gap between consecutive flow children.
    pub gap: i32,
    /// Flow direction (used by `frame`/`scroll`; `row`/`column` fix their own).
    pub dir: Dir,
    /// Cross-axis placement of children. `None` = the node type's default:
    /// `scroll` stretches its content (a scrolling list should fill the
    /// viewport width), everything else starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    pub justify: Justify,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_w: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_h: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_w: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_h: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abs: Option<AbsPos>,
    /// Root-only: where the tree sits on screen. Ignored on non-root nodes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    /// An `abs` frame placed against a named widget instead of its parent:
    /// below the widget's solved rect with left edges aligned, flipped above
    /// it or shifted left when it would leave the viewport — a popup under
    /// the button that opened it, at every size. Inside a list template the
    /// widget is the same stamp's; otherwise it is the instance last pressed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_to: Option<String>,
}

impl Default for LayoutProps {
    fn default() -> Self {
        LayoutProps {
            w: Size::Auto,
            h: Size::Auto,
            pad: [0; 4],
            margin: [0; 4],
            gap: 0,
            dir: Dir::Column,
            align: None,
            justify: Justify::Start,
            min_w: None,
            min_h: None,
            max_w: None,
            max_h: None,
            abs: None,
            anchor: None,
            anchor_to: None,
        }
    }
}

impl LayoutProps {
    pub fn is_default(&self) -> bool {
        *self == LayoutProps::default()
    }
}

// ---- bindings ---------------------------------------------------------------

/// State-key bindings: each field names a key in the host-supplied `UiState`
/// (inside a list template, keys resolve against the item map first). Absent
/// keys fall back to the node's static properties.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bindings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Slider/gauge/rotimage fraction-or-angle; checkbox/toggle on-state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible: Option<String>,
    /// List content: a `UiValue::List` of item maps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<String>,
    /// List selection index (`I32`; −1 = none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    /// Image-name override for `image`/`rotimage` nodes (`Str`) — per-row
    /// icons in list templates. Also overrides the face of an image-backed
    /// `button`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Sprite-sheet frame index (numeric; truncates, clamps into the sheet)
    /// for `image`/image-backed `button` nodes with `frames`. Authoritative
    /// over `fps` — the host drives the frame directly (a smelting progress
    /// flame, a charge meter). Presentation-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    /// Multiply colour for a `gauge`'s FULL face (`I32`, packed `0xRRGGBB`) —
    /// the one way a document draws something whose colour is content rather
    /// than theme, a bar that fills with the colour of what it is measuring.
    /// Absent = the theme's own colours, unmodified.
    ///
    /// It reaches the gauge's full face and NOTHING else: on any other node,
    /// or on a gauge's empty face, it is silently ignored. Widen it by
    /// threading `inst.tint` through the paint walk, not by binding and hoping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<String>,
    /// `hook` nodes only: a `Str` key naming a game ITEM (registry name) the
    /// host draws into the hook's rect, scaled to it — the generic "show an
    /// item here" view (a machine's enlarged subject, a preview). The document
    /// stays item-agnostic: what shows is entirely the published value, and an
    /// empty string shows nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// Bound MINIMUM WIDTH (logical px): a numeric key that replaces the
    /// node's authored `layout.min_w` this frame, so a box whose CONTENT the
    /// document cannot measure still reserves the room it needs.
    ///
    /// This is the answer for host-drawn `hook` content, which the layout
    /// engine sees only as an empty rect: the host publishes how wide its
    /// drawing will be and the ancestors grow around it (a recipe tooltip
    /// widening for a five-ingredient recipe). Without it, a hook's only
    /// natural width is the authored minimum, so growing for one recipe means
    /// widening EVERY tooltip forever. Values below the authored minimum are
    /// ignored — the author's floor stands, the binding may only raise it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_w: Option<String>,
    /// Bound absolute position (logical px from the parent's content rect,
    /// like `layout.abs`), for nodes that DECLARE `layout.abs`: the host moves
    /// the widget by publishing `I32` values (a slot whose place depends on
    /// what a machine holds). The authored `abs` is the resting position;
    /// either axis may bind alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abs_x: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abs_y: Option<String>,
    /// `slot`/`slot_grid` nodes with authored `accepts` only: an `I32` key
    /// whose value NARROWS the authored filters at runtime — bit `i` set =
    /// `accepts[i]` active, absent = all active, `0` = the slot admits
    /// nothing. Machine state made admission (a socket cell that is locked,
    /// occupied, or absent for the current tool), enforced by the HOST on
    /// clicks, drags, shift-routing and their predictions alike; the
    /// document only names the key. On a grid, every cell reads the same
    /// key. Takes are never gated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepts: Option<String>,
    /// `label`, `badge` and `text_input` nodes: a `Str` key naming a THEME
    /// PALETTE entry whose colour IS state this frame — a label's text, a
    /// badge's face (a severity chip), an input's frame and text (an invalid
    /// number). Empty resolves to the node's own style; a disabled node keeps
    /// its disabled face.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub palette: Option<String>,
    /// `canvas` nodes: a `Str` key naming the host-kept scene to paint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// `button`/`toggle` nodes: a `Str` key overriding the icon name this
    /// frame (play ↔ pause); empty = the authored icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Label text alpha (0–1), for transient notices without changing layout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_opacity: Option<String>,
}

impl Bindings {
    pub fn is_empty(&self) -> bool {
        *self == Bindings::default()
    }
}

// ---- document API -----------------------------------------------------------

#[derive(Debug)]
pub struct DocError(pub String);

impl std::fmt::Display for DocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for DocError {}

impl Document {
    pub fn from_json(s: &str) -> Result<Document, DocError> {
        let doc: Document = serde_json::from_str(s).map_err(|e| DocError(e.to_string()))?;
        if doc.format != FORMAT_VERSION {
            return Err(DocError(format!(
                "unsupported document format {} (this runtime reads {FORMAT_VERSION})",
                doc.format
            )));
        }
        Ok(doc)
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("document trees always serialize")
    }

    /// Every `(role, count)` the document declares, in document order —
    /// `slot` contributes 1, `slot_grid` contributes `cols × rows`. Cell order
    /// within a grid is row-major by construction, so in-role index maps
    /// directly to the host's row-major slot index.
    pub fn role_slots(&self) -> Vec<(String, usize)> {
        let mut out: Vec<(String, usize)> = Vec::new();
        let mut add = |role: &str, n: usize| match out.iter_mut().find(|(r, _)| r == role) {
            Some((_, c)) => *c += n,
            None => out.push((role.to_owned(), n)),
        };
        self.root.visit(&mut |node| match &node.kind {
            NodeKind::Slot { role, .. } => add(role, 1),
            NodeKind::SlotGrid {
                role, cols, rows, ..
            } => add(role, (*cols as usize) * (*rows as usize)),
            _ => {}
        });
        out
    }

    /// Every slot cell's host-interpreted semantics in document order, one
    /// entry PER CELL (a grid repeats its semantics for each cell). Parallel
    /// to the cell order behind [`Self::role_slots`], so a host can zip
    /// in-role indices against these.
    pub fn slot_semantics(&self) -> Vec<SlotSemantics> {
        let mut out = Vec::new();
        self.root.visit(&mut |node| match &node.kind {
            NodeKind::Slot {
                role,
                accepts,
                take_only,
            } => out.push(SlotSemantics {
                role: role.clone(),
                accepts: accepts.clone(),
                take_only: *take_only,
                accepts_bind: node.bind.accepts.clone(),
            }),
            NodeKind::SlotGrid {
                role,
                cols,
                rows,
                accepts,
                take_only,
            } => {
                for _ in 0..(*cols as usize) * (*rows as usize) {
                    out.push(SlotSemantics {
                        role: role.clone(),
                        accepts: accepts.clone(),
                        take_only: *take_only,
                        accepts_bind: node.bind.accepts.clone(),
                    });
                }
            }
            _ => {}
        });
        out
    }
}

/// One slot cell's host-interpreted semantics, as declared on its `slot` /
/// `slot_grid` node (see [`NodeKind::Slot`] — the document runtime carries
/// these verbatim; only the host gives them meaning).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlotSemantics {
    pub role: String,
    pub accepts: Vec<Accept>,
    pub take_only: bool,
    /// The node's `bind.accepts` key (see [`Bindings::accepts`]) — the host
    /// resolves it into the slot's runtime filter mask.
    pub accepts_bind: Option<String>,
}

/// One entry of a slot's `accepts` list: an item GROUP, named either way the
/// content layer has of naming one.
///
/// `"fuel"` names a group by item TAG — a closed vocabulary a row opts into.
/// `{"data": "forge:metal"}` names one by row-DATA key, which is the SAME
/// statement a consuming system already makes about the items it understands:
/// a mod enumerating `forge:metal` to build its whitelist and a slot admitting
/// `forge:metal` are then one fact, not two kept in step by hand. It also
/// reaches rows the naming pack does not own — a `patch` row may attach a data
/// key to anyone's item, and may not attach a tag.
///
/// Like the rest of a slot's semantics the document runtime carries both forms
/// verbatim and gives neither meaning; the host resolves them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Accept {
    /// An item-tag name (bare engine tag or `mod_id:name`).
    Tag(String),
    /// Any item whose row carries this namespaced `data` key.
    Data { data: String },
}

impl Node {
    /// Depth-first pre-order visit of this node and all descendants.
    pub fn visit(&self, f: &mut impl FnMut(&Node)) {
        f(self);
        for c in &self.children {
            c.visit(f);
        }
    }

    /// A leaf node of `kind` with everything else defaulted.
    pub fn leaf(kind: NodeKind) -> Node {
        Node {
            id: None,
            kind,
            layout: LayoutProps::default(),
            compact_layout: None,
            style: None,
            overlay: false,
            ellipsis_tip: false,
            bind: Bindings::default(),
            children: Vec::new(),
        }
    }

    /// The layout this node arranges by in the given form.
    pub fn layout_for(&self, compact: bool) -> &LayoutProps {
        if compact {
            if let Some(l) = &self.compact_layout {
                return l;
            }
        }
        &self.layout
    }

    /// Whether this instance uses child-flow layout. Buttons are the one
    /// dual-form widget: a text/icon-only button stays a leaf with its exact
    /// historical natural size, while a button with children becomes a
    /// themed layout container for compound rows.
    pub fn lays_out_children(&self) -> bool {
        match self.kind {
            NodeKind::Button { .. } => !self.children.is_empty(),
            _ => self.kind.is_container(),
        }
    }

    /// The effective flow direction for this node's children.
    pub fn flow_dir(&self) -> Dir {
        self.flow_dir_of(&self.layout)
    }

    /// [`Self::flow_dir`] against an explicit layout (the solver passes the
    /// instance's active normal/compact layout).
    pub fn flow_dir_of(&self, layout: &LayoutProps) -> Dir {
        match self.kind {
            NodeKind::Row => Dir::Row,
            NodeKind::Column => Dir::Column,
            _ => layout.dir,
        }
    }

    /// The effective cross-axis alignment of this node's children.
    pub fn effective_align(&self) -> Align {
        self.effective_align_of(&self.layout)
    }

    /// [`Self::effective_align`] against an explicit layout.
    pub fn effective_align_of(&self, layout: &LayoutProps) -> Align {
        layout.align.unwrap_or(match self.kind {
            NodeKind::Scroll { .. } => Align::Stretch,
            _ => Align::Start,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_doc() -> &'static str {
        r#"{
            "format": 1,
            "kind": "petramond:furnace",
            "class": "container",
            "root": {
                "type": "frame", "style": "panel.large",
                "layout": { "pad": [8,8,8,8], "gap": 4, "anchor": { "h": "center", "v": "center" } },
                "children": [
                    { "type": "label", "text": "Furnace", "style": "label.title" },
                    { "type": "row", "layout": { "gap": 8, "align": "center" }, "children": [
                        { "type": "slot", "role": "furnace_input" },
                        { "type": "gauge", "id": "arrow", "mode": "grow_lr", "style": "gauge.arrow",
                          "bind": { "value": "cook01" }, "layout": { "w": 24, "h": 17 } },
                        { "type": "slot", "role": "furnace_output" }
                    ] },
                    { "type": "slot_grid", "role": "player_inv", "cols": 9, "rows": 3 },
                    { "type": "slot_grid", "role": "hotbar", "cols": 9, "rows": 1 },
                    { "type": "spacer", "layout": { "h": { "grow": 1 } } }
                ]
            }
        }"#
    }

    #[test]
    fn parses_and_round_trips() {
        let doc = Document::from_json(sample_doc()).unwrap();
        assert_eq!(doc.kind, "petramond:furnace");
        assert_eq!(doc.class, DocClass::Container);
        let json = doc.to_json_pretty();
        let again = Document::from_json(&json).unwrap();
        assert_eq!(doc, again, "serialize → parse is lossless");
    }

    #[test]
    fn size_forms_parse() {
        let doc = Document::from_json(sample_doc()).unwrap();
        let row = &doc.root.children[1];
        let gauge = &row.children[1];
        assert_eq!(gauge.layout.w, Size::Px(24));
        let spacer = doc.root.children.last().unwrap();
        assert_eq!(spacer.layout.h, Size::Grow(1));
        assert_eq!(spacer.layout.w, Size::Auto);
    }

    #[test]
    fn role_slots_accumulate_in_document_order() {
        let doc = Document::from_json(sample_doc()).unwrap();
        assert_eq!(
            doc.role_slots(),
            vec![
                ("furnace_input".to_owned(), 1),
                ("furnace_output".to_owned(), 1),
                ("player_inv".to_owned(), 27),
                ("hotbar".to_owned(), 9),
            ]
        );
    }

    #[test]
    fn flow_dir_fixed_by_row_column_kinds() {
        let doc = Document::from_json(sample_doc()).unwrap();
        assert_eq!(doc.root.flow_dir(), Dir::Column);
        assert_eq!(doc.root.children[1].flow_dir(), Dir::Row);
    }

    #[test]
    fn animation_fields_round_trip() {
        let json = r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column", "children": [
                { "type": "image", "image": "flame.png", "frames": [4, 1], "fps": 8.0,
                  "bind": { "frame": "flame_frame" } },
                { "type": "button", "id": "go", "image": "go.png", "frames": [2, 2] }
            ] }
        }"#;
        let doc = Document::from_json(json).unwrap();
        let again = Document::from_json(&doc.to_json_pretty()).unwrap();
        assert_eq!(
            doc, again,
            "frames/fps/bind.frame survive serialize → parse"
        );
        // Unset fields stay absent (no schema noise on plain nodes).
        let plain = Document::from_json(
            r#"{ "format": 1, "kind": "petramond:x", "class": "screen",
                 "root": { "type": "image", "image": "a.png" } }"#,
        )
        .unwrap();
        let out = plain.to_json_pretty();
        assert!(!out.contains("frames"), "{out}");
        assert!(!out.contains("fps"), "{out}");
    }

    #[test]
    fn wrong_format_version_is_rejected() {
        let json = r#"{ "format": 99, "kind": "petramond:x", "class": "screen",
                        "root": { "type": "frame" } }"#;
        assert!(Document::from_json(json).is_err());
    }

    #[test]
    fn bad_size_word_is_rejected() {
        let json = r#"{ "format": 1, "kind": "petramond:x", "class": "screen",
                        "root": { "type": "frame", "layout": { "w": "big" } } }"#;
        assert!(Document::from_json(json).is_err());
    }

    #[test]
    fn node_typos_are_rejected_at_every_level() {
        let root = r#"{ "format": 1, "kind": "petramond:x", "class": "screen",
                         "root": %s }"#;
        for node in [
            r#"{ "type": "frame", "layuot": {} }"#,
            r#"{ "type": "label", "tetx": "misspelled" }"#,
            r#"{ "type": "frame", "layout": { "magrin": [0,0,0,0] } }"#,
            r#"{ "type": "label", "bind": { "visibile": "state" } }"#,
            r#"{ "type": "frame", "children": [ { "type": "button", "txet": "Go" } ] }"#,
        ] {
            let json = root.replace("%s", node);
            assert!(
                Document::from_json(&json).is_err(),
                "accepted typo in {node}"
            );
        }
    }

    #[test]
    fn every_catalog_kind_has_a_matching_default() {
        for &name in NodeKind::TYPE_NAMES {
            let kind = NodeKind::default_for(name).expect("catalog kind has a default");
            assert_eq!(kind.type_name(), name);
        }
    }
}
