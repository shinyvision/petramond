//! Personal waypoints: the persisted list, deterministic colors, and the
//! create/edit document flow (open, save, cancel, delete).

use crate::*;

const CREATE_KIND: &str = "minimap:create_waypoint";
const EDIT_KIND: &str = "minimap:edit_waypoint";
const WAYPOINT_NAME: &str = "minimap:waypoint_name";
const WAYPOINTS_KEY: &str = "minimap:waypoints";

#[derive(Clone)]
pub(crate) struct Waypoint {
    pub(crate) name: String,
    pub(crate) pos: [i32; 3],
    pub(crate) color: [u8; 3],
}

#[derive(Copy, Clone, Default)]
pub(crate) enum Editor {
    #[default]
    None,
    Create,
    Edit(usize),
}

impl Minimap {
    pub(crate) fn load_waypoints(&mut self) {
        if let Some(bytes) = client_storage_get_many(vec![WAYPOINTS_KEY.into()])
            .into_iter()
            .next()
            .flatten()
        {
            self.waypoints = decode_waypoints(&bytes);
        }
    }

    pub(crate) fn select_waypoint_at(&mut self, x: f32, y: f32) {
        let bpp = blocks_per_pixel(self.zoom);
        let half = FULL_SIZE as f32 * 0.5;
        let wx = self.pan[0] + (x - half) * bpp;
        let wz = self.pan[1] + (y - half) * bpp;
        // A steady ~12-canvas-pixel hit target at every zoom level.
        let radius = 12.0 * bpp;
        let Some((index, _)) = self
            .waypoints
            .iter()
            .enumerate()
            .map(|(i, waypoint)| {
                let dx = waypoint.pos[0] as f32 + 0.5 - wx;
                let dz = waypoint.pos[2] as f32 + 0.5 - wz;
                (i, dx * dx + dz * dz)
            })
            .filter(|(_, distance)| *distance <= radius * radius)
            .min_by(|a, b| a.1.total_cmp(&b.1))
        else {
            return;
        };
        self.editor = Editor::Edit(index);
        self.draft = self.waypoints[index].name.clone();
        client_ui_state_set(WAYPOINT_NAME, GuiValue::Str(self.draft.clone()));
        client_gui_open(EDIT_KIND);
    }

    pub(crate) fn open_create(&mut self) {
        self.editor = Editor::Create;
        self.draft.clear();
        client_ui_state_set(WAYPOINT_NAME, GuiValue::Str(String::new()));
        client_gui_open(CREATE_KIND);
    }

    pub(crate) fn save_editor(&mut self) {
        let name = self.draft.trim().to_owned();
        if name.is_empty() {
            return;
        }
        let return_to_map = matches!(self.editor, Editor::Edit(_));
        match self.editor {
            Editor::Create => {
                let roll = rng_u64("waypoint_color");
                let color = [
                    96 + (roll & 127) as u8,
                    96 + ((roll >> 8) & 127) as u8,
                    96 + ((roll >> 16) & 127) as u8,
                ];
                let waypoint = Waypoint {
                    name: name.to_owned(),
                    pos: [
                        self.player[0].floor() as i32,
                        self.player[1].floor() as i32,
                        self.player[2].floor() as i32,
                    ],
                    color,
                };
                self.waypoints.push(waypoint.clone());
                self.invalidate_waypoint_area(&waypoint.name, waypoint.pos, waypoint.color);
            }
            Editor::Edit(index) => {
                let Some(old) = self.waypoints.get(index).cloned() else {
                    return;
                };
                self.invalidate_waypoint_area(&old.name, old.pos, old.color);
                self.waypoints[index].name = name.to_owned();
                let renamed = self.waypoints[index].clone();
                self.invalidate_waypoint_area(&renamed.name, renamed.pos, renamed.color);
            }
            Editor::None => return,
        }
        self.persist_waypoints();
        self.editor = Editor::None;
        self.waypoint_revision = self.waypoint_revision.wrapping_add(1);
        if return_to_map {
            self.sync_full_canvas();
            client_canvas_open(FULL_CANVAS, [FULL_SIZE as u16, FULL_SIZE as u16]);
        } else {
            client_gui_close();
        }
    }

    pub(crate) fn cancel_editor(&mut self) {
        let return_to_map = matches!(self.editor, Editor::Edit(_));
        self.editor = Editor::None;
        if return_to_map {
            client_canvas_open(FULL_CANVAS, [FULL_SIZE as u16, FULL_SIZE as u16]);
        } else {
            client_gui_close();
        }
    }

    pub(crate) fn delete_editor(&mut self) {
        let Editor::Edit(index) = self.editor else {
            return;
        };
        if index < self.waypoints.len() {
            let removed = self.waypoints.remove(index);
            self.invalidate_waypoint_area(&removed.name, removed.pos, removed.color);
            self.persist_waypoints();
        }
        self.editor = Editor::None;
        self.waypoint_revision = self.waypoint_revision.wrapping_add(1);
        self.sync_full_canvas();
        client_canvas_open(FULL_CANVAS, [FULL_SIZE as u16, FULL_SIZE as u16]);
    }

    fn persist_waypoints(&self) {
        client_storage_set_many(vec![(
            WAYPOINTS_KEY.into(),
            encode_waypoints(&self.waypoints),
        )]);
    }
}

fn encode_waypoints(waypoints: &[Waypoint]) -> Vec<u8> {
    let mut w = ByteWriter::new();
    w.u32(waypoints.len() as u32);
    for waypoint in waypoints {
        w.i32x3(waypoint.pos);
        w.raw(&waypoint.color);
        w.blob(waypoint.name.as_bytes());
    }
    w.finish()
}

fn decode_waypoints(bytes: &[u8]) -> Vec<Waypoint> {
    let mut r = ByteReader::new(bytes);
    let Some(count) = r.u32() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for _ in 0..count.min(4096) {
        let Some(pos) = r.i32x3() else { break };
        let Some(color) = r.take(3) else { break };
        let Some(name) = r.blob() else { break };
        let Ok(name) = std::str::from_utf8(name) else {
            continue;
        };
        out.push(Waypoint {
            name: name.to_owned(),
            pos,
            color: [color[0], color[1], color[2]],
        });
    }
    out
}
