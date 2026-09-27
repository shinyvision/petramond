use std::path::{Path, PathBuf};
use std::sync::Arc;

use mod_api::{
    ClientMediaAudio, ClientMediaCapabilities, ClientMediaFailure, ClientMediaPhase,
    ClientMediaVideo,
};
use petramond::modding::client::media::{MediaDesk, MediaInput};
use petramond_util::test_dirs::TestScratchDir;

use super::clock::{PresentationClock, SteppedClock};
use super::encoder::{Encoder, EncoderSpec};
use super::ffmpeg;

fn video() -> ClientMediaVideo {
    ClientMediaVideo {
        codec: "libx264".into(),
        width: 32,
        height: 18,
        fps: [30, 1],
        options: vec![("crf".into(), "20".into())],
    }
}

fn audio() -> ClientMediaAudio {
    ClientMediaAudio {
        codec: "flac".into(),
        sample_rate: 48_000,
        channels: 2,
        options: vec![("b:a".into(), "192k".into())],
    }
}

fn spec(ffmpeg: &Path, with_audio: bool) -> EncoderSpec {
    EncoderSpec {
        ffmpeg: ffmpeg.to_path_buf(),
        container: "matroska".into(),
        video: Some(video()),
        audio: with_audio.then(audio),
        options: vec![("movflags".into(), "+faststart".into())],
    }
}

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let at = args.iter().position(|a| a == flag)?;
    args.get(at + 1).map(String::as_str)
}

#[test]
fn a_media_file_is_written_with_the_codecs_and_muxer_the_mod_names() {
    let spec = spec(Path::new("ffmpeg"), true);
    let pass_one = spec.video_args(&video(), Path::new("/out/v"));
    assert_eq!(value_after(&pass_one, "-c:v"), Some("libx264"));
    assert_eq!(value_after(&pass_one, "-s"), Some("32x18"));
    assert_eq!(value_after(&pass_one, "-framerate"), Some("30/1"));
    assert_eq!(value_after(&pass_one, "-i"), Some("pipe:0"));
    assert_eq!(value_after(&pass_one, "-pix_fmt"), Some("rgba"));
    assert_eq!(value_after(&pass_one, "-crf"), Some("20"));
    assert!(pass_one
        .windows(2)
        .any(|w| w[0] == "-f" && w[1] == "matroska"));
    assert!(
        !pass_one.iter().any(|a| a == "-movflags"),
        "container options go on the pass that makes the file"
    );
    assert_eq!(pass_one.last().map(String::as_str), Some("/out/v"));

    let mux = spec.mux_args(
        &audio(),
        Some(Path::new("/out/v")),
        Path::new("/out/a"),
        Path::new("/out/m"),
    );
    assert_eq!(value_after(&mux, "-c:v"), Some("copy"), "the video is kept");
    assert_eq!(value_after(&mux, "-c:a"), Some("flac"));
    assert_eq!(value_after(&mux, "-b:a"), Some("192k"));
    assert_eq!(value_after(&mux, "-movflags"), Some("+faststart"));
    assert_eq!(value_after(&mux, "-ar"), Some("48000"));
    assert_eq!(value_after(&mux, "-ac"), Some("2"));
    assert!(mux.windows(2).any(|w| w[0] == "-f" && w[1] == "f32le"));
    assert!(mux.windows(2).any(|w| w[0] == "-map" && w[1] == "1:a:0"));
    assert_eq!(mux.last().map(String::as_str), Some("/out/m"));

    let sound_only = spec.mux_args(&audio(), None, Path::new("/out/a"), Path::new("/out/m"));
    assert!(!sound_only.iter().any(|a| a == "copy" || a == "0:v:0"));
    assert!(sound_only
        .windows(2)
        .any(|w| w[0] == "-map" && w[1] == "0:a:0"));

    let silent = self::spec(Path::new("ffmpeg"), false).video_args(&video(), Path::new("/out/f"));
    assert_eq!(
        value_after(&silent, "-movflags"),
        Some("+faststart"),
        "with no audio, pass one makes the file"
    );
}

#[test]
fn a_machine_without_ffmpeg_can_write_nothing_and_says_how_to_fix_it() {
    assert_eq!(ffmpeg::find_in(std::ffi::OsStr::new("")), None);
    let empty = TestScratchDir::new("media-no-ffmpeg");
    assert_eq!(ffmpeg::find_in(empty.as_os_str()), None);
    let capabilities = ffmpeg::capabilities(None);
    assert_eq!(capabilities.encoder, None);
    assert!(capabilities.containers.is_empty() && capabilities.video_codecs.is_empty());

    let desk = MediaDesk::default();
    desk.publish_encoders(capabilities);
    let refusal = desk
        .open_refusal("matroska", Some(&video()), None)
        .expect("nothing opens without an encoder");
    assert!(refusal.contains("install ffmpeg"), "{refusal}");

    // An encoder that vanished after the probe fails the file in words.
    let input = Arc::new(MediaInput::at(empty.join("clip.mkv"), Some(&video()), None));
    let encoder = Encoder::start(spec(&empty.join("ffmpeg"), false), Arc::clone(&input));
    input.close();
    let state = finished(&encoder, &input);
    assert_eq!(state.phase, ClientMediaPhase::Failed);
    assert!(
        state
            .error
            .as_deref()
            .unwrap_or("")
            .contains("could not start ffmpeg"),
        "{state:?}"
    );
    assert_eq!(leftovers(&empty), Vec::<String>::new());
}

#[test]
fn an_encoder_listing_is_read_by_name() {
    let encoders = "Encoders:\n V..... = Video\n A..... = Audio\n ------\n \
                    V....D libx264              libx264 H.264\n A....D aac   AAC\n";
    let names: Vec<(String, String)> = ffmpeg::listed(encoders).collect();
    assert_eq!(
        names,
        [
            ("V....D".to_owned(), "libx264".to_owned()),
            ("A....D".to_owned(), "aac".to_owned())
        ]
    );
    let muxers = "File formats:\n D. = Demuxing\n .E = Muxing\n --\n  E mp4  MP4\n D  mov  QT\n";
    let names: Vec<String> = ffmpeg::listed(muxers)
        .filter(|(flags, _)| flags.contains('E'))
        .map(|(_, name)| name)
        .collect();
    assert_eq!(names, ["mp4"]);
}

#[cfg(unix)]
fn fake_encoder(dir: &Path, script: &str) -> PathBuf {
    let path = dir.join("ffmpeg");
    super::write_stand_in_encoder(&path, script);
    path
}

fn frame() -> Vec<u8> {
    vec![0; 32 * 18 * 4]
}

fn finished(encoder: &Encoder, input: &MediaInput) -> mod_api::ClientMediaStateData {
    let deadline = std::time::Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
    while !encoder.done() {
        assert!(
            std::time::Instant::now() < deadline,
            "the encoder never finished"
        );
        std::thread::yield_now();
    }
    input.state()
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name != "ffmpeg")
                .collect()
        })
        .unwrap_or_default()
}

fn open(dir: &Path, with_audio: bool) -> Arc<MediaInput> {
    let audio = with_audio.then(audio);
    Arc::new(MediaInput::at(
        dir.join("clip.mkv"),
        Some(&video()),
        audio.as_ref(),
    ))
}

#[cfg(unix)]
#[test]
fn an_encoder_that_fails_reports_its_own_words_and_leaves_no_file() {
    let dir = TestScratchDir::new("media-failing");
    let ffmpeg = fake_encoder(
        &dir,
        "cat >/dev/null\necho 'No space left on device' >&2\nexit 1",
    );
    let input = open(&dir, true);
    let encoder = Encoder::start(spec(&ffmpeg, true), Arc::clone(&input));
    assert!(input.push_frame(frame()));
    input.deliver_audio(vec![0; 3200 * 4]);
    input.close();
    let state = finished(&encoder, &input);
    assert_eq!(state.phase, ClientMediaPhase::Failed);
    assert!(
        state
            .error
            .as_deref()
            .unwrap_or("")
            .contains("No space left on device"),
        "{state:?}"
    );
    assert_eq!(state.failure, Some(ClientMediaFailure::DiskFull));
    assert_eq!(leftovers(&dir), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn an_encoder_that_dies_mid_stream_fails_the_file_in_its_own_words() {
    let dir = TestScratchDir::new("media-dying");
    let ffmpeg = fake_encoder(&dir, "echo 'Invalid argument' >&2\nexit 1");
    let input = open(&dir, false);
    let encoder = Encoder::start(spec(&ffmpeg, false), Arc::clone(&input));
    let deadline = std::time::Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
    while input.phase() != ClientMediaPhase::Failed {
        input.push_frame(frame());
        assert!(
            std::time::Instant::now() < deadline,
            "the file never failed"
        );
        std::thread::yield_now();
    }
    let state = finished(&encoder, &input);
    assert!(
        state
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Invalid argument"),
        "{state:?}"
    );
    assert!(!input.push_frame(frame()), "a failed file takes nothing");
    assert_eq!(leftovers(&dir), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn an_aborted_media_file_leaves_nothing() {
    let dir = TestScratchDir::new("media-abort");
    // An encoder that never finishes on its own: an abort has to stop it.
    let ffmpeg = fake_encoder(&dir, "cat >/dev/null\nexec sleep 600");
    let input = open(&dir, true);
    let encoder = Encoder::start(spec(&ffmpeg, true), Arc::clone(&input));
    assert!(input.push_frame(frame()));
    input.deliver_audio(vec![0; 3200 * 4]);
    input.close();
    let deadline = std::time::Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
    while input.phase() != ClientMediaPhase::Finishing {
        assert!(std::time::Instant::now() < deadline, "never finishing");
        std::thread::yield_now();
    }
    input.abort();
    encoder.kill();
    let state = finished(&encoder, &input);
    assert_eq!(state.phase, ClientMediaPhase::Failed);
    assert_eq!(state.failure, Some(ClientMediaFailure::Aborted));
    assert!(!dir.join("clip.mkv").exists(), "no file claims success");
    assert_eq!(leftovers(&dir), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn a_finished_file_replaces_the_old_one_only_once_complete() {
    let dir = TestScratchDir::new("media-finished");
    std::fs::write(dir.join("clip.mkv"), b"old").unwrap();
    // Pass one keeps the raw frames it is piped; pass two copies its first
    // input to its output.
    let ffmpeg = fake_encoder(
        &dir,
        "for last; do :; done\ncase \" $* \" in\n  *\" pipe:0 \"*) cat > \"$last\" ;;\n  \
         *) while [ \"$1\" != \"-i\" ]; do shift; done; cp \"$2\" \"$last\" ;;\nesac",
    );
    let input = open(&dir, true);
    let encoder = Encoder::start(spec(&ffmpeg, true), Arc::clone(&input));
    for _ in 0..3 {
        let deadline = std::time::Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
        while !input.push_frame(frame()) {
            assert!(std::time::Instant::now() < deadline, "the pipe never took");
            std::thread::yield_now();
        }
        input.deliver_audio(vec![0; 1600 * 8]);
        assert_eq!(std::fs::read(dir.join("clip.mkv")).unwrap(), b"old");
    }
    input.close();
    let state = finished(&encoder, &input);
    assert_eq!(state.phase, ClientMediaPhase::Finished, "{state:?}");
    assert_eq!(
        (state.frames_in, state.frames_encoded, state.audio_in),
        (3, 3, 4800)
    );
    assert_eq!(state.bytes, 3 * frame().len() as u64);
    assert_eq!(
        std::fs::metadata(dir.join("clip.mkv")).unwrap().len(),
        state.bytes
    );
    assert_eq!(leftovers(&dir), ["clip.mkv"]);
}

#[test]
fn a_full_disk_is_told_apart_by_the_systems_own_words() {
    let code = if cfg!(windows) { 112 } else { 28 };
    let io = std::io::Error::from_raw_os_error(code);
    assert_eq!(
        super::failure_of(&format!("could not write the audio track: {io}")),
        ClientMediaFailure::DiskFull
    );
    let words = io.to_string();
    let words = &words[..words.rfind(" (os error").unwrap_or(words.len())];
    assert_eq!(
        super::failure_of(&format!("ffmpeg failed (exit 1): out.mp4: {words}")),
        ClientMediaFailure::DiskFull,
        "the encoder repeating the system's words"
    );
    assert_eq!(
        super::failure_of("ffmpeg failed (exit 1): bad"),
        ClientMediaFailure::EncoderFailed
    );
}

#[test]
fn n_advances_span_exactly_n_steps() {
    for fps in [24.0, 30.0, 60.0] {
        let mut clock = SteppedClock::new(100.0, 1.0 / fps);
        let (n, mut advanced, mut attempt) = (200u64, 0u64, 0u64);
        let mut moved = 0.0;
        while advanced < n {
            moved += clock.take_step();
            // Hold every third frame, as an unsettled world would.
            if attempt % 3 != 2 {
                clock.advance(1);
                advanced += 1;
            }
            attempt += 1;
        }
        moved += clock.take_step();
        let expected = n as f64 / fps;
        assert!((clock.now() - 100.0 - expected).abs() < 1e-9, "{fps}");
        assert!(
            (moved - expected).abs() < 1e-9,
            "{fps}: {moved} vs {expected}"
        );
    }
}

#[test]
fn a_held_frame_moves_no_time_and_queued_advances_move_at_once() {
    let mut clock = SteppedClock::new(0.0, 0.5);
    assert_eq!(clock.take_step(), 0.0, "nothing advanced yet");
    clock.advance(3);
    assert_eq!(clock.take_step(), 1.5);
    assert_eq!(clock.take_step(), 0.0, "each advance steps once");
}

#[test]
fn presentation_time_never_runs_backwards_across_a_stepped_stretch() {
    let mut clock = PresentationClock::default();
    assert_eq!(clock.now(5.0), 5.0);
    clock.begin_stepped(5.0, 0.1);
    let stepped = clock.stepped_mut().unwrap();
    for _ in 0..20 {
        stepped.advance(1);
        stepped.take_step();
    }
    assert!(
        (clock.now(999.0) - 7.0).abs() < 1e-9,
        "stepped ignores the wall"
    );
    // It ran slower than real time: the wall is far ahead now.
    clock.end_stepped(60.0);
    assert!((clock.now(60.0) - 7.0).abs() < 1e-9);
    assert!((clock.now(61.0) - 8.0).abs() < 1e-9);
}

/// What `ClientMediaEncoders` answers once this machine was asked, and a
/// name it lacks refuses the open by name.
#[test]
fn a_media_open_naming_a_codec_the_machine_lacks_is_refused_with_its_name() {
    let desk = MediaDesk::default();
    assert!(
        desk.open_refusal("mp4", Some(&video()), None).is_some(),
        "not asked yet"
    );
    desk.publish_encoders(ClientMediaCapabilities {
        encoder: Some("ffmpeg version 7".into()),
        containers: vec!["mp4".into(), "matroska".into()],
        video_codecs: vec!["libx264".into()],
        audio_codecs: vec!["aac".into()],
    });
    assert_eq!(desk.open_refusal("mp4", Some(&video()), None), None);
    let lacking = desk
        .open_refusal("webm", Some(&video()), Some(&audio()))
        .unwrap();
    assert!(
        lacking.contains("webm") && lacking.contains("flac"),
        "{lacking}"
    );
    assert!(!lacking.contains("libx264"), "{lacking}");
}
