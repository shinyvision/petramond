//! Presentation-only client mod subsystem (a pack's `client_wasm`):
//! isolated instances beside the client replica, with read-only surface
//! sampling, published images/state, physical overlays, retained canvases,
//! and sandboxed storage. Never installed in the deterministic tick
//! scheduler; see WIKI/modding.md "Client modules".

mod calls;
mod runtime;
pub(in crate::modding) mod scope;
mod state;
mod storage;

pub(in crate::modding) use calls::{client_capability, handle_client_call};
pub(crate) use runtime::{ClientCanvasView, ClientModRuntime, ClientUiView};
pub(in crate::modding) use state::ClientStoreData;
pub(crate) use state::{ClientCommand, ClientImageData, ClientOverlayRegistration};
