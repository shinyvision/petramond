//! Presentation-only client instance calls: overlays, registered keys,
//! replica surface sampling, document state, images and text, GUI/canvas
//! lifecycle, and sandboxed client storage.

use mod_api::{
    ClientCanvasElement, ClientOverlayAnchor, ClientSurfaceColumn, ClientSurfaceQuery,
    ClientTextRun, GuiValue, HostCall, HostRet,
};

// Imported for intra-doc links only.
#[allow(unused_imports)]
use crate::Mod;

use crate::__rt;

/// Register an always-on physical-pixel overlay image during [`Mod::init`].
pub fn client_register_overlay(
    image_key: &str,
    anchor: ClientOverlayAnchor,
    margin: [u16; 2],
    display_size: [u16; 2],
) {
    __rt::expect_unit(
        "ClientRegisterOverlay",
        __rt::host_call(&HostCall::ClientRegisterOverlay {
            image_key: image_key.into(),
            anchor,
            margin,
            display_size,
        }),
    );
}

/// Register a REMAPPABLE client key action during init: a stable bare `id`
/// (the player's remap persists as `mod_id:id`), a display `label` for the
/// Options → Controls screen, the DEFAULT physical `key` (for example
/// `"key_m"`), and the `action_id` your `client_key` handler matches on.
pub fn client_register_key(id: &str, label: &str, key: &str, action_id: u32) {
    __rt::expect_unit(
        "ClientRegisterKey",
        __rt::host_call(&HostCall::ClientRegisterKey {
            id: id.into(),
            label: label.into(),
            key: key.into(),
            action_id,
        }),
    );
}

/// Read whole surface chunk columns from the client replica, revision gated:
/// the reply is parallel to `queries`, `None` = column unknown, and a reply
/// without cell bytes = unchanged since the queried revision. Only echo a
/// revision back once a reply for it had every cell known (see
/// [`mod_api::ClientSurfaceColumn`]).
pub fn client_surface_columns(queries: Vec<ClientSurfaceQuery>) -> Vec<Option<ClientSurfaceColumn>> {
    match __rt::host_call(&HostCall::ClientSurfaceColumns { queries }) {
        HostRet::ClientSurfaceColumns(columns) => columns,
        other => panic!("ClientSurfaceColumns returned {other:?}"),
    }
}

/// Overwrite one rectangle of an existing published client image in place:
/// `origin`/`size` in image pixels, `rgba` = exactly `size` pixels of RGBA8.
pub fn client_image_blit(key: &str, origin: [u16; 2], size: [u16; 2], rgba: Vec<u8>) {
    __rt::expect_unit(
        "ClientImageBlit",
        __rt::host_call(&HostCall::ClientImageBlit {
            key: key.into(),
            origin,
            size,
            rgba,
        }),
    );
}

pub fn client_ui_state_set(key: &str, value: GuiValue) {
    __rt::expect_unit(
        "ClientUiStateSet",
        __rt::host_call(&HostCall::ClientUiStateSet {
            key: key.into(),
            value,
        }),
    );
}

pub fn client_ui_state_get(key: &str) -> Option<GuiValue> {
    match __rt::host_call(&HostCall::ClientUiStateGet { key: key.into() }) {
        HostRet::GuiValue(value) => value,
        other => panic!("ClientUiStateGet returned {other:?}"),
    }
}

/// Publish one host-fed RGBA8 document/overlay/canvas image.
pub fn client_image_set(key: &str, width: u16, height: u16, rgba: Vec<u8>) {
    __rt::expect_unit(
        "ClientImageSet",
        __rt::host_call(&HostCall::ClientImageSet {
            key: key.into(),
            width,
            height,
            rgba,
        }),
    );
}

/// Measure a single-line run using the host's shared text subsystem.
pub fn client_text_measure(text: &str, scale: u8) -> [u16; 2] {
    match __rt::host_call(&HostCall::ClientTextMeasure {
        text: text.into(),
        scale,
    }) {
        HostRet::ClientTextSize(size) => size,
        other => panic!("ClientTextMeasure returned {other:?}"),
    }
}

/// Draw ordered text runs into an already-published client image.
pub fn client_image_draw_texts(key: &str, runs: Vec<ClientTextRun>) {
    __rt::expect_unit(
        "ClientImageDrawTexts",
        __rt::host_call(&HostCall::ClientImageDrawTexts {
            key: key.into(),
            runs,
        }),
    );
}

pub fn client_gui_open(kind_key: &str) -> bool {
    match __rt::host_call(&HostCall::ClientGuiOpen {
        kind_key: kind_key.into(),
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("ClientGuiOpen returned {other:?}"),
    }
}

pub fn client_gui_close() {
    __rt::expect_unit("ClientGuiClose", __rt::host_call(&HostCall::ClientGuiClose));
}

pub fn client_canvas_open(canvas_key: &str, size: [u16; 2]) -> bool {
    match __rt::host_call(&HostCall::ClientCanvasOpen {
        canvas_key: canvas_key.into(),
        size,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("ClientCanvasOpen returned {other:?}"),
    }
}

pub fn client_canvas_close() {
    __rt::expect_unit(
        "ClientCanvasClose",
        __rt::host_call(&HostCall::ClientCanvasClose),
    );
}

pub fn client_canvas_scene_set(canvas_key: &str, elements: Vec<ClientCanvasElement>) {
    __rt::expect_unit(
        "ClientCanvasSceneSet",
        __rt::host_call(&HostCall::ClientCanvasSceneSet {
            canvas_key: canvas_key.into(),
            elements,
        }),
    );
}

pub fn client_canvas_view_set(canvas_key: &str, offset: [f32; 2]) {
    __rt::expect_unit(
        "ClientCanvasViewSet",
        __rt::host_call(&HostCall::ClientCanvasViewSet {
            canvas_key: canvas_key.into(),
            offset,
        }),
    );
}

pub fn client_storage_get_many(keys: Vec<String>) -> Vec<Option<Vec<u8>>> {
    match __rt::host_call(&HostCall::ClientStorageGetMany { keys }) {
        HostRet::ClientStorageValues(values) => values,
        other => panic!("ClientStorageGetMany returned {other:?}"),
    }
}

pub fn client_storage_set_many(entries: Vec<(String, Vec<u8>)>) -> bool {
    match __rt::host_call(&HostCall::ClientStorageSetMany { entries }) {
        HostRet::Bool(ok) => ok,
        other => panic!("ClientStorageSetMany returned {other:?}"),
    }
}
