use crate::keys::{CREATE_WAYPOINT_GUI, EDIT_WAYPOINT_GUI, WAYPOINT_NAME};
use crate::*;

const WAYPOINTS_KEY: &str = "minimap:waypoint_list";
const LEGACY_WAYPOINTS_KEY: &str = "minimap:waypoints";

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
        let mut stored = client_storage_get_many(
            ClientStorageScope::World,
            vec![WAYPOINTS_KEY.into(), LEGACY_WAYPOINTS_KEY.into()],
        )
        .into_iter();
        let current = stored.next().flatten();
        let legacy = stored.next().flatten();
        self.waypoints = load_list(current.as_deref(), legacy.as_deref());
    }

    pub(crate) fn select_waypoint_at(&mut self, x: f32, y: f32) {
        if self.store.ephemeral {
            return;
        }
        let bpp = blocks_per_pixel(self.zoom);
        let half = FULL_SIZE as f32 * 0.5;
        let wx = self.pan[0] + f64::from((x - half) * bpp);
        let wz = self.pan[1] + f64::from((y - half) * bpp);
        let radius = 12.0 * bpp;
        let Some((index, _)) = self
            .waypoints
            .iter()
            .enumerate()
            .map(|(i, waypoint)| {
                let dx = (f64::from(waypoint.pos[0]) + 0.5 - wx) as f32;
                let dz = (f64::from(waypoint.pos[2]) + 0.5 - wz) as f32;
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
        client_gui_open(EDIT_WAYPOINT_GUI);
    }

    pub(crate) fn open_create(&mut self) {
        self.editor = Editor::Create;
        self.draft.clear();
        client_ui_state_set(WAYPOINT_NAME, GuiValue::Str(String::new()));
        client_gui_open(CREATE_WAYPOINT_GUI);
    }

    pub(crate) fn save_editor(&mut self) {
        let name = self.draft.trim().to_owned();
        if name.is_empty() || self.store.ephemeral {
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
        if self.store.ephemeral {
            return;
        }
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
        if let Err(why) = client_storage_set_many(
            ClientStorageScope::World,
            vec![(
                WAYPOINTS_KEY.into(),
                Some(encode_versioned(&WaypointList(self.waypoints.clone()))),
            )],
        ) {
            log(&format!("minimap: waypoints not saved: {why}"));
        }
    }
}

#[derive(Clone)]
struct WaypointList(Vec<Waypoint>);

impl KvRecord for WaypointList {
    const VERSION: u8 = 1;
    const OLDEST_VERSION: u8 = 0;

    fn encode(&self) -> Vec<u8> {
        encode_waypoints(&self.0)
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        Some(Self(decode_waypoints(bytes)))
    }

    fn upgrade(from: u8, bytes: &[u8]) -> Option<Vec<u8>> {
        (from == 0).then(|| bytes.to_vec())
    }
}

fn load_list(current: Option<&[u8]>, legacy: Option<&[u8]>) -> Vec<Waypoint> {
    match (current, legacy) {
        (Some(bytes), _) => match decode_versioned::<WaypointList>(bytes) {
            Ok(list) => list.0,
            Err(error) => {
                log(&format!(
                    "minimap: stored waypoints are unreadable ({error})"
                ));
                Vec::new()
            }
        },
        (None, Some(bytes)) => decode_waypoints(bytes),
        (None, None) => Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Waypoint> {
        vec![
            Waypoint {
                name: "Home".into(),
                pos: [10, 64, -3],
                color: [200, 40, 40],
            },
            Waypoint {
                name: "Mine".into(),
                pos: [-500, 12, 9000],
                color: [1, 2, 3],
            },
        ]
    }

    fn names(list: &[Waypoint]) -> Vec<(&str, [i32; 3], [u8; 3])> {
        list.iter()
            .map(|w| (w.name.as_str(), w.pos, w.color))
            .collect()
    }

    #[test]
    fn the_versioned_list_round_trips() {
        let bytes = encode_versioned(&WaypointList(sample()));
        assert_eq!(bytes[0], WaypointList::VERSION);
        assert_eq!(names(&load_list(Some(&bytes), None)), names(&sample()));
    }

    #[test]
    fn a_legacy_list_is_read_until_the_versioned_one_exists() {
        let legacy = encode_waypoints(&sample());
        assert_eq!(names(&load_list(None, Some(&legacy))), names(&sample()));
        let current = encode_versioned(&WaypointList(sample()[..1].to_vec()));
        assert_eq!(load_list(Some(&current), Some(&legacy)).len(), 1);
    }

    #[test]
    fn a_newer_builds_list_is_not_misread() {
        let host = mod_sdk::testing::MockHost::new();
        let _host_guard = host.install();
        let mut bytes = encode_versioned(&WaypointList(sample()));
        bytes[0] = WaypointList::VERSION + 1;
        assert!(load_list(Some(&bytes), None).is_empty());
        assert!(host.logs().iter().any(|line| line.contains("unreadable")));
    }
}
