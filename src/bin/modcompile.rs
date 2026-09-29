//! Precompiles every `mod.wasm` under a mods root into a `mod.cwasm` beside it, for the packs
//! the game ships: the engine then loads the artifact instead of compiling the module on the
//! player's machine (a first-launch cost of tens of CPU-seconds), and falls back to compiling
//! whenever an artifact does not fit its engine or machine.
//!
//!   petramond_modcompile <mods-root> [--target <triple>]
//!
//! Artifacts are target-specific (a Windows x86-64 build cannot load a Linux one) and tied to
//! this engine version's configuration: produce them with the same engine build that ships,
//! once per packaged target.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root: Option<PathBuf> = None;
    let mut target: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--target" => target = args.next(),
            _ => root = Some(PathBuf::from(arg)),
        }
    }
    let Some(root) = root else {
        eprintln!("usage: petramond_modcompile <mods-root> [--target <triple>]");
        std::process::exit(2);
    };
    let mut failed = false;
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| {
            eprintln!("read {}: {e}", root.display());
            std::process::exit(2);
        })
        .flatten()
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        let wasm = dir.join("mod.wasm");
        if !wasm.is_file() {
            continue;
        }
        let started = std::time::Instant::now();
        match petramond::modding::precompile_module(&wasm, target.as_deref()) {
            Ok(bytes) => {
                let artifact = wasm.with_extension("cwasm");
                if let Err(e) = std::fs::write(&artifact, &bytes) {
                    eprintln!("write {}: {e}", artifact.display());
                    failed = true;
                    continue;
                }
                println!(
                    "{}: {} KiB in {:.1} s",
                    artifact.display(),
                    bytes.len() / 1024,
                    started.elapsed().as_secs_f64()
                );
            }
            Err(e) => {
                eprintln!("{e}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
