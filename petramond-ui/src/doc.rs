use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub format: u32,
    pub kind: String,
    pub class: DocClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_below_w: Option<i32>,
    #[serde(default, skip_serializing_if = "Dismiss::is_close")]
    pub dismiss: Dismiss,
    pub root: Node,
}

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
    pub fn compact_active(&self, viewport_w: i32) -> bool {
        self.compact_below_w.is_some_and(|w| viewport_w < w)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocClass {
    Screen,
    Container,
    Hud,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Node {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(flatten)]
    pub kind: NodeKind,
    #[serde(default, skip_serializing_if = "LayoutProps::is_default")]
    pub layout: LayoutProps,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_layout: Option<Box<LayoutProps>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overlay: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ellipsis_tip: bool,
    #[serde(default, skip_serializing_if = "Bindings::is_empty")]
    pub bind: Bindings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NodeKind {
    Frame,
    Row,
    Column,
    Spacer,
    Label {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        wrap: bool,
        #[serde(default = "default_text_scale", skip_serializing_if = "is_one")]
        scale: u32,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        small: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_lines: Option<u32>,
    },
    Image {
        image: String,
        #[serde(default, skip_serializing_if = "ImageFit::is_stretch")]
        fit: ImageFit,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frames: Option<[u32; 2]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<f32>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        interactive: bool,
    },
    Rotimage {
        image: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pivot: Option<[f32; 2]>,
    },
    Button {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frames: Option<[u32; 2]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<f32>,
    },
    Checkbox,
    Toggle {
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
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        masked: bool,
    },
    Scroll {
        #[serde(default)]
        axis: ScrollAxis,
    },
    List {
        #[serde(default = "default_cols", skip_serializing_if = "is_one")]
        cols: u32,
    },
    Slot {
        role: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        accepts: Vec<Accept>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        take_only: bool,
    },
    SlotGrid {
        role: String,
        cols: u32,
        rows: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        accepts: Vec<Accept>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        take_only: bool,
    },
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
    TabBar {
        tabs: Vec<TabSpec>,
    },
    Hook,
    Canvas {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        interactive: bool,
    },
    Viewport {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        interactive: bool,
    },
    Tooltip {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hover: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabSpec {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl NodeKind {
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
                | NodeKind::Scroll { .. }
        )
    }

    pub fn is_surface(&self) -> bool {
        matches!(
            self,
            NodeKind::Canvas { interactive: true } | NodeKind::Viewport { interactive: true }
        )
    }

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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    #[default]
    Stretch,
    Cover,
    Tile,
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GaugeMode {
    GrowLr,
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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Size {
    #[default]
    Auto,
    Px(i32),
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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    Row,
    #[default]
    Column,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorEdge {
    Start,
    #[default]
    Center,
    End,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    #[serde(default)]
    pub h: AnchorEdge,
    #[serde(default)]
    pub v: AnchorEdge,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AbsPos {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutProps {
    pub w: Size,
    pub h: Size,
    pub pad: [i32; 4],
    pub margin: [i32; 4],
    pub gap: i32,
    pub dir: Dir,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reserve_scrollbar: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
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
            reserve_scrollbar: false,
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bindings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// Bound minimum width (logical px). Replaces the node's authored `layout.min_w` this frame.
    ///
    /// Hook content is host-drawn, layout only sees an empty rect. Host publishes the width it
    /// needs and ancestors grow around it (recipe tooltip widening for a five-ingredient recipe).
    /// Without this, growing for one hook means widening every instance's authored minimum.
    ///
    /// Can only raise the authored minimum, not lower it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_w: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abs_x: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abs_y: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepts: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub palette: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_opacity: Option<String>,
}

impl Bindings {
    pub fn is_empty(&self) -> bool {
        *self == Bindings::default()
    }
}

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlotSemantics {
    pub role: String,
    pub accepts: Vec<Accept>,
    pub take_only: bool,
    pub accepts_bind: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Accept {
    Tag(String),
    Data { data: String },
}

impl Node {
    pub fn visit(&self, f: &mut impl FnMut(&Node)) {
        f(self);
        for c in &self.children {
            c.visit(f);
        }
    }

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

    pub fn layout_for(&self, compact: bool) -> &LayoutProps {
        if compact {
            if let Some(l) = &self.compact_layout {
                return l;
            }
        }
        &self.layout
    }

    pub fn lays_out_children(&self) -> bool {
        match self.kind {
            NodeKind::Button { .. } => !self.children.is_empty(),
            _ => self.kind.is_container(),
        }
    }

    pub fn flow_dir(&self) -> Dir {
        self.flow_dir_of(&self.layout)
    }

    pub fn flow_dir_of(&self, layout: &LayoutProps) -> Dir {
        match self.kind {
            NodeKind::Row => Dir::Row,
            NodeKind::Column => Dir::Column,
            _ => layout.dir,
        }
    }

    pub fn effective_align(&self) -> Align {
        self.effective_align_of(&self.layout)
    }

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
