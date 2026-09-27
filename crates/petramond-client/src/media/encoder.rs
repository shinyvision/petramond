//! The encoder: raw frames piped into ffmpeg on a writer thread, samples
//! written beside them, and one final pass that makes the file.
//!
//! Two passes, because a pipe carries one stream: pass one encodes the video
//! from stdin while the samples go raw to a sidecar; pass two copies that
//! video and encodes the sidecar into the finished container. With no audio,
//! pass one's output is the file; with no video, pass two encodes the samples
//! alone. Everything is written under hidden partial names and only the
//! finished file moves into place, so a failed, aborted or killed file never
//! leaves one that looks complete.
//!
//! The writer takes its work from the file's [`MediaInput`], which answers a
//! push from the encoder's own backpressure: the pipe is full while ffmpeg is
//! behind, and the writer is then still handing it a frame.

use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use mod_api::{ClientMediaAudio, ClientMediaFailure, ClientMediaPhase, ClientMediaVideo};
use petramond::modding::client::media::{MediaInput, MediaWork, ABORTED};

#[derive(Clone, Debug)]
pub struct EncoderSpec {
    pub ffmpeg: PathBuf,
    pub container: String,
    pub video: Option<ClientMediaVideo>,
    pub audio: Option<ClientMediaAudio>,
    pub options: Vec<(String, String)>,
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_owned()).collect()
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn options(options: &[(String, String)]) -> impl Iterator<Item = String> + '_ {
    options
        .iter()
        .flat_map(|(name, value)| [format!("-{name}"), value.clone()])
}

impl EncoderSpec {
    pub fn video_args(&self, video: &ClientMediaVideo, out: &Path) -> Vec<String> {
        let mut args = strings(&["-hide_banner", "-loglevel", "error", "-y"]);
        args.extend(strings(&["-f", "rawvideo", "-pix_fmt", "rgba"]));
        args.extend([
            "-s".into(),
            format!("{}x{}", video.width, video.height),
            "-framerate".into(),
            format!("{}/{}", video.fps[0], video.fps[1]),
            "-i".into(),
            "pipe:0".into(),
            "-an".into(),
            "-c:v".into(),
            video.codec.clone(),
        ]);
        args.extend(options(&video.options));
        if self.audio.is_none() {
            args.extend(options(&self.options));
        }
        args.extend(["-f".into(), self.container.clone(), path_arg(out)]);
        args
    }

    pub fn mux_args(
        &self,
        audio: &ClientMediaAudio,
        video: Option<&Path>,
        pcm: &Path,
        out: &Path,
    ) -> Vec<String> {
        let mut args = strings(&["-hide_banner", "-loglevel", "error", "-y"]);
        if let Some(video) = video {
            args.extend(["-i".into(), path_arg(video)]);
        }
        args.extend([
            "-f".into(),
            "f32le".into(),
            "-ar".into(),
            audio.sample_rate.to_string(),
            "-ac".into(),
            audio.channels.to_string(),
            "-i".into(),
            path_arg(pcm),
        ]);
        let pcm_input = if video.is_some() {
            args.extend(strings(&["-map", "0:v:0", "-c:v", "copy"]));
            1
        } else {
            0
        };
        args.extend([
            "-map".into(),
            format!("{pcm_input}:a:0"),
            "-c:a".into(),
            audio.codec.clone(),
        ]);
        args.extend(options(&audio.options));
        args.extend(options(&self.options));
        args.extend(["-f".into(), self.container.clone(), path_arg(out)]);
        args
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poison| poison.into_inner())
}

type ChildSlot = Arc<Mutex<Option<Child>>>;

struct Running {
    child: ChildSlot,
    stderr: Option<JoinHandle<String>>,
}

impl Running {
    fn spawn(
        ffmpeg: &Path,
        args: &[String],
        slot: &ChildSlot,
    ) -> Result<(Self, Option<ChildStdin>), String> {
        let mut child = Command::new(ffmpeg)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start ffmpeg at {}: {e}", ffmpeg.display()))?;
        let stdin = child.stdin.take();
        let stderr = child.stderr.take().and_then(|mut pipe| {
            std::thread::Builder::new()
                .name("petramond-media-encoder-log".into())
                .spawn(move || {
                    let mut text = Vec::new();
                    let _ = pipe.read_to_end(&mut text);
                    String::from_utf8_lossy(&text).trim().to_owned()
                })
                .ok()
        });
        *lock(slot) = Some(child);
        Ok((
            Self {
                child: Arc::clone(slot),
                stderr,
            },
            stdin,
        ))
    }

    fn wait(mut self) -> Result<(), String> {
        let status = loop {
            let polled = match lock(&self.child).as_mut() {
                Some(child) => child.try_wait(),
                None => break None,
            };
            match polled {
                Ok(Some(status)) => break Some(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(e) => return Err(format!("could not wait for ffmpeg: {e}")),
            }
        };
        lock(&self.child).take();
        let log = self
            .stderr
            .take()
            .and_then(|join| join.join().ok())
            .unwrap_or_default();
        match status {
            Some(status) if status.success() => Ok(()),
            Some(status) => Err(describe(status, &log)),
            None => Err(if log.is_empty() {
                "ffmpeg was stopped".into()
            } else {
                log
            }),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = lock(&self.child).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(join) = self.stderr.take() {
            let _ = join.join();
        }
    }
}

fn describe(status: ExitStatus, log: &str) -> String {
    if log.is_empty() {
        format!("ffmpeg failed ({status})")
    } else {
        format!("ffmpeg failed ({status}): {log}")
    }
}

pub struct Encoder {
    input: Arc<MediaInput>,
    child: ChildSlot,
    worker: Option<JoinHandle<()>>,
}

impl Encoder {
    pub fn start(spec: EncoderSpec, input: Arc<MediaInput>) -> Self {
        let child: ChildSlot = Arc::default();
        input.set_phase(ClientMediaPhase::Open);
        let job = Job {
            spec,
            input: Arc::clone(&input),
            child: Arc::clone(&child),
        };
        let worker = std::thread::Builder::new()
            .name("petramond-media-encoder".into())
            .spawn(move || job.run());
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(e) => {
                input.failed(
                    ClientMediaFailure::EncoderFailed,
                    format!("could not start the media writer: {e}"),
                );
                None
            }
        };
        Self {
            input,
            child,
            worker,
        }
    }

    pub fn done(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub fn abandon(&self) {
        if !self.input.closed() {
            self.input.abort();
        }
    }

    pub fn aborted(&self) -> bool {
        self.input.aborted()
    }

    pub fn kill(&self) {
        if let Some(child) = lock(&self.child).as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        if !self.input.closed() {
            self.input.abort();
        }
        if self.input.aborted() {
            self.kill();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Job {
    spec: EncoderSpec,
    input: Arc<MediaInput>,
    child: ChildSlot,
}

struct Partials {
    video: PathBuf,
    pcm: PathBuf,
    finished: PathBuf,
}

impl Job {
    fn run(self) {
        let partials = Partials {
            video: self.input.partial(Some("video")),
            pcm: self.input.partial(Some("pcm")),
            finished: self.input.partial(None),
        };
        let outcome = self.write(&partials).and_then(|()| self.finish(&partials));
        for path in [&partials.video, &partials.pcm, &partials.finished] {
            let _ = std::fs::remove_file(path);
        }
        match outcome {
            Ok(bytes) => self.input.finished(bytes),
            Err(_) if self.input.aborted() => self
                .input
                .failed(ClientMediaFailure::Aborted, ABORTED.into()),
            Err(e) => {
                log::error!("media file {} failed: {e}", self.input.target().display());
                self.input.failed(super::failure_of(&e), e)
            }
        }
    }

    fn write(&self, partials: &Partials) -> Result<(), String> {
        if let Some(parent) = partials.finished.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        let video_out = if self.spec.audio.is_some() {
            &partials.video
        } else {
            &partials.finished
        };
        let mut video = match &self.spec.video {
            Some(video) => {
                let (running, stdin) = Running::spawn(
                    &self.spec.ffmpeg,
                    &self.spec.video_args(video, video_out),
                    &self.child,
                )?;
                let stdin = stdin.ok_or("ffmpeg's input could not be opened")?;
                Some((running, stdin))
            }
            None => None,
        };
        let mut pcm = match &self.spec.audio {
            Some(_) => Some(BufWriter::new(
                std::fs::File::create(&partials.pcm)
                    .map_err(|e| format!("could not write the audio track: {e}"))?,
            )),
            None => None,
        };
        loop {
            match self.input.next_work() {
                MediaWork::Frame(rgba) => {
                    let Some((_, stdin)) = video.as_mut() else {
                        continue;
                    };
                    let written = stdin.write_all(&rgba);
                    match (written, video.take()) {
                        (Ok(()), running) => {
                            video = running;
                            self.input.frame_encoded();
                        }
                        (Err(e), Some((running, stdin))) => {
                            drop(stdin);
                            let encoded = self.input.state().frames_encoded;
                            return Err(match running.wait() {
                                Err(why) => {
                                    format!("the encoder stopped after {encoded} frames: {why}")
                                }
                                Ok(()) => {
                                    format!("the encoder stopped after {encoded} frames: {e}")
                                }
                            });
                        }
                        (Err(e), None) => return Err(format!("the encoder stopped: {e}")),
                    }
                }
                MediaWork::Audio(bytes) => {
                    if let Some(out) = pcm.as_mut() {
                        out.write_all(&bytes)
                            .map_err(|e| format!("could not write the audio track: {e}"))?;
                    }
                }
                MediaWork::Finish => break,
                MediaWork::Abort => {
                    if let Some(child) = lock(&self.child).as_mut() {
                        let _ = child.kill();
                    }
                    if let Some((running, stdin)) = video.take() {
                        drop(stdin);
                        let _ = running.wait();
                    }
                    return Err(ABORTED.into());
                }
            }
        }
        self.input.set_phase(ClientMediaPhase::Finishing);
        if let Some(out) = pcm.as_mut() {
            out.flush()
                .map_err(|e| format!("could not write the audio track: {e}"))?;
        }
        if let Some((running, stdin)) = video.take() {
            drop(stdin);
            running.wait()?;
        }
        Ok(())
    }

    fn finish(&self, partials: &Partials) -> Result<u64, String> {
        if let Some(audio) = &self.spec.audio {
            let video = self
                .spec
                .video
                .is_some()
                .then_some(partials.video.as_path());
            let args = self
                .spec
                .mux_args(audio, video, &partials.pcm, &partials.finished);
            let (running, stdin) = Running::spawn(&self.spec.ffmpeg, &args, &self.child)?;
            drop(stdin);
            running
                .wait()
                .map_err(|why| format!("the finished file could not be written: {why}"))?;
        }
        if self.input.aborted() {
            return Err(ABORTED.into());
        }
        self.input.place()
    }
}
