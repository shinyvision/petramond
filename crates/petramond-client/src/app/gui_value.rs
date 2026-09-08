use petramond_ui::UiValue;

pub(super) fn from_world(value: &petramond_world::gui_state::GuiValue) -> UiValue {
    use petramond_world::gui_state::GuiValue;
    match value {
        GuiValue::F32(v) => UiValue::F32(*v),
        GuiValue::I32(v) => UiValue::I32(*v),
        GuiValue::Str(v) => UiValue::Str(v.clone()),
        GuiValue::List(rows) => UiValue::List(std::sync::Arc::new(
            rows.iter()
                .map(|row| {
                    row.iter()
                        .map(|(k, v)| (k.clone(), from_world(v)))
                        .collect()
                })
                .collect(),
        )),
    }
}

pub(super) fn from_api(value: &mod_api::GuiValue) -> UiValue {
    use mod_api::GuiValue;
    match value {
        GuiValue::F32(v) => UiValue::F32(*v),
        GuiValue::I32(v) => UiValue::I32(*v),
        GuiValue::Str(v) => UiValue::Str(v.clone()),
        GuiValue::List(rows) => UiValue::List(std::sync::Arc::new(
            rows.iter()
                .map(|row| row.iter().map(|(k, v)| (k.clone(), from_api(v))).collect())
                .collect(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_ui::*;

    #[test]
    fn published_rows_stamp_independent_item_hooks_in_the_tooltip_tier() {
        let rows = ["test:first", "test:second"]
            .into_iter()
            .map(|name| {
                [("item".into(), mod_api::GuiValue::Str(name.into()))]
                    .into_iter()
                    .collect()
            })
            .collect();
        let mut state = UiState::default();
        state.set("rows", from_api(&mod_api::GuiValue::List(rows)));
        state.set("show", UiValue::Bool(true));
        let doc = Document::from_json(r#"{
            "format":1,"kind":"test:cost","class":"container",
            "root":{"type":"frame","children":[
                {"type":"tooltip","layout":{"abs":{"x":0,"y":0}},"bind":{"visible":"show"},"children":[
                    {"type":"list","id":"cost","layout":{"dir":"row"},"bind":{"items":"rows"},"children":[
                        {"type":"hook","id":"ingredient","layout":{"w":16,"h":16},"bind":{"item":"item"}}
                    ]}
                ]}
            ]}
        }"#).unwrap();
        let runtime = UiRuntime::new(std::sync::Arc::new(doc), petramond::gui::doc_theme::theme());
        let mut frame = FrameState::default();
        let mut out = FrameOutput::default();
        runtime.frame(
            FrameArgs {
                screen: (320, 240),
                scale: 1,
                now: 0.0,
                state: &state,
                input: &[],
                clipboard: None,
                images: &NoImages,
                dim: None,
                preview: None,
            },
            &mut frame,
            &mut out,
        );
        assert_eq!(
            out.hooks
                .iter()
                .map(|h| h.item.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("test:first"), Some("test:second")]
        );
        assert!(out.hooks.iter().all(|h| h.overlay));
        assert_ne!(out.hooks[0].rect, out.hooks[1].rect);
    }
}
