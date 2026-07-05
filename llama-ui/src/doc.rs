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

/// One GUI document: its kind key (namespaced, e.g. `llama:furnace` or
/// `somemod:wheel`), its class, and the node tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub format: u32,
    /// The kind key the host registers/opens this document under.
    pub kind: String,
    pub class: DocClass,
    pub root: Node,
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Stable id within the document: the key for events, named rects, and
    /// per-widget ephemeral state. Required on event-bearing widgets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(flatten)]
    pub kind: NodeKind,
    #[serde(default, skip_serializing_if = "LayoutProps::is_default")]
    pub layout: LayoutProps,
    /// Theme part key (e.g. `button.danger`). `None` = the widget's default
    /// part for its type, or unskinned for plain containers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Bindings::is_empty")]
    pub bind: Bindings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

/// The node type plus its type-specific properties. Serialized internally
/// tagged as `"type"` so documents read naturally.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
    },
    /// An image beside the document (path relative to the document; a bound
    /// `image` key overrides the name per instance — list-row icons).
    Image {
        image: String,
        #[serde(default, skip_serializing_if = "ImageFit::is_stretch")]
        fit: ImageFit,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Theme part drawn as the button's icon (centred alone, or left of
        /// the label) — e.g. `icon.edit` for a pencil button.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
    },
    Checkbox,
    Toggle,
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
    },
    /// Clipping scroll region around its children.
    Scroll {
        #[serde(default)]
        axis: ScrollAxis,
    },
    /// Repeats its single template child once per item in `bind.items`.
    List,
    /// One host-mapped slot (item cell) of `role`.
    Slot {
        role: String,
    },
    /// A `cols`×`rows` grid of `role` slots, generated row-major — the in-role
    /// index ↔ cell order contract holds by construction.
    SlotGrid {
        role: String,
        cols: u32,
        rows: u32,
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
    /// Layout-reserved, host-drawn region (hearts, item previews). The host
    /// reads its solved rect from the frame output by id.
    Hook,
}

impl NodeKind {
    /// Whether this node type may carry children.
    pub fn is_container(&self) -> bool {
        matches!(
            self,
            NodeKind::Frame
                | NodeKind::Row
                | NodeKind::Column
                | NodeKind::Scroll { .. }
                | NodeKind::List
        )
    }

    /// Whether this node type emits events and therefore requires an `id`.
    pub fn needs_id(&self) -> bool {
        matches!(
            self,
            NodeKind::Button { .. }
                | NodeKind::Checkbox
                | NodeKind::Toggle
                | NodeKind::Slider { .. }
                | NodeKind::TextInput { .. }
                | NodeKind::List
                | NodeKind::Hook
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
            NodeKind::Toggle => "toggle",
            NodeKind::Slider { .. } => "slider",
            NodeKind::TextInput { .. } => "text_input",
            NodeKind::Scroll { .. } => "scroll",
            NodeKind::List => "list",
            NodeKind::Slot { .. } => "slot",
            NodeKind::SlotGrid { .. } => "slot_grid",
            NodeKind::Gauge { .. } => "gauge",
            NodeKind::Badge { .. } => "badge",
            NodeKind::Alert { .. } => "alert",
            NodeKind::Hook => "hook",
        }
    }
}

fn default_max_chars() -> usize {
    64
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
#[serde(default)]
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
#[serde(default)]
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
    /// icons in list templates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
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
            NodeKind::Slot { role } => add(role, 1),
            NodeKind::SlotGrid { role, cols, rows } => {
                add(role, (*cols as usize) * (*rows as usize))
            }
            _ => {}
        });
        out
    }
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
            style: None,
            bind: Bindings::default(),
            children: Vec::new(),
        }
    }

    /// The effective flow direction for this node's children.
    pub fn flow_dir(&self) -> Dir {
        match self.kind {
            NodeKind::Row => Dir::Row,
            NodeKind::Column => Dir::Column,
            _ => self.layout.dir,
        }
    }

    /// The effective cross-axis alignment of this node's children.
    pub fn effective_align(&self) -> Align {
        self.layout.align.unwrap_or(match self.kind {
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
            "kind": "llama:furnace",
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
        assert_eq!(doc.kind, "llama:furnace");
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
    fn wrong_format_version_is_rejected() {
        let json = r#"{ "format": 99, "kind": "llama:x", "class": "screen",
                        "root": { "type": "frame" } }"#;
        assert!(Document::from_json(json).is_err());
    }

    #[test]
    fn bad_size_word_is_rejected() {
        let json = r#"{ "format": 1, "kind": "llama:x", "class": "screen",
                        "root": { "type": "frame", "layout": { "w": "big" } } }"#;
        assert!(Document::from_json(json).is_err());
    }
}
