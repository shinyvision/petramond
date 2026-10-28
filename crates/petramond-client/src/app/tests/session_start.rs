use crate::app::{App, AppScreen};
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;
use petramond_util::test_dirs::TestScratchDir;
use petramond_world::{
    assets::{PackRoots, PackSet},
    content::{self, Content, ContentRegistry},
};

fn alone(name: &str) {
    petramond_world::test_child::run_ignored(
        &format!("app::tests::session_start::{name}_inner"),
        [(
            "PETRAMOND_DATA_DIR",
            petramond_util::test_dirs::test_process_data_dir(),
        )],
    )
    .assert_passed();
}

fn fixture(broken: bool) -> (TestScratchDir, Content) {
    let scratch = TestScratchDir::new("session-start");
    let mods = scratch.join("mods");
    let pack = mods.join("startfixture");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(
        pack.join("pack.json"),
        r#"{"id":"startfixture","name":"Startup fixture"}"#,
    )
    .unwrap();
    std::fs::write(
        pack.join("items.json"),
        r#"{"items":[{
        "item":"startfixture:item","key":"startfixture:item","name":"Fixture","max_stack_size":1
    }]}"#,
    )
    .unwrap();
    if broken {
        std::fs::write(pack.join("models.json"), r#"{"models":[{
            "key":"startfixture:bad","model_file":"unused.bbmodel","cells":[1,1,1],"parts":["same","same"]
        }]}"#).unwrap();
    }
    let packs =
        PackSet::discover(&PackRoots::with_mods([mods])).enabled(&["startfixture".into()].into());
    let base = ContentRegistry::load(packs).unwrap();
    (scratch, base)
}

fn app() -> App {
    App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 1.0), 1)
}

fn refused(app: &mut App, base: Content) {
    assert!(app.session.is_none());
    assert!(app.pending_start.is_none());
    assert_eq!(app.screen, AppScreen::ConnectionLost);
    assert!(Content::current().same(base));
    assert_eq!(
        app.content_serial,
        base.serial(),
        "UI and audio remain on the previous content"
    );
    assert!(app
        .prepare_pending::<()>(|_| panic!("a failed start must not retry"))
        .is_none());
}

#[test]
fn gpu_preparation_failure_does_not_start_or_open_the_world() {
    alone("gpu_preparation_failure_does_not_start_or_open_the_world");
}

#[test]
#[ignore]
fn gpu_preparation_failure_does_not_start_or_open_the_world_inner() {
    let (_scratch, base) = fixture(false);
    let _pin = content::pin(base);
    let mut app = app();
    let world = "gpu-preparation-failure";
    app.start_game(world, 1);
    assert!(app.pending_start.is_some());
    assert!(app
        .prepare_pending::<()>(|selected| {
            assert!(!selected.same(base));
            Err("fixture GPU rejection".into())
        })
        .is_none());
    refused(&mut app, base);
    assert!(!petramond::save::world_dir(world).exists());
    assert_eq!(app.shell.disconnect_message(), "fixture GPU rejection");
    app.update_frame((1280, 720));
    assert_eq!(
        app.ui.state_mut().get_str("failure_title"),
        Some("Couldn't open")
    );
}

#[test]
fn invalid_content_is_a_start_error_and_preserves_the_shell() {
    alone("invalid_content_is_a_start_error_and_preserves_the_shell");
}

#[test]
#[ignore]
fn invalid_content_is_a_start_error_and_preserves_the_shell_inner() {
    let (_scratch, base) = fixture(true);
    let _pin = content::pin(base);
    let mut app = app();
    app.start_game("invalid-content", 1);
    refused(&mut app, base);
    assert!(app.shell.disconnect_message().contains("startfixture:bad"));
    assert!(!petramond::save::world_dir("invalid-content").exists());
}

#[test]
fn world_open_failure_discards_preparation_and_restores_content() {
    alone("world_open_failure_discards_preparation_and_restores_content");
}

#[test]
#[ignore]
fn world_open_failure_discards_preparation_and_restores_content_inner() {
    let (_scratch, base) = fixture(false);
    let _pin = content::pin(base);
    let mut app = app();
    let dir = petramond::save::world_dir("unreadable-world");
    std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
    std::fs::write(&dir, b"not a directory").unwrap();
    app.start_game("unreadable-world", 1);
    let mut prepared = false;
    assert!(app
        .prepare_pending(|selected| {
            assert!(!selected.same(base));
            prepared = true;
            Ok(())
        })
        .is_none());
    assert!(prepared);
    refused(&mut app, base);
    assert!(app
        .shell
        .disconnect_message()
        .contains("Couldn't open world"));
    assert_eq!(std::fs::read(&dir).unwrap(), b"not a directory");
    std::fs::remove_file(dir).unwrap();
}
