pub mod clock;
pub mod encoder;
pub mod ffmpeg;

#[cfg(test)]
mod tests;

use mod_api::ClientMediaFailure;

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
