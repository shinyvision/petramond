//! The VIEW CLAIMS a client instance publishes: where the frame looks from,
//! what chrome it draws, which perspective presents, and its own overrides of
//! the replicated shader params — plus the read of what actually presented.
//!
//! Every claim is RETAINED until this mod takes it back, and resolved across
//! mods: chrome folds so HIDING WINS, and the camera, the perspective and each
//! overridden param go to the last claimant in mod order.
//!
//! These are one frame's placement and three switches, nothing more. There is
//! no path, easing, keyframe or shot here — a mod that wants movement writes a
//! new claim every frame, which is where interpolation belongs.

use mod_api::{ClientEntitiesNear, ClientEntityData, ClientViewStateData, EntityRef, PlayerId};

use crate::__rt::host_fn;

host_fn! {
    /// CLIENT: claim where the view looks from — a world point, a look in
    /// radians, and optionally a vertical field of view (`None` keeps the
    /// player's own). With `anchor`, `pos` is an offset from that body's
    /// presented feet, resolved as the frame presents. Held until
    /// [`client_view_camera_release`].
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
    /// CLIENT: drop this mod's camera claim; the player's own eye presents
    /// again from the next frame.
    pub fn client_view_camera_release() => ClientViewCameraRelease
}

host_fn! {
    /// CLIENT: this mod's opinion about the on-screen chrome — `None` per field
    /// is no opinion, and a `Some(false)` anywhere hides that piece whatever
    /// another mod asked for. The call REPLACES this mod's whole opinion.
    pub fn client_view_chrome_set(
        hud: Option<bool>,
        hands: Option<bool>,
        crosshair: Option<bool>,
    ) => ClientViewChromeSet { hud, hands, crosshair }
}

host_fn! {
    /// CLIENT: claim which perspective presents — `Some(true)` third person,
    /// `Some(false)` first person, `None` releases the claim so the player's
    /// own toggle stands again.
    pub fn client_view_perspective_set(third_person: Option<bool>)
        => ClientViewPerspectiveSet { third_person }
}

host_fn! {
    /// CLIENT: what the client presented in the last frame, after every mod's
    /// claims and the player's own controls — read it to tell whether a claim
    /// reached the screen, where the camera stood, and whether the world on
    /// screen has settled.
    pub fn client_view_state() -> ClientViewStateData
        => ClientViewState => ClientViewState
}

host_fn! {
    /// CLIENT: override named shader params for this client only — the write
    /// counterpart of [`crate::client_env_params`]'s read. At most 16 keys, all
    /// finite; the call REPLACES this mod's whole override set, so an empty
    /// `params` clears it and every cleared key falls back to the replicated
    /// value. Presentation only: light values never change.
    pub fn client_env_set(params: Vec<(String, [f32; 4])>) => ClientEnvSet { params }
}

host_fn! {
    /// CLIENT: render the world (and the HUD the claims leave up) at exactly
    /// `size` pixels, shown scaled to fit where it presents; `None` releases.
    /// Sides from 1 to the device's `max_frame_side` ([`crate::client_engine_facts`]).
    pub fn client_view_frame_set(size: Option<[u32; 2]>) => ClientViewFrameSet { size }
}

host_fn! {
    /// CLIENT: present `player`'s view (their eye, hands and aim) whenever no
    /// camera is claimed; `None` releases.
    pub fn client_view_subject_set(player: Option<PlayerId>) => ClientViewSubjectSet { player }
}

host_fn! {
    /// CLIENT: the replicated players and mobs the last frame drew — those in
    /// `ids`, then with `near` the nearest within its radius.
    pub fn client_entities(ids: Vec<EntityRef>, near: Option<ClientEntitiesNear>)
        -> Vec<ClientEntityData>
        => ClientEntities { ids, near } => ClientEntities
}
