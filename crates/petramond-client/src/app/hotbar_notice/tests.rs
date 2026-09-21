use super::*;

#[test]
fn wand_notice_holds_then_fades_and_restarts_on_selection_or_mode_changes() {
    let mut notice = HotbarNotice::default();
    let mut state = UiState::default();
    let mut pending = String::new();
    let region = Some((0, "Region selection"));
    for (now, expected) in [(10.0, 1.0), (12.0, 1.0), (12.5, 0.5), (13.0, 0.0)] {
        notice.populate(region, &mut pending, now, &mut state);
        assert_eq!(state.get_f32("creative_notice_opacity"), Some(expected));
    }
    notice.populate(Some((0, "Cell selection")), &mut pending, 14.0, &mut state);
    assert_eq!(state.get_f32("creative_notice_opacity"), Some(1.0));
    notice.populate(None, &mut pending, 14.1, &mut state);
    assert_eq!(state.get_f32("creative_notice_opacity"), Some(0.0));
    notice.populate(region, &mut pending, 14.2, &mut state);
    assert_eq!(state.get_f32("creative_notice_opacity"), Some(1.0));
    notice.populate(
        Some((1, "Region selection")),
        &mut pending,
        18.0,
        &mut state,
    );
    assert_eq!(state.get_f32("creative_notice_opacity"), Some(1.0));
}

#[test]
fn errors_need_no_wand_survive_switching_tools_and_repeat_after_expiry() {
    let mut notice = HotbarNotice::default();
    let mut state = UiState::default();
    let mut pending = "Nothing to redo".to_string();
    notice.populate(None, &mut pending, 10.0, &mut state);
    assert!(pending.is_empty(), "delivery consumes the pending event");
    let region = Some((0, "Region selection"));
    notice.populate(region, &mut pending, 11.0, &mut state);
    assert_eq!(state.get_str("creative_notice"), Some("Nothing to redo"));
    notice.populate(None, &mut pending, 12.5, &mut state);
    assert_eq!(state.get_f32("creative_notice_opacity"), Some(0.5));
    notice.populate(None, &mut pending, 13.0, &mut state);
    assert_eq!(state.get_bool("creative_notice_visible"), Some(false));
    pending = "Nothing to redo".into();
    notice.populate(region, &mut pending, 14.0, &mut state);
    assert_eq!(state.get_str("creative_notice"), Some("Nothing to redo"));
    assert_eq!(state.get_f32("creative_notice_opacity"), Some(1.0));
    notice.populate(region, &mut pending, 17.0, &mut state);
    assert_eq!(state.get_bool("creative_notice_visible"), Some(false));
}
