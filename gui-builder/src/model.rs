//! Pure data model for a GUI project. No egui / no rendering here — this is the
//! serializable heart that round-trips to `.llgui` (project) and feeds the bake
//! (PNG + JSON manifest).
//!
//! Layers live in a tree: the project holds an ordered list of `Node`s (a
//! `Layer` or a `Group`, and groups nest arbitrarily). Order is z-order,
//! back-to-front. Positions/sizes are whole pixels (`i32`).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuiType {
    Chest,
    Inventory,
    CraftingTable,
    Furnace,
    Hotbar,
    Custom,
}

impl GuiType {
    pub const ALL: [GuiType; 6] = [
        GuiType::Chest,
        GuiType::Inventory,
        GuiType::CraftingTable,
        GuiType::Furnace,
        GuiType::Hotbar,
        GuiType::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GuiType::Chest => "Chest",
            GuiType::Inventory => "Inventory",
            GuiType::CraftingTable => "Crafting Table",
            GuiType::Furnace => "Furnace",
            GuiType::Hotbar => "Hotbar",
            GuiType::Custom => "Custom",
        }
    }

    pub fn base_size(self) -> (u32, u32) {
        match self {
            GuiType::Chest => (176, 166),
            GuiType::Inventory => (176, 166),
            GuiType::CraftingTable => (176, 166),
            GuiType::Furnace => (176, 166),
            GuiType::Hotbar => (182, 22),
            GuiType::Custom => (176, 166),
        }
    }

    pub fn aspect_locked(self) -> bool {
        !matches!(self, GuiType::Custom)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotRole {
    Generic,
    Storage,
    PlayerInv,
    Hotbar,
    CraftInput,
    CraftResult,
    FurnaceInput,
    FurnaceFuel,
    FurnaceOutput,
}

impl SlotRole {
    pub const ALL: [SlotRole; 9] = [
        SlotRole::Generic,
        SlotRole::Storage,
        SlotRole::PlayerInv,
        SlotRole::Hotbar,
        SlotRole::CraftInput,
        SlotRole::CraftResult,
        SlotRole::FurnaceInput,
        SlotRole::FurnaceFuel,
        SlotRole::FurnaceOutput,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SlotRole::Generic => "Generic",
            SlotRole::Storage => "Storage",
            SlotRole::PlayerInv => "Player Inv",
            SlotRole::Hotbar => "Hotbar",
            SlotRole::CraftInput => "Craft Input",
            SlotRole::CraftResult => "Craft Result",
            SlotRole::FurnaceInput => "Furnace Input",
            SlotRole::FurnaceFuel => "Furnace Fuel",
            SlotRole::FurnaceOutput => "Furnace Output",
        }
    }

    pub fn tint(self) -> [u8; 3] {
        match self {
            SlotRole::Generic => [120, 160, 220],
            SlotRole::Storage => [120, 200, 140],
            SlotRole::PlayerInv => [200, 180, 110],
            SlotRole::Hotbar => [220, 150, 90],
            SlotRole::CraftInput => [180, 130, 220],
            SlotRole::CraftResult => [220, 110, 140],
            SlotRole::FurnaceInput => [110, 200, 210],
            SlotRole::FurnaceFuel => [210, 110, 110],
            SlotRole::FurnaceOutput => [150, 200, 110],
        }
    }
}

/// A rectangle in canvas units, whole pixels (top-left origin, y down).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RectF {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RectF {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    pub cols: u32,
    pub rows: u32,
    pub pitch_x: i32,
    pub pitch_y: i32,
}

impl Default for Grid {
    fn default() -> Self {
        Self { cols: 1, rows: 1, pitch_x: 18, pitch_y: 18 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum AssetSpec {
    Builtin { key: String },
    File { path: PathBuf },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum LayerFit {
    Stretch,
    Tile,
    NineSlice { l: u32, r: u32, t: u32, b: u32 },
}

/// A predefined role a layer can be tagged with so the game can drive it
/// dynamically. Tagged layers are *excluded* from the static baked panel and
/// instead baked to their own sibling PNG (recorded in the manifest), because
/// the game renders them at runtime — e.g. clipping the furnace arrow by smelt
/// progress, or the flame by remaining fuel. A tag is unique per project: each
/// tag names exactly one layer, so applying it to another layer moves it there.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerTag {
    FurnaceArrow,
    FurnaceFlame,
}

impl LayerTag {
    pub const ALL: [LayerTag; 2] = [LayerTag::FurnaceArrow, LayerTag::FurnaceFlame];

    /// Full label for the tag dialog / tooltips.
    pub fn label(self) -> &'static str {
        match self {
            LayerTag::FurnaceArrow => "Furnace Arrow",
            LayerTag::FurnaceFlame => "Furnace Flame",
        }
    }

    /// Compact label for the layer-panel badge.
    pub fn short(self) -> &'static str {
        match self {
            LayerTag::FurnaceArrow => "arrow",
            LayerTag::FurnaceFlame => "flame",
        }
    }

    /// Stable key for the baked PNG filename + manifest (mirrors the serde repr,
    /// kept explicit so renaming the variant can't silently change file names).
    pub fn key(self) -> &'static str {
        match self {
            LayerTag::FurnaceArrow => "furnace_arrow",
            LayerTag::FurnaceFlame => "furnace_flame",
        }
    }

    /// Badge tint in the layer panel / tag dialog.
    pub fn badge_color(self) -> [u8; 3] {
        match self {
            LayerTag::FurnaceArrow => [90, 160, 235],
            LayerTag::FurnaceFlame => [228, 120, 64],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub asset: AssetSpec,
    pub rect: RectF,
    pub fit: LayerFit,
    pub opacity: f32,
    pub visible: bool,
    #[serde(default)]
    pub flip_h: bool,
    #[serde(default)]
    pub flip_v: bool,
    #[serde(default)]
    pub rotation: i32,
    /// Optional dynamic-overlay tag. `#[serde(default)]` keeps older `.llgui`
    /// files loading; skipped when `None` so untagged layers stay byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<LayerTag>,
}

/// A named, collapsible folder. Children may be layers or nested groups.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    #[serde(default)]
    pub collapsed: bool,
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum Node {
    Layer(Layer),
    Group(Group),
}

impl Node {
    pub fn id(&self) -> u64 {
        match self {
            Node::Layer(l) => l.id,
            Node::Group(g) => g.id,
        }
    }
}

pub struct FlatLayer<'a> {
    pub layer: &'a Layer,
    pub effective_visible: bool,
    /// Immediate parent group (None at top level).
    pub group: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Top,
    Group(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropTarget {
    pub container: Container,
    pub index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub id: u64,
    pub role: SlotRole,
    pub rect: RectF,
    pub grid: Grid,
    pub color: [u8; 4],
    pub paint_frame: bool,
    pub visible: bool,
}

impl Slot {
    pub fn cells(&self) -> Vec<RectF> {
        let mut out = Vec::with_capacity((self.grid.cols * self.grid.rows) as usize);
        for r in 0..self.grid.rows {
            for c in 0..self.grid.cols {
                out.push(RectF::new(
                    self.rect.x + c as i32 * self.grid.pitch_x,
                    self.rect.y + r as i32 * self.grid.pitch_y,
                    self.rect.w,
                    self.rect.h,
                ));
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Canvas {
    pub w: u32,
    pub h: u32,
}

/// Per-GUI hover highlight: the graphic drawn over a slot when it's hovered
/// in-game. The slot graphic sits *inside* the highlight, so it's drawn over the
/// slot rect inflated by `margin` (canvas px) on every side — i.e. always a bit
/// bigger than the slot. Baked to a separate PNG + recorded in the manifest, so
/// the game can draw it dynamically (it can't be composited into the static
/// panel because it only appears on hover).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hover {
    pub asset: AssetSpec,
    pub margin: i32,
    #[serde(default = "hover_default_fit")]
    pub fit: LayerFit,
    #[serde(default = "hover_default_opacity")]
    pub opacity: f32,
}

fn hover_default_fit() -> LayerFit {
    LayerFit::NineSlice { l: 4, r: 4, t: 4, b: 4 }
}

fn hover_default_opacity() -> f32 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    #[serde(default = "default_version")]
    pub version: u32,
    pub gui_type: GuiType,
    pub scale: u32,
    pub canvas: Canvas,
    pub nodes: Vec<Node>,
    pub slots: Vec<Slot>,
    /// Optional hover highlight. `#[serde(default)]` so older `.llgui` files
    /// (saved before this field existed) still load — `hover` is simply `None`.
    /// Skipped when `None` so files without a highlight stay byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover: Option<Hover>,
}

fn default_version() -> u32 {
    2
}

// ---- recursive tree helpers (free functions over &[Node]) -----------------

fn find_layer<'a>(nodes: &'a [Node], id: u64) -> Option<&'a Layer> {
    for n in nodes {
        match n {
            Node::Layer(l) if l.id == id => return Some(l),
            Node::Group(g) => {
                if let Some(r) = find_layer(&g.children, id) {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_layer_mut<'a>(nodes: &'a mut [Node], id: u64) -> Option<&'a mut Layer> {
    for n in nodes.iter_mut() {
        match n {
            Node::Layer(l) => {
                if l.id == id {
                    return Some(l);
                }
            }
            Node::Group(g) => {
                if let Some(r) = find_layer_mut(&mut g.children, id) {
                    return Some(r);
                }
            }
        }
    }
    None
}

fn find_tag(nodes: &[Node], tag: LayerTag) -> Option<u64> {
    for n in nodes {
        match n {
            Node::Layer(l) if l.tag == Some(tag) => return Some(l.id),
            Node::Group(g) => {
                if let Some(r) = find_tag(&g.children, tag) {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

fn clear_tag(nodes: &mut [Node], tag: LayerTag) {
    for n in nodes.iter_mut() {
        match n {
            Node::Layer(l) => {
                if l.tag == Some(tag) {
                    l.tag = None;
                }
            }
            Node::Group(g) => clear_tag(&mut g.children, tag),
        }
    }
}

fn find_group<'a>(nodes: &'a [Node], id: u64) -> Option<&'a Group> {
    for n in nodes {
        if let Node::Group(g) = n {
            if g.id == id {
                return Some(g);
            }
            if let Some(r) = find_group(&g.children, id) {
                return Some(r);
            }
        }
    }
    None
}

fn find_group_mut<'a>(nodes: &'a mut [Node], id: u64) -> Option<&'a mut Group> {
    for n in nodes.iter_mut() {
        if let Node::Group(g) = n {
            if g.id == id {
                return Some(g);
            }
            if let Some(r) = find_group_mut(&mut g.children, id) {
                return Some(r);
            }
        }
    }
    None
}

fn remove_node(nodes: &mut Vec<Node>, id: u64) -> bool {
    if let Some(pos) = nodes.iter().position(|n| n.id() == id) {
        nodes.remove(pos);
        return true;
    }
    for n in nodes.iter_mut() {
        if let Node::Group(g) = n {
            if remove_node(&mut g.children, id) {
                return true;
            }
        }
    }
    false
}

fn extract_node(nodes: &mut Vec<Node>, container: Container, id: u64) -> Option<(Container, usize, Node)> {
    if let Some(pos) = nodes.iter().position(|n| n.id() == id) {
        return Some((container, pos, nodes.remove(pos)));
    }
    for n in nodes.iter_mut() {
        if let Node::Group(g) = n {
            let gid = g.id;
            if let Some(r) = extract_node(&mut g.children, Container::Group(gid), id) {
                return Some(r);
            }
        }
    }
    None
}

fn reassign_ids(node: &mut Node, next: &mut u64) {
    match node {
        Node::Layer(l) => {
            l.id = *next;
            *next += 1;
            // A tag names exactly one layer, so a duplicate can't inherit it.
            l.tag = None;
        }
        Node::Group(g) => {
            g.id = *next;
            *next += 1;
            for c in &mut g.children {
                reassign_ids(c, next);
            }
        }
    }
}

fn dup_in(nodes: &mut Vec<Node>, id: u64, first: u64) -> Option<(u64, u64, bool)> {
    if let Some(pos) = nodes.iter().position(|n| n.id() == id) {
        let mut clone = nodes[pos].clone();
        let mut next = first;
        reassign_ids(&mut clone, &mut next);
        let is_group = matches!(clone, Node::Group(_));
        match &mut clone {
            Node::Layer(l) => l.name = format!("{} copy", l.name),
            Node::Group(g) => g.name = format!("{} copy", g.name),
        }
        nodes.insert(pos + 1, clone);
        return Some((first, next - first, is_group));
    }
    for n in nodes.iter_mut() {
        if let Node::Group(g) = n {
            if let Some(r) = dup_in(&mut g.children, id, first) {
                return Some(r);
            }
        }
    }
    None
}

fn ungroup_in(nodes: &mut Vec<Node>, gid: u64) -> bool {
    if let Some(pos) = nodes.iter().position(|n| matches!(n, Node::Group(g) if g.id == gid)) {
        if let Node::Group(g) = nodes.remove(pos) {
            for (i, c) in g.children.into_iter().enumerate() {
                nodes.insert(pos + i, c);
            }
        }
        return true;
    }
    for n in nodes.iter_mut() {
        if let Node::Group(g) = n {
            if ungroup_in(&mut g.children, gid) {
                return true;
            }
        }
    }
    false
}

fn collect_layer_rects(nodes: &[Node], out: &mut Vec<RectF>) {
    for n in nodes {
        match n {
            Node::Layer(l) => out.push(l.rect),
            Node::Group(g) => collect_layer_rects(&g.children, out),
        }
    }
}

fn collect_layer_id_rects(nodes: &[Node], out: &mut Vec<(u64, RectF)>) {
    for n in nodes {
        match n {
            Node::Layer(l) => out.push((l.id, l.rect)),
            Node::Group(g) => collect_layer_id_rects(&g.children, out),
        }
    }
}

fn collect_ids(nodes: &[Node], out: &mut Vec<u64>) {
    for n in nodes {
        out.push(n.id());
        if let Node::Group(g) = n {
            collect_ids(&g.children, out);
        }
    }
}

fn max_id_in(nodes: &[Node]) -> u64 {
    let mut m = 0;
    for n in nodes {
        m = m.max(n.id());
        if let Node::Group(g) = n {
            m = m.max(max_id_in(&g.children));
        }
    }
    m
}

impl Project {
    pub fn new(gui_type: GuiType) -> Self {
        let (bw, bh) = gui_type.base_size();
        let bg = Layer {
            id: 1,
            name: "Background".to_string(),
            asset: AssetSpec::Builtin { key: "panel".to_string() },
            rect: RectF::new(0, 0, bw as i32, bh as i32),
            fit: LayerFit::NineSlice { l: 8, r: 8, t: 8, b: 8 },
            opacity: 1.0,
            visible: true,
            flip_h: false,
            flip_v: false,
            rotation: 0,
            tag: None,
        };
        Self {
            version: 2,
            gui_type,
            scale: 1,
            canvas: Canvas { w: bw, h: bh },
            nodes: vec![Node::Layer(bg)],
            slots: Vec::new(),
            hover: None,
        }
    }

    pub fn max_id(&self) -> u64 {
        let nodes = max_id_in(&self.nodes);
        let slots = self.slots.iter().map(|s| s.id).max().unwrap_or(0);
        nodes.max(slots)
    }

    pub fn resize_to_type_scale(&mut self) {
        if self.gui_type.aspect_locked() {
            let (bw, bh) = self.gui_type.base_size();
            let s = self.scale.max(1);
            self.canvas = Canvas { w: bw * s, h: bh * s };
        }
    }

    // ---- lookups ---------------------------------------------------------

    pub fn layer(&self, id: u64) -> Option<&Layer> {
        find_layer(&self.nodes, id)
    }

    pub fn layer_mut(&mut self, id: u64) -> Option<&mut Layer> {
        find_layer_mut(&mut self.nodes, id)
    }

    pub fn group(&self, id: u64) -> Option<&Group> {
        find_group(&self.nodes, id)
    }

    pub fn group_mut(&mut self, id: u64) -> Option<&mut Group> {
        find_group_mut(&mut self.nodes, id)
    }

    pub fn flat_layers(&self) -> Vec<FlatLayer<'_>> {
        fn walk<'a>(nodes: &'a [Node], vis: bool, group: Option<u64>, out: &mut Vec<FlatLayer<'a>>) {
            for n in nodes {
                match n {
                    Node::Layer(l) => out.push(FlatLayer { layer: l, effective_visible: vis && l.visible, group }),
                    Node::Group(g) => walk(&g.children, vis && g.visible, Some(g.id), out),
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.nodes, true, None, &mut out);
        out
    }

    /// Union of all descendant layer rects of a group (None if empty / missing).
    pub fn group_bounds(&self, gid: u64) -> Option<RectF> {
        let g = self.group(gid)?;
        let mut rects = Vec::new();
        collect_layer_rects(&g.children, &mut rects);
        if rects.is_empty() {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for r in rects {
            x0 = x0.min(r.x);
            y0 = y0.min(r.y);
            x1 = x1.max(r.x + r.w);
            y1 = y1.max(r.y + r.h);
        }
        Some(RectF::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// (id, rect) for every descendant layer of a group.
    pub fn group_child_rects(&self, gid: u64) -> Vec<(u64, RectF)> {
        let mut out = Vec::new();
        if let Some(g) = self.group(gid) {
            collect_layer_id_rects(&g.children, &mut out);
        }
        out
    }

    /// `gid` plus every id nested anywhere inside it.
    pub fn subtree_ids(&self, gid: u64) -> Vec<u64> {
        let mut out = vec![gid];
        if let Some(g) = self.group(gid) {
            collect_ids(&g.children, &mut out);
        }
        out
    }

    // ---- mutations -------------------------------------------------------

    pub fn add_group(&mut self, id: u64) {
        self.nodes.push(Node::Group(Group {
            id,
            name: "Group".to_string(),
            visible: true,
            collapsed: false,
            children: Vec::new(),
        }));
    }

    pub fn rename(&mut self, id: u64, name: String) {
        if let Some(g) = self.group_mut(id) {
            g.name = name;
        } else if let Some(l) = self.layer_mut(id) {
            l.name = name;
        }
    }

    /// Which layer currently holds `tag` (a tag names at most one layer).
    pub fn layer_with_tag(&self, tag: LayerTag) -> Option<u64> {
        find_tag(&self.nodes, tag)
    }

    /// Set (or clear) layer `id`'s tag. A tag is unique project-wide, so applying
    /// one another layer already holds *moves* it: the previous holder is cleared.
    pub fn set_layer_tag(&mut self, id: u64, tag: Option<LayerTag>) {
        if let Some(t) = tag {
            clear_tag(&mut self.nodes, t);
        }
        if let Some(l) = self.layer_mut(id) {
            l.tag = tag;
        }
    }

    pub fn delete(&mut self, id: u64) {
        remove_node(&mut self.nodes, id);
    }

    pub fn ungroup(&mut self, gid: u64) {
        ungroup_in(&mut self.nodes, gid);
    }

    /// Duplicate the layer/group `id`, assigning new ids from `first_new_id`.
    /// Returns (new top id, ids consumed, is_group).
    pub fn duplicate(&mut self, id: u64, first_new_id: u64) -> Option<(u64, u64, bool)> {
        dup_in(&mut self.nodes, id, first_new_id)
    }

    /// Move a tree item to a drop target. Refuses to drop a group into itself or
    /// any of its own descendants (which would create a cycle / lose data).
    pub fn move_item(&mut self, id: u64, target: DropTarget) {
        if let Container::Group(tc) = target.container {
            if tc == id {
                return;
            }
            if self.group(id).is_some() && self.subtree_ids(id).contains(&tc) {
                return;
            }
        }
        let Some((src_c, src_i, node)) = extract_node(&mut self.nodes, Container::Top, id) else {
            return;
        };
        let mut t = target;
        if src_c == t.container && src_i < t.index {
            t.index -= 1;
        }
        match t.container {
            Container::Top => {
                let i = t.index.min(self.nodes.len());
                self.nodes.insert(i, node);
            }
            Container::Group(gid) => {
                if let Some(g) = self.group_mut(gid) {
                    let i = t.index.min(g.children.len());
                    g.children.insert(i, node);
                } else {
                    self.nodes.push(node);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(id: u64, name: &str) -> Layer {
        Layer {
            id,
            name: name.to_string(),
            asset: AssetSpec::Builtin { key: "panel".to_string() },
            rect: RectF::new(0, 0, 16, 16),
            fit: LayerFit::Stretch,
            opacity: 1.0,
            visible: true,
            flip_h: false,
            flip_v: false,
            rotation: 0,
            tag: None,
        }
    }

    fn group(id: u64, children: Vec<Node>) -> Group {
        Group { id, name: format!("g{id}"), visible: true, collapsed: false, children }
    }

    fn proj(nodes: Vec<Node>) -> Project {
        Project { version: 2, gui_type: GuiType::Custom, scale: 1, canvas: Canvas { w: 100, h: 100 }, nodes, slots: vec![], hover: None }
    }

    #[test]
    fn older_llgui_without_hover_still_loads() {
        // A project saved before the hover field existed must still deserialize.
        let json = r#"{"version":2,"gui_type":"chest","scale":1,"canvas":{"w":176,"h":166},"nodes":[],"slots":[]}"#;
        let p: Project = serde_json::from_str(json).unwrap();
        assert!(p.hover.is_none());
    }

    #[test]
    fn hover_round_trips_through_llgui() {
        let mut p = proj(vec![]);
        p.hover = Some(Hover {
            asset: AssetSpec::Builtin { key: "highlight".to_string() },
            margin: 6,
            fit: LayerFit::NineSlice { l: 4, r: 4, t: 4, b: 4 },
            opacity: 0.8,
        });
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Project>(&s).unwrap(), p);
    }

    #[test]
    fn flat_layers_gates_on_nested_group_visibility() {
        // Outer group hidden => inner layer hidden even if its own group is visible.
        let inner = group(11, vec![Node::Layer(layer(2, "a"))]);
        let mut outer = group(10, vec![Node::Group(inner)]);
        outer.visible = false;
        let p = proj(vec![Node::Group(outer)]);
        let flat = p.flat_layers();
        assert_eq!(flat.len(), 1);
        assert!(!flat[0].effective_visible);
        assert_eq!(flat[0].group, Some(11));
    }

    #[test]
    fn move_group_into_group() {
        let p_inner = group(11, vec![Node::Layer(layer(2, "a"))]);
        let target = group(20, vec![]);
        let mut p = proj(vec![Node::Group(p_inner), Node::Group(target)]);
        p.move_item(11, DropTarget { container: Container::Group(20), index: 0 });
        assert_eq!(p.nodes.len(), 1); // only group 20 at top
        let g20 = p.group(20).unwrap();
        assert_eq!(g20.children.len(), 1);
        assert_eq!(g20.children[0].id(), 11);
        // The nested group's own child is still reachable.
        assert!(p.layer(2).is_some());
    }

    #[test]
    fn cannot_move_group_into_its_own_descendant() {
        let inner = group(11, vec![]);
        let outer = group(10, vec![Node::Group(inner)]);
        let mut p = proj(vec![Node::Group(outer)]);
        // Try to drop the outer group into its own child group -> rejected.
        p.move_item(10, DropTarget { container: Container::Group(11), index: 0 });
        // Tree unchanged: outer still at top, inner still inside outer.
        assert_eq!(p.nodes.len(), 1);
        assert_eq!(p.nodes[0].id(), 10);
        assert!(p.group(11).is_some());
    }

    #[test]
    fn duplicate_nested_group_assigns_fresh_ids_to_subtree() {
        let inner = group(11, vec![Node::Layer(layer(3, "a"))]);
        let outer = group(10, vec![Node::Layer(layer(2, "x")), Node::Group(inner)]);
        let mut p = proj(vec![Node::Group(outer)]);
        // outer(1) + x(1) + inner(1) + a(1) = 4 ids.
        let r = p.duplicate(10, 50);
        assert_eq!(r, Some((50, 4, true)));
        assert!(p.group(50).is_some());
        // The deep copy has its own nested group with a fresh id.
        let dup = p.group(50).unwrap();
        assert_eq!(dup.children.len(), 2);
    }

    #[test]
    fn ungroup_hoists_nested_children_in_place() {
        let inner = group(11, vec![Node::Layer(layer(3, "a"))]);
        let outer = group(10, vec![Node::Layer(layer(2, "x")), Node::Group(inner)]);
        let mut p = proj(vec![Node::Layer(layer(1, "top")), Node::Group(outer)]);
        p.ungroup(10);
        // outer dissolves: its layer + nested group land at the top level.
        let ids: Vec<u64> = p.nodes.iter().map(|n| n.id()).collect();
        assert_eq!(ids, vec![1, 2, 11]);
        assert!(p.group(11).is_some());
    }

    #[test]
    fn group_bounds_unions_all_descendants() {
        let mut a = layer(3, "a");
        a.rect = RectF::new(10, 10, 20, 20);
        let inner = group(11, vec![Node::Layer(a)]);
        let mut b = layer(2, "b");
        b.rect = RectF::new(40, 5, 10, 10);
        let outer = group(10, vec![Node::Layer(b), Node::Group(inner)]);
        let p = proj(vec![Node::Group(outer)]);
        assert_eq!(p.group_bounds(10), Some(RectF::new(10, 5, 40, 25)));
    }

    #[test]
    fn move_layer_out_of_nested_group_to_top() {
        let inner = group(11, vec![Node::Layer(layer(3, "a"))]);
        let outer = group(10, vec![Node::Group(inner)]);
        let mut p = proj(vec![Node::Group(outer)]);
        p.move_item(3, DropTarget { container: Container::Top, index: 0 });
        assert_eq!(p.nodes[0].id(), 3);
        assert!(p.group(11).unwrap().children.is_empty());
    }

    fn tagged(id: u64, name: &str, tag: LayerTag) -> Layer {
        let mut l = layer(id, name);
        l.tag = Some(tag);
        l
    }

    #[test]
    fn tag_is_unique_and_moves_between_layers() {
        let mut p = proj(vec![Node::Layer(tagged(2, "a", LayerTag::FurnaceArrow)), Node::Layer(layer(3, "b"))]);
        // Applying the same tag to b moves it off a.
        p.set_layer_tag(3, Some(LayerTag::FurnaceArrow));
        assert_eq!(p.layer(2).unwrap().tag, None);
        assert_eq!(p.layer(3).unwrap().tag, Some(LayerTag::FurnaceArrow));
        assert_eq!(p.layer_with_tag(LayerTag::FurnaceArrow), Some(3));
    }

    #[test]
    fn set_tag_none_clears_it() {
        let mut p = proj(vec![Node::Layer(tagged(2, "a", LayerTag::FurnaceFlame))]);
        p.set_layer_tag(2, None);
        assert_eq!(p.layer(2).unwrap().tag, None);
        assert_eq!(p.layer_with_tag(LayerTag::FurnaceFlame), None);
    }

    #[test]
    fn distinct_tags_coexist_on_different_layers() {
        let mut p = proj(vec![Node::Layer(layer(2, "a")), Node::Layer(layer(3, "b"))]);
        p.set_layer_tag(2, Some(LayerTag::FurnaceArrow));
        p.set_layer_tag(3, Some(LayerTag::FurnaceFlame));
        assert_eq!(p.layer(2).unwrap().tag, Some(LayerTag::FurnaceArrow));
        assert_eq!(p.layer(3).unwrap().tag, Some(LayerTag::FurnaceFlame));
    }

    #[test]
    fn duplicating_a_tagged_layer_drops_the_tag_on_the_copy() {
        let mut p = proj(vec![Node::Layer(tagged(2, "a", LayerTag::FurnaceArrow))]);
        let r = p.duplicate(2, 50);
        assert_eq!(r, Some((50, 1, false)));
        // Original keeps the tag; the unique tag still resolves to it, not the copy.
        assert_eq!(p.layer(2).unwrap().tag, Some(LayerTag::FurnaceArrow));
        assert_eq!(p.layer(50).unwrap().tag, None);
        assert_eq!(p.layer_with_tag(LayerTag::FurnaceArrow), Some(2));
    }

    #[test]
    fn applying_a_tag_held_deep_in_a_group_clears_the_nested_layer() {
        let inner = group(11, vec![Node::Layer(tagged(3, "deep", LayerTag::FurnaceFlame))]);
        let outer = group(10, vec![Node::Group(inner)]);
        let mut p = proj(vec![Node::Layer(layer(2, "top")), Node::Group(outer)]);
        p.set_layer_tag(2, Some(LayerTag::FurnaceFlame));
        assert_eq!(p.layer(3).unwrap().tag, None);
        assert_eq!(p.layer(2).unwrap().tag, Some(LayerTag::FurnaceFlame));
    }

    #[test]
    fn older_llgui_without_tag_field_loads_layer() {
        // A layer serialized before `tag` existed must still deserialize (tag=None).
        let json = r#"{"version":2,"gui_type":"furnace","scale":1,"canvas":{"w":176,"h":166},
            "nodes":[{"node":"layer","id":1,"name":"bg","asset":{"source":"builtin","key":"panel"},
            "rect":{"x":0,"y":0,"w":176,"h":166},"fit":{"mode":"stretch"},"opacity":1.0,"visible":true}],
            "slots":[]}"#;
        let p: Project = serde_json::from_str(json).unwrap();
        assert_eq!(p.layer(1).unwrap().tag, None);
    }

    #[test]
    fn tag_round_trips_and_untagged_layers_omit_the_field() {
        let mut p = proj(vec![Node::Layer(tagged(2, "a", LayerTag::FurnaceArrow)), Node::Layer(layer(3, "b"))]);
        p.version = 2;
        let s = serde_json::to_string(&p).unwrap();
        // Untagged layers don't write the field (skip_serializing_if).
        assert_eq!(s.matches("\"tag\"").count(), 1);
        assert_eq!(serde_json::from_str::<Project>(&s).unwrap(), p);
    }
}
