//! Presentation-only client mod subsystem (a pack's `client_wasm`):
//! isolated instances beside the client replica, with read-only surface
//! sampling, published images/state, physical overlays, retained canvases,
//! and sandboxed storage. Never installed in the deterministic tick
//! scheduler.

mod calls;
mod runtime;
pub(in crate::modding) mod scope;
mod state;
mod storage;

pub(in crate::modding) use calls::{client_capability, handle_client_call};
#[cfg(test)]
pub(crate) use runtime::{client_storage_dir_for_test, seed_client_storage_for_test};
pub(crate) use runtime::{
    bake_installed_custom_item_geometry, delete_local_world_storage, local_session_key,
    remote_session_key, ClientCanvasView, ClientModRuntime, ClientUiView,
};
pub(in crate::modding) use state::ClientStoreData;
pub(crate) use state::{ClientCommand, ClientImageData, ClientOverlayRegistration};
