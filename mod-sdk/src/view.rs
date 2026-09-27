use mod_api::{ClientEntitiesNear, ClientEntityData, ClientViewStateData, EntityRef, PlayerId};

use crate::__rt::host_fn;

host_fn! {
    pub fn client_view_camera_set(
        pos: [f64; 3],
        yaw: f32,
        pitch: f32,
        roll: f32,
        fov_y: Option<f32>,
        anchor: Option<EntityRef>,
    ) => ClientViewCameraSet { pos, yaw, pitch, roll, fov_y, anchor }
}

host_fn! {
    pub fn client_view_camera_release() => ClientViewCameraRelease
}

host_fn! {
    pub fn client_view_chrome_set(
        hud: Option<bool>,
        hands: Option<bool>,
        crosshair: Option<bool>,
    ) => ClientViewChromeSet { hud, hands, crosshair }
}

host_fn! {
    pub fn client_view_perspective_set(third_person: Option<bool>)
        => ClientViewPerspectiveSet { third_person }
}

host_fn! {
    pub fn client_view_state() -> ClientViewStateData
        => ClientViewState => ClientViewState
}

host_fn! {
    pub fn client_env_set(params: Vec<(String, [f32; 4])>) => ClientEnvSet { params }
}

host_fn! {
    pub fn client_view_frame_set(size: Option<[u32; 2]>) => ClientViewFrameSet { size }
}

host_fn! {
    pub fn client_view_subject_set(player: Option<PlayerId>) => ClientViewSubjectSet { player }
}

host_fn! {
    pub fn client_entities(ids: Vec<EntityRef>, near: Option<ClientEntitiesNear>)
        -> Vec<ClientEntityData>
        => ClientEntities { ids, near } => ClientEntities
}
