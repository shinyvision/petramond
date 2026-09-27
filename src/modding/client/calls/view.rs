//! The view-claim calls: camera, chrome, perspective, shader-param overrides,
//! and the read of what the client actually presents.
//!
//! Writes land on the calling mod's own store and are resolved across mods by
//! [`view::fold`](super::super::view::fold). The read answers the snapshot the
//! frame dispatch published, the same way `PlayerState` answers the actor's.

use mod_api::{ClientCall, HostRet, CLIENT_ENV_OVERRIDE_MAX};

use crate::modding::client::state::ClientStoreData;
use crate::modding::client::view::ViewCameraClaim;
use crate::modding::host::guards::{finite_pos, KV_MAX_KEY_BYTES};

pub(super) fn handle(client: &mut ClientStoreData, call: ClientCall) -> HostRet {
    match call {
        ClientCall::ClientViewCameraSet {
            pos,
            yaw,
            pitch,
            roll,
            fov_y,
            anchor,
        } => {
            // An anchored position is an offset, not a world point: finite
            // is its whole contract (the border clamps the resolved point).
            let pos = match anchor {
                Some(_) if pos.iter().all(|v| v.is_finite()) => pos,
                Some(_) => {
                    return HostRet::invalid("ClientViewCameraSet: offset must be finite".into())
                }
                None => match finite_pos(pos, "ClientViewCameraSet pos") {
                    Ok(pos) => pos.to_array(),
                    Err(error) => return error,
                },
            };
            if ![yaw, pitch, roll].iter().all(|v| v.is_finite())
                || fov_y.is_some_and(|f| !(f.is_finite() && f > 0.0 && f < std::f32::consts::PI))
            {
                return HostRet::invalid(
                    "ClientViewCameraSet: yaw/pitch/roll must be finite and fov_y in (0, π)".into(),
                );
            }
            client.view.camera = Some(ViewCameraClaim {
                pos,
                yaw,
                pitch,
                roll,
                fov_y,
                anchor,
            });
            HostRet::Unit
        }
        ClientCall::ClientViewCameraRelease => {
            client.view.camera = None;
            HostRet::Unit
        }
        ClientCall::ClientViewChromeSet {
            hud,
            hands,
            crosshair,
        } => {
            client.view.chrome.hud = hud;
            client.view.chrome.hands = hands;
            client.view.chrome.crosshair = crosshair;
            HostRet::Unit
        }
        ClientCall::ClientViewPerspectiveSet { third_person } => {
            client.view.third_person = third_person;
            HostRet::Unit
        }
        ClientCall::ClientViewState => match client.presented.lock().view {
            Some(view) => HostRet::ClientViewState(view),
            None => HostRet::invalid("ClientViewState: no frame has presented yet".into()),
        },
        ClientCall::ClientViewFrameSet { size } => {
            // The device's own limit; before a renderer has presented, only the floor.
            let max_side = client
                .presented
                .lock()
                .frame_limits
                .map_or(u32::MAX, |limits| limits.max_side);
            let side = 1..=max_side;
            if size.is_some_and(|[w, h]| !side.contains(&w) || !side.contains(&h)) {
                return HostRet::invalid(format!(
                    "ClientViewFrameSet: sides must be {}..={}",
                    side.start(),
                    side.end()
                ));
            }
            client.view.frame_size = size;
            HostRet::Unit
        }
        ClientCall::ClientViewSubjectSet { player } => {
            client.view.subject = player;
            HostRet::Unit
        }
        ClientCall::ClientEnvSet { params } => {
            if params.len() > CLIENT_ENV_OVERRIDE_MAX {
                return HostRet::invalid(format!(
                    "ClientEnvSet key count {} exceeds {CLIENT_ENV_OVERRIDE_MAX}",
                    params.len()
                ));
            }
            for (key, value) in &params {
                // Overrides cross namespaces, like the read they answer: a
                // param is the renderer's, not one pack's. Only the key's
                // shape and the value's finiteness are the mod's contract.
                if key.is_empty() || key.len() > KV_MAX_KEY_BYTES {
                    return HostRet::invalid(format!("invalid ClientEnvSet param key '{key}'"));
                }
                if !value.iter().all(|v| v.is_finite()) {
                    return HostRet::invalid(format!("ClientEnvSet param '{key}' must be finite"));
                }
            }
            client.view.env = params.into_iter().collect();
            HostRet::Unit
        }
        other => HostRet::invalid(format!(
            "non-view call {other:?} mis-routed to the view handler (host bug)"
        )),
    }
}
