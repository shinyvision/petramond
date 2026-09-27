#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mod_api::{
    ClientAudioTapData, ClientCaptureSource, ClientCaptureStatus, ClientCaptureWhen,
    ClientMediaAudio, ClientMediaCapabilities, ClientMediaFailure, ClientMediaPhase,
    ClientMediaVideo, ClientStep,
};
use petramond::modding::client::media::{Capture, Destination, Media, MediaDesk, MediaInput, Tap};
use petramond_util::test_dirs::TestScratchDir;

use super::app;

const SCREEN: (u32, u32) = (320, 180);
const FPS: u32 = 30;
const OWNER: &str = "minimap";

fn renderer() -> Option<petramond_render::Renderer> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    if pollster::block_on(instance.request_adapter(&Default::default())).is_err() {
        eprintln!("[skip] no wgpu adapter; no frame to capture");
        return None;
    }
    Some(
        pollster::block_on(petramond_render::new_offscreen_renderer(
            SCREEN.0,
            SCREEN.1,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ))
        .expect("offscreen renderer"),
    )
}

/// Pass one keeps the raw frames it is piped; pass two copies its first
/// input to its output. A finished file is every frame's RGBA, end to end.
const RAW: &str =
    "for last; do :; done\ncase \" $* \" in\n  *\" pipe:0 \"*) cat > \"$last\" ;;\n  \
                   *) while [ \"$1\" != \"-i\" ]; do shift; done; cp \"$2\" \"$last\" ;;\nesac";

fn use_stand_in(app: &mut crate::app::App, dir: &Path, script: &str) {
    let path = dir.join("ffmpeg");
    crate::media::write_stand_in_encoder(&path, script);
    app.media.use_encoder_for_test(
        Some(path),
        ClientMediaCapabilities {
            encoder: Some("stand-in".into()),
            containers: vec!["matroska".into()],
            video_codecs: vec!["rawvideo".into()],
            audio_codecs: vec!["pcm_f32le".into()],
        },
    );
}

fn frame(app: &mut crate::app::App, renderer: &mut petramond_render::Renderer) {
    for _ in 0..4 {
        app.update(renderer);
        if app.render(renderer) {
            return;
        }
    }
    panic!("no frame presented");
}

fn frames_until(
    app: &mut crate::app::App,
    renderer: &mut petramond_render::Renderer,
    mut done: impl FnMut(&crate::app::App) -> bool,
) {
    let deadline = std::time::Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
    while !done(app) {
        assert!(std::time::Instant::now() < deadline, "never got there");
        frame(app, renderer);
    }
}

fn desk(app: &crate::app::App) -> MediaDesk {
    app.game().client_mod_runtime().media_desk().clone()
}

fn video(size: (u32, u32)) -> ClientMediaVideo {
    ClientMediaVideo {
        codec: "rawvideo".into(),
        width: size.0,
        height: size.1,
        fps: [FPS, 1],
        options: Vec::new(),
    }
}

fn sound(sample_rate: u32, channels: u16) -> ClientMediaAudio {
    ClientMediaAudio {
        codec: "pcm_f32le".into(),
        sample_rate,
        channels,
        options: Vec::new(),
    }
}

fn open(
    desk: &MediaDesk,
    at: PathBuf,
    video: Option<ClientMediaVideo>,
    audio: Option<ClientMediaAudio>,
) -> (u64, Arc<MediaInput>) {
    let input = Arc::new(MediaInput::at(at, video.as_ref(), audio.as_ref()));
    let id = desk.open_media(Media {
        owner: OWNER.into(),
        container: "matroska".into(),
        video,
        audio,
        options: Vec::new(),
        input: Arc::clone(&input),
        close_requested: false,
    });
    (id, input)
}

fn capture(desk: &MediaDesk, into: u64, advance: bool) -> u64 {
    desk.arm_capture(Capture {
        owner: OWNER.into(),
        source: ClientCaptureSource::Scene,
        size: None,
        when: ClientCaptureWhen::Next,
        advance,
        into: Destination::Media(into),
        status: ClientCaptureStatus::Armed,
    })
}

fn tap(desk: &MediaDesk, into: u64, audio: &ClientMediaAudio) -> u64 {
    desk.begin_tap(Tap {
        owner: OWNER.into(),
        sample_rate: audio.sample_rate,
        channels: audio.channels,
        into: Destination::Media(into),
        data: ClientAudioTapData {
            started_at: None,
            frames: 0,
            ended: false,
            error: None,
        },
        end_requested: false,
    })
}

fn armed(desk: &MediaDesk, id: u64) -> bool {
    desk.armed_captures().iter().any(|(armed, _)| *armed == id)
}

fn names_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n != "ffmpeg")
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn captures_into_a_media_file_step_the_clock_exactly_and_carry_their_tap() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let dir = TestScratchDir::new("media-stepped");
    let mut app = app();
    frame(&mut app, &mut renderer);
    use_stand_in(&mut app, &dir, RAW);
    let desk = desk(&app);
    let stereo = sound(48_000, 2);
    let mono = sound(44_100, 1);
    let (film, input) = open(
        &desk,
        dir.join("film.mkv"),
        Some(video(SCREEN)),
        Some(stereo.clone()),
    );
    let (track, track_input) = open(&desk, dir.join("track.mkv"), None, Some(mono.clone()));
    let step = 1.0 / f64::from(FPS);
    assert!(desk.set_clock(
        OWNER,
        Some(ClientStep {
            seconds: step,
            seed: 7
        })
    ));
    let taps = [tap(&desk, film, &stereo), tap(&desk, track, &mono)];
    frame(&mut app, &mut renderer);
    let start = app.now();

    const N: u64 = 12;
    for k in 0..N {
        let id = capture(&desk, film, true);
        frames_until(&mut app, &mut renderer, |_| !armed(&desk, id));
        for _ in 0..3 {
            app.render(&mut renderer);
        }
        assert!(desk.armed_captures().is_empty(), "capture {k}");
    }
    frame(&mut app, &mut renderer);
    let elapsed = app.now() - start;
    assert!(
        (elapsed - N as f64 * step).abs() < 1e-9,
        "{N} captures moved {elapsed} s"
    );
    frame(&mut app, &mut renderer);
    assert!((app.now() - start - N as f64 * step).abs() < 1e-9);

    desk.close_media(OWNER, film).unwrap();
    desk.close_media(OWNER, track).unwrap();
    for tap in taps {
        desk.end_tap(OWNER, tap).unwrap();
    }
    assert!(desk.set_clock(OWNER, None));
    frames_until(&mut app, &mut renderer, |_| {
        input.done() && track_input.done()
    });
    let state = input.state();
    assert_eq!(state.phase, ClientMediaPhase::Finished, "{state:?}");
    assert_eq!(state.frames_in, N);
    let covers = |audio_in: u64, rate: u32| {
        let expected = N as f64 * f64::from(rate) / f64::from(FPS);
        (audio_in as f64 - expected).abs() <= 1.0
    };
    assert!(
        covers(state.audio_in, 48_000),
        "{} sample frames",
        state.audio_in
    );
    let track_state = track_input.state();
    assert!(covers(track_state.audio_in, 44_100), "{track_state:?}");
    let bytes = std::fs::metadata(dir.join("film.mkv")).unwrap().len();
    assert_eq!(bytes, N * u64::from(SCREEN.0 * SCREEN.1 * 4));
}

#[test]
fn a_held_frame_draws_only_for_a_capture_or_a_due_present_and_only_an_advance_lifts_the_cap() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let dir = TestScratchDir::new("media-pacing");
    let mut app = app();
    frame(&mut app, &mut renderer);
    use_stand_in(&mut app, &dir, RAW);
    let desk = desk(&app);
    app.set_display_refresh_hz(1e-6);
    assert!(desk.set_clock(
        OWNER,
        Some(ClientStep {
            seconds: 0.25,
            seed: 1
        })
    ));
    frame(&mut app, &mut renderer);
    frame(&mut app, &mut renderer);
    let presented = app.media.last_present_for_test();
    let (film, input) = open(&desk, dir.join("held.mkv"), Some(video(SCREEN)), None);
    frames_until(&mut app, &mut renderer, |_| input.takes_frame());
    for _ in 0..3 {
        frame(&mut app, &mut renderer);
        assert!(!app.frame_uncapped(), "a held frame keeps the cap");
    }
    assert_eq!(
        app.media.last_present_for_test(),
        presented,
        "nothing presented"
    );

    let id = capture(&desk, film, false);
    frame(&mut app, &mut renderer);
    assert!(!armed(&desk, id), "a capture draws a held frame");
    assert_eq!(app.media.last_present_for_test(), presented);

    let start = app.now();
    let id = capture(&desk, film, true);
    frame(&mut app, &mut renderer);
    assert!(!armed(&desk, id));
    assert!(
        app.frame_uncapped(),
        "the frame after an advance runs uncapped"
    );
    frame(&mut app, &mut renderer);
    assert!(!app.frame_uncapped());
    assert!(
        (app.now() - start - 0.25).abs() < 1e-9,
        "one advance, one step"
    );
    desk.abort_media(OWNER, film).unwrap();
    frames_until(&mut app, &mut renderer, |_| input.done());
}

#[test]
fn an_aborted_media_file_leaves_nothing() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let dir = TestScratchDir::new("media-app-abort");
    let mut app = app();
    frame(&mut app, &mut renderer);
    use_stand_in(&mut app, &dir, RAW);
    let desk = desk(&app);
    let (film, input) = open(&desk, dir.join("gone.mkv"), Some(video(SCREEN)), None);
    for _ in 0..3 {
        let id = capture(&desk, film, false);
        frames_until(&mut app, &mut renderer, |_| !armed(&desk, id));
    }
    frames_until(&mut app, &mut renderer, |_| {
        input.state().frames_encoded >= 3
    });
    desk.abort_media(OWNER, film).unwrap();
    frames_until(&mut app, &mut renderer, |_| input.done());
    let state = input.state();
    assert_eq!(state.phase, ClientMediaPhase::Failed);
    assert_eq!(state.failure, Some(ClientMediaFailure::Aborted));
    assert_eq!(
        names_in(&dir),
        Vec::<String>::new(),
        "nothing claims success"
    );
}

#[test]
fn opens_beyond_the_running_encoders_wait_queued_and_start_in_order() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let dir = TestScratchDir::new("media-queue");
    let mut app = app();
    frame(&mut app, &mut renderer);
    use_stand_in(&mut app, &dir, RAW);
    let desk = desk(&app);
    let running = std::thread::available_parallelism().map_or(1, usize::from);
    let files: Vec<(u64, Arc<MediaInput>)> = (0..running + 2)
        .map(|i| {
            open(
                &desk,
                dir.join(format!("q{i}.mkv")),
                Some(video((2, 2))),
                None,
            )
        })
        .collect();
    frame(&mut app, &mut renderer);
    let phases = |files: &[(u64, Arc<MediaInput>)]| -> Vec<ClientMediaPhase> {
        files.iter().map(|(_, input)| input.phase()).collect()
    };
    let open_now = phases(&files);
    assert!(open_now[..running]
        .iter()
        .all(|p| *p == ClientMediaPhase::Open));
    assert_eq!(open_now[running..], [ClientMediaPhase::Queued; 2]);
    assert!(
        !files[running].1.push_frame(vec![0; 16]),
        "a queued file takes nothing"
    );

    let (first, first_input) = &files[0];
    assert!(first_input.push_frame(vec![0; 16]));
    desk.close_media(OWNER, *first).unwrap();
    frames_until(&mut app, &mut renderer, |_| {
        files[running].1.phase() == ClientMediaPhase::Open
    });
    assert_eq!(first_input.phase(), ClientMediaPhase::Finished);
    assert_eq!(
        files[running + 1].1.phase(),
        ClientMediaPhase::Queued,
        "opens start in the order they came"
    );
    for (id, _) in &files {
        desk.abort_media(OWNER, *id).unwrap();
    }
    frames_until(&mut app, &mut renderer, |_| {
        files.iter().all(|(_, i)| i.done())
    });
}

#[test]
fn a_scene_capture_never_holds_a_mods_canvas_on_the_window() {
    use petramond_input::keycode::KeyCode;
    let Some(mut renderer) = renderer() else {
        return;
    };
    let dir = TestScratchDir::new("media-canvas");
    let mut app = app();
    frame(&mut app, &mut renderer);
    use_stand_in(&mut app, &dir, RAW);
    let desk = desk(&app);
    let (film, input) = open(&desk, dir.join("canvas.mkv"), Some(video(SCREEN)), None);
    let take = |app: &mut crate::app::App, renderer: &mut petramond_render::Renderer| {
        let id = capture(&desk, film, false);
        frames_until(app, renderer, |_| !armed(&desk, id));
    };
    for _ in 0..30 {
        frame(&mut app, &mut renderer);
    }
    take(&mut app, &mut renderer);
    assert!(app.handle_raw_key(KeyCode::KeyM, true));
    let _ = app.handle_raw_key(KeyCode::KeyM, false);
    frame(&mut app, &mut renderer);
    assert!(app.screen.client_canvas_open(), "the map canvas opened");
    assert!(
        !app.client_overlays.items.is_empty(),
        "the canvas is on the window"
    );
    take(&mut app, &mut renderer);
    desk.close_media(OWNER, film).unwrap();
    frames_until(&mut app, &mut renderer, |_| input.done());
    assert_eq!(input.state().phase, ClientMediaPhase::Finished);

    let video = std::fs::read(dir.join("canvas.mkv")).unwrap();
    let frame_bytes = (SCREEN.0 * SCREEN.1 * 4) as usize;
    let pixels = |index: usize| video[index * frame_bytes..][..frame_bytes].chunks_exact(4);
    let changed = pixels(0)
        .zip(pixels(1))
        .filter(|(a, b)| (0..3).any(|c| a[c].abs_diff(b[c]) > 24))
        .count();
    assert!(
        changed * 50 < (SCREEN.0 * SCREEN.1) as usize,
        "{changed} pixels of the capture changed when the canvas opened on the window"
    );
}

#[test]
fn a_scene_capture_never_holds_a_mods_world_marks() {
    use petramond::modding::client::view::{ViewCameraClaim, ViewClaims};
    let Some(mut renderer) = renderer() else {
        return;
    };
    let magenta = |rgba: &[u8]| {
        rgba.chunks_exact(4)
            .filter(|p| p[0] > 200 && p[1] < 60 && p[2] > 200)
            .count()
    };
    let dir = TestScratchDir::new("media-marks");
    let mut app = app();
    frame(&mut app, &mut renderer);
    let eye = [0.5, 300.0, 0.5];
    let game = app.game_mut();
    assert!(game.claim_view_for_test(
        OWNER,
        ViewClaims {
            camera: Some(ViewCameraClaim {
                pos: eye,
                yaw: 0.0,
                pitch: 0.0,
                roll: 0.0,
                fov_y: None,
                anchor: None,
            }),
            ..Default::default()
        },
    ));
    let across = |x: f64| [eye[0] + x, eye[1], eye[2] + 4.0];
    assert!(game.set_client_world_marks_for_test(
        OWNER,
        vec![mod_api::ClientWorldMark::Line {
            from: across(-50.0),
            to: across(50.0),
            color: [255, 0, 255, 255],
            width: 12.0,
            occluded: 255,
        }],
    ));
    frame(&mut app, &mut renderer);
    assert!(
        magenta(&renderer.capture_frame().rgba) > 0,
        "the mark never reached the window"
    );

    use_stand_in(&mut app, &dir, RAW);
    let desk = desk(&app);
    let (film, input) = open(&desk, dir.join("marks.mkv"), Some(video(SCREEN)), None);
    for _ in 0..3 {
        let id = capture(&desk, film, false);
        frames_until(&mut app, &mut renderer, |_| !armed(&desk, id));
    }
    assert!(
        !app.world_marks.is_empty(),
        "the marks stayed on the window"
    );
    desk.close_media(OWNER, film).unwrap();
    frames_until(&mut app, &mut renderer, |_| input.done());
    let video = std::fs::read(dir.join("marks.mkv")).unwrap();
    assert!(!video.is_empty());
    assert_eq!(magenta(&video), 0, "a world mark reached a capture");
}

#[test]
#[ignore = "throughput measurement"]
fn capture_throughput() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    if pollster::block_on(instance.request_adapter(&Default::default())).is_err() {
        return;
    }
    const SIZE: (u32, u32) = (1920, 1080);
    let mut renderer = pollster::block_on(petramond_render::new_offscreen_renderer(
        SIZE.0,
        SIZE.1,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ))
    .expect("offscreen renderer");
    let dir = TestScratchDir::new("media-bench");
    let mut app = app();
    frame(&mut app, &mut renderer);
    let (video, audio) = if std::env::var_os("PETRAMOND_CAPTURE_BENCH_FFMPEG").is_some() {
        app.media.use_encoder_for_test(
            crate::media::ffmpeg::find(),
            crate::media::ffmpeg::capabilities(crate::media::ffmpeg::find().as_deref()),
        );
        ("libx264", "flac")
    } else {
        use_stand_in(
            &mut app,
            &dir,
            "for last; do :; done\ncase \" $* \" in\n  *\" pipe:0 \"*) cat > /dev/null; : > \"$last\" ;;\n  \
             *) : > \"$last\" ;;\nesac",
        );
        ("rawvideo", "pcm_f32le")
    };
    let desk = desk(&app);
    let stereo = ClientMediaAudio {
        codec: audio.into(),
        ..sound(48_000, 2)
    };
    let (film, input) = open(
        &desk,
        dir.join("bench.mkv"),
        Some(ClientMediaVideo {
            codec: video.into(),
            fps: [60, 1],
            ..self::video(SIZE)
        }),
        Some(stereo.clone()),
    );
    assert!(desk.set_clock(
        OWNER,
        Some(ClientStep {
            seconds: 1.0 / 60.0,
            seed: 1
        })
    ));
    let tap = tap(&desk, film, &stereo);
    let mut captured = 0u64;
    let mut run =
        |count: u64, app: &mut crate::app::App, renderer: &mut petramond_render::Renderer| {
            let target = captured + count;
            while captured < target {
                let id = capture(&desk, film, true);
                frames_until(app, renderer, |_| !armed(&desk, id));
                captured += 1;
            }
        };
    run(30, &mut app, &mut renderer);
    let start = std::time::Instant::now();
    run(600, &mut app, &mut renderer);
    let secs = start.elapsed().as_secs_f64();
    desk.close_media(OWNER, film).unwrap();
    desk.end_tap(OWNER, tap).unwrap();
    desk.set_clock(OWNER, None);
    frames_until(&mut app, &mut renderer, |_| input.done());
    eprintln!(
        "capture throughput: 600 frames in {secs:.2} s = {:.1} fps ({:?})",
        600.0 / secs,
        input.state().phase
    );
}
