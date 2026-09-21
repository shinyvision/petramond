use super::*;
use petramond::net::protocol::TickUpdate;
use petramond::schematic::CreativeReply;

fn send_error(app: &mut TestApp, message: &str) {
    app.game
        .as_mut()
        .unwrap()
        .apply_tick_update(Box::new(TickUpdate {
            creative: vec![CreativeReply::Message(message.into())],
            ..Default::default()
        }));
}

#[test]
fn creative_errors_are_transient_without_moving_the_menu_hotbar() {
    let mut app = creative_app();
    let screen = (1280, 960);
    open_save_page(&mut app);
    app.drive_creative_menu(screen, 10.0);
    let hotbar = app.ui.out().rect("creative_hotbar").unwrap();
    send_error(&mut app, "Selected terrain is not loaded");
    app.drive_creative_menu(screen, 11.0);
    assert_eq!(
        app.ui.state_mut().get_str("creative_notice"),
        Some("Selected terrain is not loaded")
    );
    assert_eq!(app.ui.out().rect("creative_hotbar"), Some(hotbar));
    let notice = app.ui.out().rect("creative_notice").unwrap();
    let save = app.ui.out().rect("save_schematic").unwrap();
    assert!(notice.y + notice.h <= save.y);
    assert!(app.game().notice.is_empty());
    app.drive_creative_menu(screen, 13.5);
    assert_eq!(
        app.ui.state_mut().get_f32("creative_notice_opacity"),
        Some(0.5)
    );
    app.drive_creative_menu(screen, 14.0);
    assert!(app.ui.out().rect("creative_notice").is_none());
    assert_eq!(app.ui.out().rect("creative_hotbar"), Some(hotbar));
}

#[test]
#[ignore = "manual native creative error-notice verification"]
fn creative_notice_visual_check() {
    let mut app = creative_app();
    let screen = (1280, 960);
    let mut renderer = pollster::block_on(petramond_render::new_offscreen_renderer(
        screen.0,
        screen.1,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ));
    let dir = std::path::PathBuf::from(
        std::env::var_os("PETRAMOND_CREATIVE_QA").expect("capture directory"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let capture = |app: &mut TestApp, renderer: &mut petramond_render::Renderer, name: &str| {
        let mut rendered = false;
        for _ in 0..3 {
            if app.doc_ui_kind().is_some() {
                app.drive_creative_menu(screen, crate::app::now_seconds());
            }
            rendered = app.render(renderer);
        }
        assert!(rendered);
        let frame = renderer.capture_frame();
        image::save_buffer(
            dir.join(name),
            &frame.rgba,
            frame.width,
            frame.height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    };
    open_save_page(&mut app);
    app.library_form.name = "Workshop".into();
    app.game
        .as_mut()
        .unwrap()
        .world_tools
        .selection
        .selection
        .region([0, 0, 0], [3, 3, 3], false)
        .unwrap();
    capture(&mut app, &mut renderer, "menu-clean.png");
    send_error(&mut app, "Selected terrain is not loaded");
    capture(&mut app, &mut renderer, "menu-error.png");
    send_error(
        &mut app,
        "Could not save schematic: permission denied while writing the schematic library",
    );
    capture(&mut app, &mut renderer, "menu-long-error.png");
    app.close_screen();
    capture(&mut app, &mut renderer, "hud-error.png");
    std::thread::sleep(std::time::Duration::from_millis(2450));
    capture(&mut app, &mut renderer, "hud-fading.png");
    std::thread::sleep(std::time::Duration::from_millis(650));
    capture(&mut app, &mut renderer, "hud-expired.png");
}
