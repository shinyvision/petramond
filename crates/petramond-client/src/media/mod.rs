//! The client's half of frames, time and sound out of the presented world:
//! the stepped clock ([`clock`]), finding and asking the encoder
//! ([`ffmpeg`]), and the encoder that writes a media file ([`encoder`]).
//!
//! The host calls a mod makes land on the engine's desk
//! (`petramond::modding::client::media`); the app carries them out
//! (`app::media`). Nothing here knows what a file is FOR: the mod names every
//! container, codec, option and path.

pub mod clock;
pub mod encoder;
pub mod ffmpeg;

#[cfg(test)]
mod tests;

use mod_api::ClientMediaFailure;

/// What a failed media file's message means. A full disk is the OPERATING
/// SYSTEM's own words for it, however they reached the message (an I/O error
/// here, or the encoder repeating what the OS told it).
pub fn failure_of(message: &str) -> ClientMediaFailure {
    if disk_full_words()
        .iter()
        .any(|words| message.contains(words.as_str()))
    {
        ClientMediaFailure::DiskFull
    } else {
        ClientMediaFailure::EncoderFailed
    }
}

/// The OS's messages for "no space" and "quota exceeded", without their
/// `(os error N)` suffix.
fn disk_full_words() -> &'static [String] {
    static WORDS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    WORDS.get_or_init(|| {
        let codes: &[i32] = if cfg!(windows) {
            &[39, 112]
        } else if cfg!(target_os = "macos") {
            &[28, 69]
        } else {
            &[28, 122]
        };
        codes
            .iter()
            .map(|&code| {
                let text = std::io::Error::from_raw_os_error(code).to_string();
                match text.rfind(" (os error") {
                    Some(at) => text[..at].to_owned(),
                    None => text,
                }
            })
            .filter(|words| !words.is_empty())
            .collect()
    })
}

/// Write an executable stand-in for the encoder at `path`. A child shell
/// writes it, never this process: a test process that holds a script open for
/// writing while another test forks makes that script unexecutable (ETXTBSY).
#[cfg(all(test, unix))]
pub(crate) fn write_stand_in_encoder(path: &std::path::Path, script: &str) {
    let status = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "printf '%s\\n' \"$1\" > \"$2\" && chmod 755 \"$2\"",
            "sh",
        ])
        .arg(format!("#!/bin/sh\n{script}"))
        .arg(path)
        .status()
        .expect("run /bin/sh");
    assert!(status.success(), "could not write {}", path.display());
}
