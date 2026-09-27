/// The image a canvas element draws, when it draws one at all: the geometry
/// and glyph primitives name no image.
pub(super) fn client_canvas_element_image_key(
    element: &mod_api::ClientCanvasElement,
) -> Option<&str> {
    match element {
        mod_api::ClientCanvasElement::Image { image_key, .. }
        | mod_api::ClientCanvasElement::Sprite { image_key, .. } => Some(image_key),
        mod_api::ClientCanvasElement::Rect { .. } | mod_api::ClientCanvasElement::Text { .. } => {
            None
        }
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
        mod_api::ClientCanvasElement::Rect { rect, .. } => {
            rect.iter().all(|value| value.is_finite()) && rect[2] > 0.0 && rect[3] > 0.0
        }
        mod_api::ClientCanvasElement::Text { pos, text, .. } => {
            pos.iter().all(|value| value.is_finite())
                && text.len() <= mod_api::CLIENT_CANVAS_TEXT_MAX
        }
    }
}

pub(super) const CLIENT_SURFACE_QUERY_MAX: usize = 512;
/// Positions one `ClientBlocksAt` call may read (the doc'd ABI bound).
pub(super) const CLIENT_BLOCKS_QUERY_MAX: usize = 512;
pub(super) const CLIENT_IMAGE_SIDE_MAX: u16 = 640;
pub(super) const CLIENT_OVERLAY_MAX: usize = 16;
pub(super) const CLIENT_OVERLAY_DISPLAY_SIDE_MAX: u16 = 2048;
pub(super) const CLIENT_UI_STRING_MAX: usize = 16 << 10;
pub(super) const CLIENT_TEXT_RUN_MAX: usize = 256;
pub(super) const CLIENT_TEXT_BYTES_MAX: usize = 16 << 10;
pub(super) const CLIENT_TEXT_SCALE_MAX: u8 = 8;
pub(super) const CLIENT_COMMAND_MAX: usize = 64;
pub(super) const CLIENT_CANVAS_SIDE_MAX: u16 = 2048;
pub(super) const CLIENT_CANVAS_MAX: usize = 8;
/// Largest per-axis ambient wind magnitude, blocks/s.
pub(super) const CLIENT_AMBIENT_WIND_MAX: f32 = 64.0;

/// A bare (un-namespaced) action id: lowercase snake_case, persisted in the
/// player's client.json as `mod_id:id` — so it must be stable and file-safe.
pub(super) fn valid_client_key_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

pub(super) fn gui_value_fits(value: &mod_api::GuiValue) -> bool {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            mod_api::GuiValue::Str(s) if s.len() > CLIENT_UI_STRING_MAX => return false,
            mod_api::GuiValue::List(rows) => {
                for row in rows {
                    for (key, value) in row {
                        if key.len() > CLIENT_UI_STRING_MAX {
                            return false;
                        }
                        pending.push(value);
                    }
                }
            }
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod gui_tests {
    use super::*;
    #[test]
    fn list_values_cannot_bypass_client_state_size_limits() {
        let list =
            |value| mod_api::GuiValue::List(vec![[("value".into(), value)].into_iter().collect()]);
        assert!(gui_value_fits(&list(mod_api::GuiValue::Str(
            "short".into()
        ))));
        assert!(!gui_value_fits(&list(mod_api::GuiValue::Str(
            "x".repeat(CLIENT_UI_STRING_MAX + 1)
        ))));
    }
}
