//! Presentation-only client instance calls: overlays, registered keys,
//! replica surface sampling, document state, images and text, GUI/canvas
//! lifecycle, and sandboxed client storage.

use mod_api::{
    ClientCanvasElement, ClientOverlayAnchor, ClientSurfaceColumn, ClientSurfaceQuery,
    ClientTextRun, GuiValue, HostRet,
};

// Imported for intra-doc links only.
#[allow(unused_imports)]
use crate::Mod;

use crate::__rt::host_fn;

host_fn! {
    /// Register an always-on physical-pixel overlay image during [`Mod::init`].
    pub fn client_register_overlay(
        image_key: &str,
        anchor: ClientOverlayAnchor,
        margin: [u16; 2],
        display_size: [u16; 2]
    ) => ClientRegisterOverlay {
        image_key: image_key.into(),
        anchor,
        margin,
        display_size,
    }
}

host_fn! {
    /// Register a REMAPPABLE client key action during init: a stable bare `id`
    /// (the player's remap persists as `mod_id:id`), a display `label` for the
    /// Options → Controls screen, the DEFAULT physical `key` (for example
    /// `"key_m"`), and the `action_id` your `client_key` handler matches on.
    pub fn client_register_key(id: &str, label: &str, key: &str, action_id: u32)
        => ClientRegisterKey {
            id: id.into(),
            label: label.into(),
            key: key.into(),
            action_id,
        }
}

host_fn! {
    /// Read whole surface chunk columns from the client replica, revision gated:
    /// the reply is parallel to `queries`, `None` = column unknown, and a reply
    /// without cell bytes = unchanged since the queried revision. Only echo a
    /// revision back once a reply for it had every cell known (see
    /// [`mod_api::ClientSurfaceColumn`]).
    pub fn client_surface_columns(queries: Vec<ClientSurfaceQuery>) -> Vec<Option<ClientSurfaceColumn>>
        => ClientSurfaceColumns { queries } => ClientSurfaceColumns
}

host_fn! {
    /// Overwrite one rectangle of an existing published client image in place:
    /// `origin`/`size` in image pixels, `rgba` = exactly `size` pixels of RGBA8.
    pub fn client_image_blit(key: &str, origin: [u16; 2], size: [u16; 2], rgba: Vec<u8>)
        => ClientImageBlit { key: key.into(), origin, size, rgba }
}

host_fn! {
    pub fn client_ui_state_set(key: &str, value: GuiValue)
        => ClientUiStateSet { key: key.into(), value }
}

host_fn! {
    pub fn client_ui_state_get(key: &str) -> Option<GuiValue>
        => ClientUiStateGet { key: key.into() } => GuiValue
}

host_fn! {
    /// Publish one host-fed RGBA8 document/overlay/canvas image.
    pub fn client_image_set(key: &str, width: u16, height: u16, rgba: Vec<u8>)
        => ClientImageSet { key: key.into(), width, height, rgba }
}

host_fn! {
    /// Measure a single-line run using the host's shared text subsystem.
    pub fn client_text_measure(text: &str, scale: u8) -> [u16; 2]
        => ClientTextMeasure { text: text.into(), scale } => ClientTextSize
}

host_fn! {
    /// Draw ordered text runs into an already-published client image.
    pub fn client_image_draw_texts(key: &str, runs: Vec<ClientTextRun>)
        => ClientImageDrawTexts { key: key.into(), runs }
}

host_fn! {
    pub fn client_gui_open(kind_key: &str) -> bool
        => ClientGuiOpen { kind_key: kind_key.into() } => Bool
}

host_fn! {
    pub fn client_gui_close() => ClientGuiClose
}

host_fn! {
    pub fn client_canvas_open(canvas_key: &str, size: [u16; 2]) -> bool
        => ClientCanvasOpen { canvas_key: canvas_key.into(), size } => Bool
}

host_fn! {
    pub fn client_canvas_close() => ClientCanvasClose
}

host_fn! {
    pub fn client_canvas_scene_set(canvas_key: &str, elements: Vec<ClientCanvasElement>)
        => ClientCanvasSceneSet { canvas_key: canvas_key.into(), elements }
}

host_fn! {
    pub fn client_canvas_view_set(canvas_key: &str, offset: [f32; 2])
        => ClientCanvasViewSet { canvas_key: canvas_key.into(), offset }
}

host_fn! {
    pub fn client_storage_get_many(keys: Vec<String>) -> Vec<Option<Vec<u8>>>
        => ClientStorageGetMany { keys }
        => HostRet::ClientStorageValues(values) => values
            .into_iter()
            .map(|value| value.map(mod_api::ByteBuf::into_vec))
            .collect()
}

host_fn! {
    pub fn client_storage_set_many(entries: Vec<(String, Vec<u8>)>) -> bool
        => ClientStorageSetMany {
            entries: entries
                .into_iter()
                .map(|(key, value)| (key, mod_api::ByteBuf::from(value)))
                .collect(),
        } => Bool
}

host_fn! {
    /// Begin an asynchronous storage read on the host's background worker; the
    /// returned ticket resolves through [`client_storage_read_poll`], usually on
    /// a later frame. The REQUIRED path for bulk spatial reads — a slow disk
    /// delays the data, never the frame. Bounded outstanding tickets (see the
    /// ABI docs); ordered after already-issued writes.
    pub fn client_storage_read_begin(keys: Vec<String>) -> u64
        => ClientStorageReadBegin { keys } => U64
}

host_fn! {
    /// Poll an asynchronous storage read: `Some(values)` (parallel to the begun
    /// keys, `None` entry = absent) consumes the ticket, `None` = still in
    /// flight. Polling an unknown or consumed ticket disables the mod.
    pub fn client_storage_read_poll(ticket: u64) -> Option<Vec<Option<Vec<u8>>>>
        => ClientStorageReadPoll { ticket }
        => HostRet::ClientStorageRead(values) => values.map(|values| {
            values
                .into_iter()
                .map(|value| value.map(mod_api::ByteBuf::into_vec))
                .collect()
        })
}
