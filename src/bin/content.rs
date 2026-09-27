//! `petramond_content`: the content library's build and install side, with
//! no GPU and no window.
//!
//!   pack <pack dir> [--wasm <file>] --out <dir> [--content-packs a,b,...]
//!   check <zip> [--content-packs a,b,...]
//!   install <zip> --kind addon|mod
//!
//! `pack` writes `<out>/<id>-<version>.zip` and prints its path, after the
//! archive passes `check`: the installer's own reader, the website's limits,
//! an id that is not a content pack's, and pack admission as the game runs
//! it. `--content-packs` names the content pack ids (default: the packs in
//! the game's shipped roots). `install` stages the archive exactly as the
//! content browser stages a download, as a local install (no content id);
//! the next start of the game applies it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use petramond::content::archive::{self, Checked};
use petramond::content::install::{self, Offer, Op};
use petramond::content::{ContentLock, Dirs, Kind};

const USAGE: &str = "usage:
  petramond_content pack <pack dir> [--wasm <file>] --out <dir> [--content-packs a,b,...]
  petramond_content check <zip> [--content-packs a,b,...]
  petramond_content install <zip> --kind addon|mod";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("pack") => Args::parse(&args[1..]).and_then(|a| pack(&a)),
        Some("check") => Args::parse(&args[1..]).and_then(|a| check_file(&a)),
        Some("install") => Args::parse(&args[1..]).and_then(|a| install(&a)),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("petramond_content: {why}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Default)]
struct Args {
    path: Option<PathBuf>,
    wasm: Option<PathBuf>,
    out: Option<PathBuf>,
    content_packs: Option<BTreeSet<String>>,
    kind: Option<Kind>,
}

impl Args {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut out = Self::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = || {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{arg} needs a value\n{USAGE}"))
            };
            match arg.as_str() {
                "--wasm" => out.wasm = Some(value()?.into()),
                "--out" => out.out = Some(value()?.into()),
                "--content-packs" => {
                    out.content_packs = Some(
                        value()?
                            .split(',')
                            .map(str::trim)
                            .filter(|id| !id.is_empty())
                            .map(str::to_owned)
                            .collect(),
                    )
                }
                "--kind" => {
                    out.kind = Some(match value()?.as_str() {
                        "addon" => Kind::Addon,
                        "mod" => Kind::Mod,
                        other => return Err(format!("--kind is addon or mod, not '{other}'")),
                    })
                }
                flag if flag.starts_with("--") => return Err(format!("unknown {flag}\n{USAGE}")),
                path if out.path.is_none() => out.path = Some(path.into()),
                extra => return Err(format!("unexpected '{extra}'\n{USAGE}")),
            }
        }
        Ok(out)
    }

    fn path(&self) -> Result<&Path, String> {
        self.path.as_deref().ok_or_else(|| USAGE.to_owned())
    }

    fn content_packs(&self) -> BTreeSet<String> {
        self.content_packs
            .clone()
            .unwrap_or_else(petramond_world::assets::shipped_pack_ids)
    }
}

fn pack(args: &Args) -> Result<(), String> {
    let out = args.out.as_deref().ok_or_else(|| USAGE.to_owned())?;
    let bytes = archive::pack(args.path()?, args.wasm.as_deref())?;
    let checked = check(&bytes, &args.content_packs())?;
    let file = out.join(zip_name(&checked));
    std::fs::create_dir_all(out).map_err(|e| format!("could not create {}: {e}", out.display()))?;
    petramond_persist::atomic_file::replace(&file, &bytes)
        .map_err(|e| format!("could not write {}: {e}", file.display()))?;
    println!("{}", file.display());
    Ok(())
}

fn check_file(args: &Args) -> Result<(), String> {
    let path = args.path()?;
    let bytes = read(path)?;
    let checked = check(&bytes, &args.content_packs())?;
    println!(
        "{}: {} {} ({} bytes, sha256 {})",
        path.display(),
        checked.id,
        checked.version,
        bytes.len(),
        petramond::content::sha256_hex(&bytes)
    );
    Ok(())
}

fn install(args: &Args) -> Result<(), String> {
    let kind = args
        .kind
        .ok_or_else(|| format!("install needs --kind addon|mod\n{USAGE}"))?;
    let path = args.path()?;
    let dirs = Dirs::installed();
    let _lock = ContentLock::shared(&dirs).map_err(|e| format!("content lock: {e}"))?;
    let shipped = petramond_world::assets::shipped_pack_ids();
    let bytes = read(path)?;
    let checked = archive::check(&bytes, &shipped)?;

    // Rebuilding a local install before the game has applied the last one
    // replaces it; a change the content browser staged is left alone.
    if let Some(waiting) = install::pending(&dirs)
        .into_iter()
        .find(|c| c.dir == checked.id)
    {
        match &waiting.op {
            Op::Install { record, .. } if record.content_id.is_none() => {
                install::undo(&dirs, &waiting.dir);
                println!("replacing the local install of {} still waiting for a restart", checked.id);
            }
            _ => {
                return Err(format!(
                    "a change to {} from the content browser is waiting for a restart; start the game once, or undo it there",
                    checked.id
                ))
            }
        }
    }

    // Staging consumes its zip, and the build output must stay.
    std::fs::create_dir_all(dirs.staging()).map_err(|e| e.to_string())?;
    let copy = dirs.staging().join(format!(
        "{}-local-{}.zip.partial",
        checked.id,
        std::process::id()
    ));
    std::fs::write(&copy, &bytes)
        .map_err(|e| format!("could not stage {}: {e}", copy.display()))?;
    let offer = Offer {
        mod_id: checked.id.clone(),
        kind,
        content_id: None,
        name: checked.name,
        version: checked.version,
    };
    install::stage_install(&dirs, &copy, &offer, &shipped, &AtomicBool::new(false))?;
    println!(
        "staged {} {} into {}; the next start of Petramond installs it",
        offer.mod_id,
        offer.version,
        dirs.mods.display()
    );
    Ok(())
}

/// The archive reader's verdict, then admission as the game runs it on the
/// unpacked files: a pack the game would refuse at startup fails here.
fn check(bytes: &[u8], content_packs: &BTreeSet<String>) -> Result<Checked, String> {
    let checked = archive::check(bytes, content_packs)?;
    let scratch =
        std::env::temp_dir().join(format!("petramond-content-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let admitted = archive::Archive::open(bytes)
        .and_then(|a| a.extract(&scratch, &AtomicBool::new(false)))
        .and_then(|()| {
            petramond_world::assets::admit_pack_dir(&scratch)
                .map_err(|why| format!("not a valid pack: {why}"))
        });
    let _ = std::fs::remove_dir_all(&scratch);
    admitted.map(|_| checked)
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))
}

/// `<id>-<version>.zip`, the website's download name, with anything a file
/// name cannot hold in the version made `_`.
fn zip_name(checked: &Checked) -> String {
    let version: String = checked
        .version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if version.is_empty() {
        format!("{}.zip", checked.id)
    } else {
        format!("{}-{version}.zip", checked.id)
    }
}
