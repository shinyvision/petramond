use std::path::Path;
use std::process::{Command, Stdio};

pub fn reveal(path: &Path) -> bool {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let mut command = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        if path.is_dir() {
            c.arg(path);
        } else {
            c.arg("-R").arg(path);
        }
        c
    } else if cfg!(windows) {
        let mut c = Command::new("explorer");
        if path.is_dir() {
            c.arg(path);
        } else {
            c.arg(format!("/select,{}", path.display()));
        }
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(dir);
        c
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

const ONE_SHOT: &[&str] = &["PETRAMOND_WORLD", "PETRAMOND_START"];

pub fn spawn_self(start_route: &str) -> std::io::Result<()> {
    let exe = match std::env::var_os("APPIMAGE") {
        Some(image) if !image.is_empty() => std::path::PathBuf::from(image),
        _ => std::env::current_exe()?,
    };
    let mut command = Command::new(exe);
    command.args(std::env::args_os().skip(1));
    if let Ok(cwd) = std::env::current_dir() {
        command.current_dir(cwd);
    }
    for name in ONE_SHOT {
        command.env_remove(name);
    }
    command.env("PETRAMOND_START", start_route);
    command.spawn().map(drop)
}
