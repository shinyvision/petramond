//! Held world tools: an item row names the tool it is while held
//! ([`ItemType::world_tool`](petramond_world::item::ItemType::world_tool)),
//! and the tool of that name takes the frame's clicks instead of the
//! ordinary break/place path.

use super::selection_tool::{SelectionTool, SELECTION_TOOL};
use super::{Game, GameInput};
use petramond::schematic::{Selection, SelectionFace};
use petramond::world::World;
use petramond_render::camera::Camera;

/// What a tool reads and reports while it handles a frame.
pub struct ToolContext<'a> {
    pub cam: &'a Camera,
    pub world: &'a World,
    /// A refusal to show the player; left alone when nothing went wrong.
    pub notice: &'a mut String,
}

/// What a tool asks the renderer's selection overlay to show.
#[derive(Default, Clone, Copy)]
pub struct ToolOverlay<'a> {
    pub selection: Option<&'a Selection>,
    /// The two corners of a box being defined.
    pub corners: Option<([i32; 3], [i32; 3])>,
    pub face: Option<&'a SelectionFace>,
}

pub trait WorldTool {
    /// One frame while held with nothing else claiming the clicks. The caller
    /// consumes the frame's clicks afterwards.
    fn input(&mut self, ctx: &mut ToolContext<'_>, input: &GameInput);

    fn overlay(&self) -> ToolOverlay<'_>;

    /// Drop every unfinished operation; whether there was one.
    fn cancel(&mut self) -> bool;

    /// Step the tool's setting by whole notches; whether anything changed.
    fn adjust(&mut self, steps: i32) -> bool;

    /// Undo the tool's last edit, an unfinished operation first.
    fn undo(&mut self);

    fn redo(&mut self);

    /// Whether look input is claimed by a drag this frame.
    fn holds_camera(&self) -> bool;

    /// The name of the current setting, announced when it changes.
    fn setting_label(&self) -> &'static str;
}

/// Every world tool by the name item rows use.
#[derive(Default)]
pub struct WorldTools {
    pub selection: SelectionTool,
}

impl WorldTools {
    pub fn get(&self, name: &str) -> Option<&dyn WorldTool> {
        match name {
            SELECTION_TOOL => Some(&self.selection),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut dyn WorldTool> {
        match name {
            SELECTION_TOOL => Some(&mut self.selection),
            _ => None,
        }
    }

    pub fn cancel_all(&mut self) -> bool {
        self.selection.cancel()
    }
}

impl Game {
    /// The world tool in hand, wherever the player may hold it: always in
    /// creative, and outside it once its row is no longer creative-only.
    pub fn held_world_tool(&self) -> Option<&'static str> {
        let stack = self.self_view.inventory.selected()?;
        let name = stack.item.world_tool()?;
        (self.creative_mode() || !stack.item.creative_only()).then_some(name)
    }

    /// The held tool while it owns the edit controls: a placement preview
    /// takes them over.
    pub(super) fn editing_tool(&mut self) -> Option<&mut dyn WorldTool> {
        if self.schematic_preview.is_up() {
            return None;
        }
        let name = self.held_world_tool()?;
        self.world_tools.get_mut(name)
    }

    /// The held tool's current setting, for the change notice.
    pub fn held_tool_setting(&self) -> Option<&'static str> {
        let tool = self.world_tools.get(self.held_world_tool()?)?;
        Some(tool.setting_label())
    }

    /// Step whatever is adjustable by whole notches: a preview's height
    /// (positive = up) first, else the held tool's setting (positive = next).
    /// `false` = nothing took the notches.
    pub fn adjust_tool(&mut self, steps: i32) -> bool {
        if self.raise_schematic_preview(steps.saturating_neg()) {
            return true;
        }
        let Some(name) = self.held_world_tool() else {
            return false;
        };
        match self.world_tools.get_mut(name) {
            Some(tool) => {
                tool.adjust(steps);
                true
            }
            None => false,
        }
    }

    /// Take down the placement preview and every unfinished tool operation;
    /// whether there was anything to take down.
    pub fn cancel_world_tools(&mut self) -> bool {
        self.cancel_pending_paste();
        self.schematic_library.set_previewed(None);
        self.world_tools.cancel_all() | self.schematic_preview.cancel()
    }

    /// What the selection overlay shows: the held tool's view, and in
    /// creative the selection even with the tool put away.
    pub fn tool_overlay(&self) -> Option<ToolOverlay<'_>> {
        match self.held_world_tool().and_then(|n| self.world_tools.get(n)) {
            Some(tool) => Some(tool.overlay()),
            None => self
                .creative_mode()
                .then(|| self.world_tools.selection.overlay()),
        }
    }

    pub(in crate::game) fn world_tool_holds_camera(&self) -> bool {
        !self.schematic_preview.is_up()
            && self
                .held_world_tool()
                .and_then(|name| self.world_tools.get(name))
                .is_some_and(|tool| tool.holds_camera())
    }

    /// Route the frame's clicks: a placement preview first, then the held
    /// world tool; with neither, the ordinary break/place path keeps them.
    pub(super) fn world_tool_input(&mut self, input: &mut GameInput) {
        self.poll_schematic_library();
        if self.schematic_preview.is_up() && !self.schematic_preview_active() {
            // A paste preview never outlives creative mode.
            self.cancel_world_tools();
        }
        let held = self.held_world_tool();
        if self.schematic_preview.is_up() {
            self.world_tools.cancel_all();
            self.schematic_preview_input(input);
        } else if let Some(tool) = held.and_then(|name| self.world_tools.get_mut(name)) {
            let mut ctx = ToolContext {
                cam: &self.cam,
                world: &self.replica,
                notice: &mut self.notice,
            };
            tool.input(&mut ctx, input);
        } else {
            self.world_tools.cancel_all();
            return;
        }
        input.break_held = false;
        input.attack_clicked = false;
        input.place_clicked = false;
        input.use_held = false;
        self.intent_use_held = false;
    }
}
