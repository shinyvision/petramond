use super::app;
use mod_api::{ClientUiEvent, HostCall, HostRet};
use petramond_input::keycode::KeyCode;

const SCREEN: (u32, u32) = (1280, 720);

fn call(app: &mut crate::app::App, call: HostCall) -> HostRet {
    app.game_mut()
        .client_call_for_test("minimap", call)
        .expect("the minimap runs")
}

#[test]
fn a_mod_focuses_its_own_input_and_pauses_over_its_own_ui() {
    let mut app = app();
    app.update_frame(SCREEN);
    app.handle_raw_key(KeyCode::KeyN, true);
    let _ = app.handle_raw_key(KeyCode::KeyN, false);
    app.update_frame(SCREEN);
    app.update_frame(SCREEN);
    assert!(app.screen.client_ui_open(), "N opens the waypoint editor");
    assert_eq!(
        call(
            &mut app,
            HostCall::from(mod_api::calls::ClientUiFocus {
                id: "missing".into(),
                item: None
            })
        ),
        HostRet::Bool(false),
        "no such input on screen"
    );
    assert_eq!(
        call(
            &mut app,
            HostCall::from(mod_api::calls::ClientUiFocus {
                id: "name".into(),
                item: None
            })
        ),
        HostRet::Bool(true)
    );
    app.apply_client_mod_commands();
    app.update_frame(SCREEN);
    assert!(app.ui.text_input_focused(), "the caret is in the field");

    let editor = app.screen;
    assert_eq!(
        call(&mut app, HostCall::from(mod_api::calls::ClientPauseOpen)),
        HostRet::Bool(true)
    );
    app.apply_client_mod_commands();
    assert_eq!(app.screen, crate::app::AppScreen::Pause);
    app.resume_game();
    assert_eq!(app.screen, editor, "Resume goes back to the mod's document");
}

#[test]
fn hover_and_list_ranges_are_reported_once_per_change() {
    use petramond_ui::{FrameOutput, InstKey};
    let kind = petramond_world::gui_state::intern_kind("minimap:create").unwrap();
    let mut watch = crate::app::client_doc_events::DocWatch::default();
    let key = |id: &str, item| InstKey {
        id: id.into(),
        item,
    };
    let mut out = FrameOutput {
        hover_widget: Some(key("row_btn", Some(3))),
        list_ranges: vec![(key("rows", None), 0, 5), (key("inner", Some(1)), 0, 2)],
        ..FrameOutput::default()
    };
    let first = watch.changes(kind, &out);
    assert_eq!(
        first,
        vec![
            ClientUiEvent::Hover {
                id: Some("row_btn".into()),
                item: Some(3)
            },
            ClientUiEvent::ListRange {
                id: "rows".into(),
                first: 0,
                count: 5
            },
        ],
        "a list stamped inside a template reports no document range"
    );
    assert!(watch.changes(kind, &out).is_empty(), "nothing changed");
    out.hover_widget = None;
    out.list_ranges[0].1 = 2;
    assert_eq!(
        watch.changes(kind, &out),
        vec![
            ClientUiEvent::Hover {
                id: None,
                item: None
            },
            ClientUiEvent::ListRange {
                id: "rows".into(),
                first: 2,
                count: 5
            },
        ]
    );
}
