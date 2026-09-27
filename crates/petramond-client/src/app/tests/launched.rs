use std::path::{Path, PathBuf};

use super::super::{App, AppScreen};
use petramond::modding::client::ClientModRuntime;
use petramond_input::controls::Control;
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;

const SCREEN: (u32, u32) = (1280, 720);

fn guest(dir: &Path, name: &str, init: &[mod_api::HostCall]) -> PathBuf {
    let hex = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("\\{b:02x}")).collect() };
    let unit = mod_api::encode(&mod_api::GuestRet::Unit).unwrap();
    let packed = (1024u64 << 32) | unit.len() as u64;
    let (mut call_data, mut init_body) = (String::new(), String::new());
    let mut at = 2048;
    for call in init {
        let bytes = mod_api::encode(call).unwrap();
        call_data += &format!("  (data (i32.const {at}) \"{}\")\n", hex(&bytes));
        init_body += &format!(
            "(drop (call $hd (i32.const {at}) (i32.const {})))",
            bytes.len()
        );
        at += bytes.len();
    }
    let abi = petramond::modding::wat_abi_exports();
    let wat = format!(
        r#"(module
  (import "env" "host_dispatch" (func $hd (param i32 i32) (result i64)))
  (memory (export "memory") 1)
{call_data}  (data (i32.const 1024) "{unit}")
{abi}  (func (export "mod_init") (param i32 i64) {init_body})
  (func (export "mod_alloc") (param i32) (result i32) (i32.const 8192))
  (func (export "mod_free") (param i32 i32))
  (func (export "mod_dispatch") (param i32 i32) (result i64) (i64.const {packed})))"#,
        unit = hex(&unit),
    );
    let path = dir.join(format!("{name}.wat"));
    std::fs::write(&path, wat).unwrap();
    path
}

#[test]
fn a_launched_pack_lives_as_long_as_its_own_ui() {
    super::ensure_test_data_dir();
    let dir = petramond_util::test_dirs::TestScratchDir::new("app-launched");
    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);

    let opens = mod_api::HostCall::from(mod_api::calls::ClientCanvasOpen {
        canvas_key: "tool:canvas".into(),
        size: [64, 64],
    });
    let key = mod_api::HostCall::from(mod_api::calls::ClientRegisterKey {
        id: "choose".into(),
        label: "Choose".into(),
        key: "enter".into(),
        mods: mod_api::ClientKeyMods::default(),
        contexts: mod_api::ClientKeyContexts {
            gameplay: false,
            screens: vec!["tool:canvas".into()],
        },
        action_id: 1,
    });
    let tool =
        ClientModRuntime::launch_module_for_test("tool", &guest(&dir, "tool", &[key, opens]))
            .expect("the guest starts");
    app.host_launched(tool, SCREEN);
    assert_eq!(
        app.screen,
        AppScreen::ClientCanvas,
        "its canvas replaces the title"
    );
    assert!(
        app.controls.action_table.row("tool:choose").is_some(),
        "the keys it registered resolve on the shell"
    );
    for _ in 0..3 {
        app.update_frame(SCREEN);
    }
    assert!(app.shell_mods().is_some(), "it keeps running under its UI");
    assert!(app.handle_control(Control::CloseScreen, true));
    assert_eq!(
        app.screen,
        AppScreen::Title,
        "Escape goes back to the title"
    );
    assert!(app.shell_mods().is_none(), "and the shell ends with its UI");

    let silent = ClientModRuntime::launch_module_for_test("silent", &guest(&dir, "silent", &[]))
        .expect("the guest starts");
    app.host_launched(silent, SCREEN);
    assert_eq!(app.screen, AppScreen::Title);
    assert!(
        app.shell_mods().is_none(),
        "a pack that opens nothing has nothing to show"
    );
}
