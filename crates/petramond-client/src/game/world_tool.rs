use super::selection_tool::{SelectionTool, SELECTION_TOOL};
use super::{Game, GameInput};
use petramond::schematic::{Selection, SelectionFace};
use petramond::world::ReplicaWorld;
use petramond_render::camera::Camera;

pub struct ToolContext<'a> {
    pub cam: &'a Camera,
    pub world: &'a ReplicaWorld,
    pub notice: &'a mut String,
}

#[derive(Default, Clone, Copy)]
pub struct ToolOverlay<'a> {
    pub selection: Option<&'a Selection>,
    pub corners: Option<([i32; 3], [i32; 3])>,
    pub face: Option<&'a SelectionFace>,
}

pub trait WorldTool {
    fn input(&mut self, ctx: &mut ToolContext<'_>, input: &GameInput);

    fn overlay(&self) -> ToolOverlay<'_>;

    fn cancel(&mut self) -> bool;

    fn adjust(&mut self, steps: i32) -> bool;

    fn undo(&mut self);

    fn redo(&mut self);

    fn holds_camera(&self) -> bool;

    fn setting_label(&self) -> &'static str;
}

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
    pub fn held_world_tool(&self) -> Option<&'static str> {
        let stack = self.replica.self_view.inventory.selected()?;
        let name = stack.item.world_tool()?;
        (self.creative_mode() || !stack.item.creative_only()).then_some(name)
    }

    pub(super) fn editing_tool(&mut self) -> Option<&mut dyn WorldTool> {
        if self.tools.preview.is_up() {
            return None;
        }
        let name = self.held_world_tool()?;
        self.tools.world.get_mut(name)
    }

    pub fn held_tool_setting(&self) -> Option<&'static str> {
        let tool = self.tools.world.get(self.held_world_tool()?)?;
        Some(tool.setting_label())
    }

    pub fn adjust_tool(&mut self, steps: i32) -> bool {
        if self.raise_schematic_preview(steps.saturating_neg()) {
            return true;
        }
        let Some(name) = self.held_world_tool() else {
            return false;
        };
        match self.tools.world.get_mut(name) {
            Some(tool) => {
                tool.adjust(steps);
                true
            }
            None => false,
        }
    }

    pub fn cancel_world_tools(&mut self) -> bool {
        self.cancel_pending_paste();
        self.tools.library.set_previewed(None);
        self.tools.world.cancel_all() | self.tools.preview.cancel()
    }

    pub fn tool_overlay(&self) -> Option<ToolOverlay<'_>> {
        match self.held_world_tool().and_then(|n| self.tools.world.get(n)) {
            Some(tool) => Some(tool.overlay()),
            None => self
                .creative_mode()
                .then(|| self.tools.world.selection.overlay()),
        }
    }

    pub(in crate::game) fn world_tool_holds_camera(&self) -> bool {
        !self.tools.preview.is_up()
            && self
                .held_world_tool()
                .and_then(|name| self.tools.world.get(name))
                .is_some_and(|tool| tool.holds_camera())
    }

    pub(super) fn world_tool_input(&mut self, input: &mut GameInput) {
        self.poll_schematic_library();
        if self.tools.preview.is_up() && !self.schematic_preview_active() {
            self.cancel_world_tools();
        }
        let held = self.held_world_tool();
        if self.tools.preview.is_up() {
            self.tools.world.cancel_all();
            self.schematic_preview_input(input);
        } else if let Some(tool) = held.and_then(|name| self.tools.world.get_mut(name)) {
            let mut ctx = ToolContext {
                cam: &self.local.cam,
                world: &self.replica.world,
                notice: &mut self.notice,
            };
            tool.input(&mut ctx, input);
        } else {
            self.tools.world.cancel_all();
            return;
        }
        input.break_held = false;
        input.attack_clicked = false;
        input.place_clicked = false;
        input.use_held = false;
        self.local.intent_use_held = false;
    }
}
