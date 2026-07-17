pub(super) fn client_canvas_element_image_key(element: &mod_api::ClientCanvasElement) -> &str {
    match element {
        mod_api::ClientCanvasElement::Image { image_key, .. }
        | mod_api::ClientCanvasElement::Sprite { image_key, .. } => image_key,
    }
}

pub(super) fn client_canvas_element_valid(element: &mod_api::ClientCanvasElement) -> bool {
    match element {
        mod_api::ClientCanvasElement::Image { rect, .. } => {
            rect.iter().all(|value| value.is_finite()) && rect[2] > 0.0 && rect[3] > 0.0
        }
        mod_api::ClientCanvasElement::Sprite { center, .. } => {
            center[0].is_finite() && center[1].is_finite()
        }
    }
}

pub(super) const CLIENT_SURFACE_QUERY_MAX: usize = 512;
/// Positions one `ClientBlocksAt` call may read (the doc'd ABI bound).
pub(super) const CLIENT_BLOCKS_QUERY_MAX: usize = 512;
pub(super) const CLIENT_IMAGE_SIDE_MAX: u16 = 640;
pub(super) const CLIENT_OVERLAY_MAX: usize = 16;
pub(super) const CLIENT_OVERLAY_DISPLAY_SIDE_MAX: u16 = 2048;
pub(super) const CLIENT_KEY_BINDING_MAX: usize = 32;
pub(super) const CLIENT_UI_STATE_MAX: usize = 1024;
pub(super) const CLIENT_UI_STRING_MAX: usize = 16 << 10;
pub(super) const CLIENT_IMAGE_MAX: usize = 64;
pub(super) const CLIENT_TEXT_RUN_MAX: usize = 256;
pub(super) const CLIENT_TEXT_BYTES_MAX: usize = 16 << 10;
pub(super) const CLIENT_TEXT_SCALE_MAX: u8 = 8;
pub(super) const CLIENT_COMMAND_MAX: usize = 64;
pub(super) const CLIENT_CANVAS_SIDE_MAX: u16 = 2048;
pub(super) const CLIENT_CANVAS_MAX: usize = 8;
pub(super) const CLIENT_CANVAS_ELEMENT_MAX: usize = 64;
/// Named shader params one `ClientEnvParams` call may read (the GPU slot
/// budget — no shader can consume more anyway).
pub(super) const CLIENT_ENV_PARAM_MAX: usize = 16;
/// Largest per-axis ambient wind magnitude, blocks/s.
pub(super) const CLIENT_AMBIENT_WIND_MAX: f32 = 64.0;

pub(super) fn valid_client_key(key: &str) -> bool {
    key.strip_prefix("key_")
        .is_some_and(|tail| tail.len() == 1 && tail.as_bytes()[0].is_ascii_lowercase())
        || key
            .strip_prefix("digit_")
            .is_some_and(|tail| tail.len() == 1 && tail.as_bytes()[0].is_ascii_digit())
}

/// A bare (un-namespaced) action id: lowercase snake_case, persisted in the
/// player's client.json as `mod_id:id` — so it must be stable and file-safe.
pub(super) fn valid_client_key_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
