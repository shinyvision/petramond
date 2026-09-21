use super::SelectionTool;
use crate::game::world_tool::ToolContext;
use crate::game::GameInput;
use petramond::schematic::{SelectionFace, SelectionSurface, PLACEMENT_REACH};

#[derive(Default)]
pub(super) struct FaceEdit {
    surface: SelectionSurface,
    revision: Option<u64>,
    face: Option<SelectionFace>,
    drag: Option<Drag>,
}

struct Drag {
    mouse_axis: [f64; 2],
    distance: f64,
}

impl Drag {
    fn new(face: &SelectionFace, depth: f64, cam: &petramond_render::camera::Camera) -> Self {
        let right = cam.right();
        let up = right.cross(cam.forward());
        let x = f64::from(right[face.axis] * face.normal as f32);
        let y = f64::from(-up[face.axis] * face.normal as f32);
        let projected = x * x + y * y;
        let scale = depth.max(1.0) * 0.0025;
        // A face viewed head-on has no screen-space normal: use vertical dragging
        // rather than dividing by an almost zero projection and jumping blocks.
        let mouse_axis = if projected < 0.0625 {
            [0.0, -scale]
        } else {
            [x * scale / projected, y * scale / projected]
        };
        Self {
            mouse_axis,
            distance: 0.0,
        }
    }

    fn advance(&mut self, delta: (f32, f32)) -> i32 {
        self.distance +=
            f64::from(delta.0) * self.mouse_axis[0] + f64::from(delta.1) * self.mouse_axis[1];
        self.distance.round() as i32
    }
}

impl FaceEdit {
    pub(super) fn face(&self) -> Option<&SelectionFace> {
        self.face.as_ref()
    }

    pub(super) fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

impl SelectionTool {
    pub(in crate::game) fn cancel_extrusion(&mut self) -> bool {
        self.face_edit.drag = None;
        self.face_edit.face = None;
        self.selection.cancel_extrusion()
    }

    pub(super) fn extrude_input(&mut self, ctx: &mut ToolContext<'_>, input: &GameInput) {
        if !input.gameplay_enabled || input.place_clicked {
            self.cancel_extrusion();
            return;
        }
        let edit = &mut self.face_edit;
        if let Some(drag) = &mut edit.drag {
            let offset = drag.advance(input.look_delta);
            if let Err(message) = self.selection.extrude(offset) {
                *ctx.notice = message;
            }
            edit.face = self.selection.extrusion_face();
            if !input.break_held {
                self.selection.finish_extrusion();
                edit.drag = None;
            }
            return;
        }
        if edit.revision != Some(self.selection.revision()) {
            edit.surface = self.selection.surface();
            edit.revision = Some(self.selection.revision());
        }
        let hit = edit
            .surface
            .pick(ctx.cam.pos, ctx.cam.forward(), PLACEMENT_REACH);
        edit.face = hit.map(|(face, _)| face.clone());
        if input.attack_clicked {
            if let Some((face, depth)) = hit {
                self.selection.begin_extrusion(face.clone());
                edit.drag = Some(Drag::new(face, depth, ctx.cam));
            }
        }
    }
}
