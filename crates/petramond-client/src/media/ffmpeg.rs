use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use mod_api::ClientMediaCapabilities;

pub fn find() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("PETRAMOND_FFMPEG") {
        let path = PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }
    find_in(&std::env::var_os("PATH")?)
}

pub fn find_in(path_list: &OsStr) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["ffmpeg.exe"]
    } else {
        &["ffmpeg"]
    };
    std::env::split_paths(path_list)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

pub fn capabilities(ffmpeg: Option<&Path>) -> ClientMediaCapabilities {
    let asked = ffmpeg.and_then(|path| {
        let version = run(path, &["-version"])?;
        let muxers = run(path, &["-hide_banner", "-muxers"])?;
        let encoders = run(path, &["-hide_banner", "-encoders"])?;
        Some((version, muxers, encoders))
    });
    let Some((version, muxers, encoders)) = asked else {
        return ClientMediaCapabilities {
            encoder: None,
            containers: Vec::new(),
            video_codecs: Vec::new(),
            audio_codecs: Vec::new(),
        };
    };
    let codecs = |kind: char| {
        listed(&encoders)
            .filter(|(flags, _)| flags.starts_with(kind))
            .map(|(_, name)| name)
            .collect()
    };
    ClientMediaCapabilities {
        encoder: version.lines().next().map(|line| line.trim().to_owned()),
        containers: listed(&muxers)
            .filter(|(flags, _)| flags.contains('E'))
            .map(|(_, name)| name)
            .collect(),
        video_codecs: codecs('V'),
        audio_codecs: codecs('A'),
    }
}

fn run(ffmpeg: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| log::warn!("ffmpeg at {} could not be run: {e}", ffmpeg.display()))
        .ok()?;
    if !out.status.success() {
        log::warn!(
            "ffmpeg at {} failed to answer {args:?}: {}",
            ffmpeg.display(),
            out.status
        );
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub(super) fn listed(text: &str) -> impl Iterator<Item = (String, String)> + '_ {
    text.lines()
        .skip_while(|line| !line.trim_start().starts_with("--"))
        .skip(1)
        .filter_map(|line| {
            let mut tokens = line.split_whitespace();
            let flags = tokens.next()?.to_owned();
            let name = tokens.next()?.to_owned();
            Some((flags, name))
        })
}
