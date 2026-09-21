//! The region selection tool: two-corner boxes, single cells, and face
//! extrusion over a geometry-only [`Selection`].

mod extrude;

use super::world_tool::{ToolContext, ToolOverlay, WorldTool};
use super::GameInput;
use petramond::player::{Player, RayFilter};
use petramond::schematic::{Selection, PLACEMENT_REACH};

/// The [`world_tool`](petramond_world::item::ItemType::world_tool) name of
/// the tool that selects and captures regions.
pub const SELECTION_TOOL: &str = "schematic";

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    #[default]
    Region,
    Cell,
    Extrude,
}

impl SelectionMode {
    const ALL: [Self; 3] = [Self::Region, Self::Cell, Self::Extrude];
}

#[derive(Default)]
pub struct SelectionTool {
    pub selection: Selection,
    mode: SelectionMode,
    /// The first corner of a box being defined, and whether it removes.
    corner: Option<([i32; 3], bool)>,
    target: Option<[i32; 3]>,
    face_edit: extrude::FaceEdit,
}

impl SelectionTool {
    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    pub fn has_pending_corner(&self) -> bool {
        self.corner.is_some()
    }

    #[cfg(test)]
    pub fn pending_corner(&self) -> Option<[i32; 3]> {
        self.corner.map(|(corner, _)| corner)
    }

    #[cfg(test)]
    pub fn set_pending_corner(&mut self, corner: [i32; 3]) {
        self.corner = Some((corner, false));
    }

    /// Empty the selection as one undoable edit.
    pub fn clear(&mut self) {
        self.cancel();
        self.selection.clear();
    }

    fn click(&mut self, pos: [i32; 3], remove: bool, notice: &mut String) {
        let first = match (self.mode, self.corner) {
            (SelectionMode::Cell, _) => pos,
            (_, Some((first, pending))) if pending == remove => first,
            _ => {
                self.corner = Some((pos, remove));
                return;
            }
        };
        self.corner = None;
        *notice = self
            .selection
            .region(first, pos, remove)
            .err()
            .unwrap_or_default();
    }
}

impl WorldTool for SelectionTool {
    fn input(&mut self, ctx: &mut ToolContext<'_>, input: &GameInput) {
        if self.mode == SelectionMode::Extrude {
            self.corner = None;
            self.target = None;
            self.extrude_input(ctx, input);
            return;
        }
        self.cancel_extrusion();
        let (eye, forward) = (ctx.cam.pos, ctx.cam.forward());
        let hit = Player::raycast_filtered(
            eye,
            forward,
            PLACEMENT_REACH,
            RayFilter::Selectable,
            ctx.world,
        )
        .map(|(h, _)| h.block.to_array());
        // Removal picks the selection's own geometry, so selected air erases.
        let selected = self.selection.raycast(eye, forward, PLACEMENT_REACH);
        let removing = self.corner.is_some_and(|(_, remove)| remove);
        self.target = if removing { selected.or(hit) } else { hit };
        if !input.gameplay_enabled || !(input.attack_clicked || input.place_clicked) {
            return;
        }
        let remove = input.place_clicked;
        if let Some(pos) = remove.then_some(selected).flatten().or(hit) {
            self.click(pos, remove, ctx.notice);
        }
    }

    fn overlay(&self) -> ToolOverlay<'_> {
        ToolOverlay {
            selection: Some(&self.selection),
            corners: self.corner.map(|(corner, _)| corner).zip(self.target),
            face: self.face_edit.face(),
        }
    }

    fn cancel(&mut self) -> bool {
        self.target = None;
        self.cancel_extrusion() | self.corner.take().is_some()
    }

    fn adjust(&mut self, steps: i32) -> bool {
        let modes = SelectionMode::ALL;
        let current = modes.iter().position(|m| *m == self.mode).unwrap_or(0) as i32;
        let next = modes[(current + steps).rem_euclid(modes.len() as i32) as usize];
        if next == self.mode {
            return false;
        }
        self.cancel();
        self.mode = next;
        true
    }

    fn undo(&mut self) {
        if !self.cancel() {
            self.selection.undo();
        }
    }

    fn redo(&mut self) {
        self.cancel_extrusion();
        if self.selection.redo() {
            self.corner = None;
        }
    }

    fn holds_camera(&self) -> bool {
        self.mode == SelectionMode::Extrude && self.face_edit.dragging()
    }

    fn setting_label(&self) -> &'static str {
        match self.mode {
            SelectionMode::Region => "Region selection",
            SelectionMode::Cell => "Cell selection",
            SelectionMode::Extrude => "Extrude / Contract",
        }
    }
}
