use super::harness::TestApp;
use super::{cursor_over_menu, cursor_over_widget, App};
use mod_api::{ClientPointerPhase, ClientUiEvent, GuestCall, GuiValue, HostCall};
use petramond::menu::MenuAnchor;
use petramond::net::protocol::{ClientToServer, PlayerAction};
use petramond_input::keycode::KeyCode;
use petramond_world::gui_state::{MenuSlot, PointerButton};
use petramond_world::item::{ItemStack, ItemType};

const SCREEN: (u32, u32) = (1280, 720);
const PACK: &str = "codrive";
const KIND: &str = "codrive:desk";
const ECHO: &str = "codrive:ui";
const TITLE: &str = "codrive:title";
const CANVAS: &str = "codrive:canvas";

const DOCUMENT: &str = r#"{
  "format": 1, "kind": "codrive:desk", "class": "container",
  "root": {"type": "frame", "style": "panel.large", "layout": {"pad": [4, 2, 4, 4], "gap": 4}, "children": [
    {"type": "label", "text": "", "layout": {"w": 120}, "bind": {"text": "codrive:title"}},
    {"type": "row", "layout": {"gap": 4}, "children": [
      {"type": "slot", "id": "tray", "role": "container"},
      {"type": "image", "id": "canvas", "image": "", "interactive": true,
       "layout": {"w": 48, "h": 32}, "bind": {"image": "codrive:canvas_image"}},
      {"type": "button", "id": "stamp", "text": "Stamp", "layout": {"w": 48, "h": 16}}
    ]},
    {"type": "slider", "id": "level", "min": 0, "max": 10, "step": 1,
     "bind": {"value": "codrive:level"}, "layout": {"w": 120}},
    {"type": "slot_grid", "id": "inventory", "role": "player_inv", "cols": 9, "rows": 3},
    {"type": "slot_grid", "id": "hotbar", "role": "hotbar", "cols": 9, "rows": 1}
  ]}
}"#;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

/// A client guest that echoes every `ClientUi` call it is handed back to the server as a
/// `ClientEmitEvent` carrying the raw call, and publishes its UI state and image on every
/// other call (frames).
fn echo_guest() -> String {
    const UNIT_AT: usize = 1024;
    const EMIT_AT: usize = 16384;
    let unit = mod_api::encode(&mod_api::GuestRet::Unit).unwrap();
    let packed = ((UNIT_AT as u64) << 32) | unit.len() as u64;

    let publishes = [
        HostCall::from(mod_api::calls::ClientUiStateSet {
            key: TITLE.into(),
            value: GuiValue::Str("from-client".into()),
        }),
        HostCall::from(mod_api::calls::ClientUiStateSet {
            key: "codrive:canvas_image".into(),
            value: GuiValue::Str(CANVAS.into()),
        }),
        HostCall::from(mod_api::calls::ClientImageSet {
            key: CANVAS.into(),
            width: 2,
            height: 2,
            rgba: vec![255; 16],
        }),
    ];
    let (mut data, mut publish) = (String::new(), String::new());
    let mut at = 2048;
    for call in &publishes {
        let bytes = mod_api::encode(call).unwrap();
        data += &format!("  (data (i32.const {at}) \"{}\")\n", hex(&bytes));
        publish += &format!(
            "(drop (call $hd (i32.const {at}) (i32.const {})))\n      ",
            bytes.len()
        );
        at += bytes.len();
    }
    assert!(at < EMIT_AT);

    let sample = mod_api::encode(&GuestCall::ClientUi {
        kind_key: String::new(),
        event: ClientUiEvent::Dismiss,
    })
    .unwrap();
    let tag_len = sample.iter().position(|b| b & 0x80 == 0).unwrap() + 1;
    let is_ui = sample[..tag_len]
        .iter()
        .enumerate()
        .map(|(i, b)| format!("(i32.eq (i32.load8_u offset={i} (local.get 0)) (i32.const {b}))"))
        .reduce(|a, b| format!("(i32.and {a} {b})"))
        .unwrap();

    // `data` is the call's last field, so everything before its length prefix is fixed.
    let mut prefix = mod_api::encode(&HostCall::from(mod_api::calls::ClientEmitEvent {
        key: ECHO.into(),
        data: Vec::new(),
    }))
    .unwrap();
    assert_eq!(prefix.pop(), Some(0));
    let len_at = EMIT_AT + prefix.len();
    let prefix_len = prefix.len();
    let abi = petramond::modding::wat_abi_exports();
    format!(
        r#"(module
  (import "env" "host_dispatch" (func $hd (param i32 i32) (result i64)))
  (memory (export "memory") 1)
{data}  (data (i32.const {UNIT_AT}) "{unit}")
  (data (i32.const {EMIT_AT}) "{prefix}")
{abi}  (func (export "mod_init") (param i32 i64))
  (func (export "mod_alloc") (param i32) (result i32) (i32.const 32768))
  (func (export "mod_free") (param i32 i32))
  (func (export "mod_dispatch") (param i32 i32) (result i64)
    (local $n i32)
    (if {is_ui}
      (then
        (if (i32.lt_u (local.get 1) (i32.const 128))
          (then
            (i32.store8 (i32.const {len_at}) (local.get 1))
            (local.set $n (i32.const 1)))
          (else
            (i32.store8 (i32.const {len_at})
              (i32.or (i32.and (local.get 1) (i32.const 127)) (i32.const 128)))
            (i32.store8 (i32.const {len_hi}) (i32.shr_u (local.get 1) (i32.const 7)))
            (local.set $n (i32.const 2))))
        (memory.copy
          (i32.add (i32.const {len_at}) (local.get $n)) (local.get 0) (local.get 1))
        (drop (call $hd (i32.const {EMIT_AT})
          (i32.add (i32.add (i32.const {prefix_len}) (local.get $n)) (local.get 1)))))
      (else
      {publish}))
    (i64.const {packed})))"#,
        unit = hex(&unit),
        prefix = hex(&prefix),
        len_hi = len_at + 1,
    )
}

struct Desk {
    app: TestApp,
    anchor: MenuAnchor,
    _pin: petramond_world::content::PinGuard,
    _dir: petramond_util::test_dirs::TestScratchDir,
}

/// What the client put on the wire in one frame: the guest's echoes decoded, the rest as is.
#[derive(Debug, PartialEq)]
enum Sent {
    Ui(ClientUiEvent),
    Other(ClientToServer),
}

impl Desk {
    fn open() -> Self {
        let dir = petramond_util::test_dirs::TestScratchDir::new("app-co-driven");
        let pack = dir.join("mods").join(PACK);
        std::fs::create_dir_all(pack.join("ui/documents")).unwrap();
        std::fs::write(
            pack.join("pack.json"),
            format!(r#"{{"name": "Co-drive", "id": "{PACK}", "client_wasm": "client.wat"}}"#),
        )
        .unwrap();
        std::fs::write(pack.join("ui/documents/desk.gui.json"), DOCUMENT).unwrap();
        std::fs::write(pack.join("client.wat"), echo_guest()).unwrap();
        let pin = petramond_world::content::pin(petramond_world::content::test_support::with_mods(
            &dir.join("mods"),
        ));

        let mut app = super::app();
        app.update_frame(SCREEN);
        // A container needs a loaded cell to live in, and the test world loads none.
        let at = petramond_math::math::IVec3::new(0, 200, 0);
        app.server_world_mut()
            .insert_empty_column_for_test(petramond_world::chunk::ChunkPos::new(0, 0));
        let anchor = MenuAnchor::Block(at);
        let kind = petramond_world::gui_state::intern_kind(KIND).expect("the fixture kind");
        app.open_server_menu_for_test(kind, anchor);
        let mut desk = Self {
            app,
            anchor,
            _pin: pin,
            _dir: dir,
        };
        for _ in 0..20 {
            if desk.app.co_driven_menu().is_some() {
                break;
            }
            desk.app
                .frame_and_pump_recorded(SCREEN, &mut Default::default());
        }
        let runs = desk
            .app
            .game_mut()
            .client_call_for_test(PACK, HostCall::from(mod_api::calls::ClientMenu))
            .is_some();
        assert_eq!(
            desk.app.co_driven_menu(),
            Some(KIND),
            "the server-opened fixture menu is co-driven by the pack's client instance \
             (screen {:?}, instance runs: {runs})",
            desk.app.screen
        );
        desk.frame();
        desk
    }

    /// One app frame with a banked tick, relayed to the server and answered.
    fn frame(&mut self) -> Vec<Sent> {
        self.app.app.last -= 0.05;
        self.app.update_frame(SCREEN);
        let mut msgs = Vec::new();
        while let Ok(msg) = self.app.pipe.inbox.try_recv() {
            msgs.push(msg);
        }
        let sent = msgs.iter().map(sent).collect();
        for reply in self.app.send_and_pump(msgs) {
            let _ = self.app.pipe.outbox.send(reply);
        }
        sent
    }

    fn drive(&mut self, now: f64) {
        let kind = self.app.doc_ui_kind().expect("the menu is document-backed");
        self.app.drive_doc_menu(kind, SCREEN, now);
    }

    fn point_at(&mut self, id: &str, fx: f32, fy: f32) {
        cursor_over_widget(&mut self.app, SCREEN, id, None);
        let rect = self.app.ui.out().rect(id).expect("the widget is laid out");
        self.app.set_cursor_position(
            rect.x as f32 + rect.w as f32 * fx,
            rect.y as f32 + rect.h as f32 * fy,
        );
    }
}

fn sent(msg: &ClientToServer) -> Sent {
    if let ClientToServer::ModEvent { key, data } = msg {
        if key == ECHO {
            return match mod_api::decode::<GuestCall>(data).expect("the guest echoes whole calls") {
                GuestCall::ClientUi { kind_key, event } => {
                    assert_eq!(kind_key, KIND, "events name the co-driven document");
                    Sent::Ui(event)
                }
                other => panic!("the guest echoed a non-UI call: {other:?}"),
            };
        }
    }
    Sent::Other(msg.clone())
}

fn ui_events(sent: &[Sent]) -> Vec<&ClientUiEvent> {
    sent.iter()
        .filter_map(|s| match s {
            Sent::Ui(event) => Some(event),
            Sent::Other(_) => None,
        })
        .collect()
}

fn pointer_phases(events: &[&ClientUiEvent], want: &str) -> Vec<ClientPointerPhase> {
    events
        .iter()
        .filter_map(|e| match e {
            ClientUiEvent::ImagePointer { id, phase, .. } if id == want => Some(*phase),
            _ => None,
        })
        .collect()
}

/// GUI documents load once per process, from whatever content is current then, so a test
/// that needs the fixture pack's document runs alone in a child process.
macro_rules! alone {
    ($($name:ident => $alone:ident;)*) => {$(
        #[test]
        fn $name() {
            let path = concat!("app::tests::co_driven::", stringify!($alone));
            let none: [(&str, &str); 0] = [];
            petramond_world::test_child::run_ignored(path, none).assert_passed();
        }
    )*};
}

alone! {
    image_and_slider_input_in_a_server_menu_reaches_the_client_instance => image_and_slider_input_in_a_server_menu_reaches_the_client_instance_alone;
    a_button_click_in_a_server_menu_reaches_the_instance_and_the_server => a_button_click_in_a_server_menu_reaches_the_instance_and_the_server_alone;
    hovering_an_id_carrying_slot_reports_its_id_to_the_instance => hovering_an_id_carrying_slot_reports_its_id_to_the_instance_alone;
    the_instance_state_and_image_are_solved_into_the_server_menu => the_instance_state_and_image_are_solved_into_the_server_menu_alone;
    closing_a_co_driven_menu_sends_the_instance_reply_before_the_close => closing_a_co_driven_menu_sends_the_instance_reply_before_the_close_alone;
    a_mod_reads_the_open_server_menu_with_its_anchor_and_stacks => a_mod_reads_the_open_server_menu_with_its_anchor_and_stacks_alone;
}

#[test]
#[ignore = "run alone by the test of the same name"]
fn image_and_slider_input_in_a_server_menu_reaches_the_client_instance_alone() {
    let mut desk = Desk::open();
    desk.point_at("canvas", 0.25, 0.5);
    desk.app.set_pointer_button(PointerButton::Primary, true);
    desk.drive(1.0);
    desk.point_at("canvas", 0.75, 0.5);
    desk.drive(1.1);
    desk.app.set_pointer_button(PointerButton::Primary, false);
    desk.drive(1.2);

    desk.point_at("level", 0.9, 0.5);
    desk.app.set_pointer_button(PointerButton::Primary, true);
    desk.drive(2.0);
    desk.app.set_pointer_button(PointerButton::Primary, false);
    desk.drive(2.1);

    let sent = desk.frame();
    let events = ui_events(&sent);
    let phases = pointer_phases(&events, "canvas");
    assert_eq!(
        (phases.first(), phases.last()),
        (
            Some(&ClientPointerPhase::Down),
            Some(&ClientPointerPhase::Up)
        ),
        "the instance sees the stroke start and end: {events:?}"
    );
    assert!(
        phases.contains(&ClientPointerPhase::Move),
        "the held move is reported: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClientUiEvent::Slider { id, .. } if id == "level")),
        "the slider change reaches the instance: {events:?}"
    );
}

#[test]
#[ignore = "run alone by the test of the same name"]
fn a_button_click_in_a_server_menu_reaches_the_instance_and_the_server_alone() {
    let mut desk = Desk::open();
    desk.point_at("stamp", 0.5, 0.5);
    desk.app.set_pointer_button(PointerButton::Primary, true);
    desk.app.set_pointer_button(PointerButton::Primary, false);
    desk.drive(1.0);

    let sent = desk.frame();
    assert!(
        ui_events(&sent)
            .iter()
            .any(|e| matches!(e, ClientUiEvent::Click { id, .. } if id == "stamp")),
        "the instance hears the click: {sent:?}"
    );
    assert!(
        sent.iter().any(|s| match s {
            Sent::Other(msg) =>
                format!("{msg:?}").contains(&format!("{:?}", MenuSlot::Widget("stamp"))),
            _ => false,
        }),
        "the same click is queued for the server's widget lane: {sent:?}"
    );
}

#[test]
#[ignore = "run alone by the test of the same name"]
fn hovering_an_id_carrying_slot_reports_its_id_to_the_instance_alone() {
    let mut desk = Desk::open();
    desk.point_at("tray", 0.5, 0.5);
    desk.drive(1.0);
    let sent = desk.frame();
    assert!(
        ui_events(&sent).iter().any(|e| matches!(
            e,
            ClientUiEvent::Hover { id: Some(id), item: None } if id == "tray"
        )),
        "the slot's own id is the hover target: {sent:?}"
    );
}

#[test]
#[ignore = "run alone by the test of the same name"]
fn the_instance_state_and_image_are_solved_into_the_server_menu_alone() {
    let mut desk = Desk::open();
    desk.drive(1.0);
    let title = desk.app.ui.state_mut().get(TITLE).cloned();
    assert!(
        format!("{title:?}").contains("from-client"),
        "the bound label reads the instance's value, got {title:?}"
    );
    assert!(
        desk.app.ui.image_sources().iter().any(|source| matches!(
            source,
            petramond::gui::DocImageSource::Dynamic { key, .. } if key == CANVAS
        )),
        "the instance's image is supplied to the document"
    );
}

#[test]
fn an_overlaid_client_value_beats_the_server_value_and_is_withdrawn_when_dropped() {
    let mut app = App::new(
        petramond_render::camera::Camera::new(
            petramond_math::world_pos::WorldPos::new(0.0, 80.0, 0.0),
            16.0 / 9.0,
        ),
        1,
    );
    let server = crate::app::gui_value::from_api(&GuiValue::Str("server".into()));
    app.ui.state_mut().set("fixture:shared".to_owned(), server);
    let client: std::collections::BTreeMap<String, GuiValue> = [
        ("fixture:shared".to_owned(), GuiValue::Str("client".into())),
        ("fixture:own".to_owned(), GuiValue::I32(3)),
    ]
    .into();
    app.ui.overlay_client_state(&client);
    let shared = app.ui.state_mut().get("fixture:shared").cloned();
    assert!(format!("{shared:?}").contains("client"), "got {shared:?}");

    let kept: std::collections::BTreeMap<String, GuiValue> =
        [("fixture:shared".to_owned(), GuiValue::Str("client".into()))].into();
    app.ui.overlay_client_state(&kept);
    assert!(app.ui.state_mut().get("fixture:own").is_none());
    assert!(app.ui.state_mut().get("fixture:shared").is_some());
}

#[test]
#[ignore = "run alone by the test of the same name"]
fn closing_a_co_driven_menu_sends_the_instance_reply_before_the_close_alone() {
    let mut desk = Desk::open();
    desk.app.handle_raw_key(KeyCode::Escape, true);
    let _ = desk.app.handle_raw_key(KeyCode::Escape, false);
    assert!(!desk.app.screen.ui_open(), "Esc closes the menu locally");

    let sent = desk.frame();
    let dismissed = sent
        .iter()
        .position(|s| *s == Sent::Ui(ClientUiEvent::Dismiss))
        .unwrap_or_else(|| panic!("the instance was told and its event was sent: {sent:?}"));
    let closed = sent
        .iter()
        .position(|s| *s == Sent::Other(ClientToServer::Action(PlayerAction::CloseMenu)))
        .unwrap_or_else(|| panic!("the close was sent: {sent:?}"));
    assert!(
        dismissed < closed,
        "the server must hear the instance's dismiss event while the menu is still open: {sent:?}"
    );
}

#[test]
#[ignore = "run alone by the test of the same name"]
fn a_mod_reads_the_open_server_menu_with_its_anchor_and_stacks_alone() {
    let mut desk = Desk::open();
    let data: petramond_world::item::variant::VariantMap =
        [("codrive:mark".to_owned(), vec![7, 9])].into();
    let variant = petramond_world::item::variant::intern(&data).unwrap();
    let anchor = desk.anchor;
    anchor
        .edit_container(desk.app.server_world_mut(), |c| {
            c.slots[0] = Some(ItemStack::with_variant(ItemType::Grass, 5, variant));
        })
        .expect("opening the menu made its container");

    let mut menu = None;
    for _ in 0..20 {
        // The slot reaches the client in the server's menu sync, which only a ticking
        // pump sends.
        desk.app
            .frame_and_pump_recorded(SCREEN, &mut Default::default());
        let ret = desk
            .app
            .game_mut()
            .client_call_for_test(PACK, HostCall::from(mod_api::calls::ClientMenu))
            .expect("the fixture instance runs");
        let mod_api::HostRet::ClientMenu(Some(seen)) = ret else {
            panic!("the open mod menu is readable, got {ret:?}");
        };
        if seen.slots.iter().any(Option::is_some) {
            menu = Some(seen);
            break;
        }
    }
    let menu = menu.expect("the tray stack reaches the published menu");
    assert_eq!(menu.kind_key, KIND);
    assert!(menu.at.is_some(), "the block anchor is published");
    let stack = menu.slots[0].as_ref().expect("the tray slot");
    assert_eq!(stack.count, 5);
    assert_eq!(stack.data, vec![("codrive:mark".to_owned(), vec![7, 9])]);

    let tray = cursor_over_menu(&mut desk.app, SCREEN, MenuSlot::Container(0));
    assert!(tray.0 > 0.0, "the tray is the document's container slot");
}
