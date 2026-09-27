//! The content library's disk and wire rules, on synthetic archives, rows and
//! directories. Nothing here touches the network or the player's data.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use serde_json::json;

use super::api::{self, DownloadError, ListingRow, Progress};
use super::archive::{self, Archive};
use super::install::{self, ContentLock, Dirs, Offer, Op};
use super::{records, Kind};
use crate::service::ServiceError;

struct Zent<'a> {
    name: &'a str,
    data: &'a [u8],
    deflate: bool,
    mode: u32,
    flags: u16,
}

fn file<'a>(name: &'a str, data: &'a [u8]) -> Zent<'a> {
    Zent {
        name,
        data,
        deflate: true,
        mode: 0o100644,
        flags: 0,
    }
}

/// A plain zip of `entries`, written the way any archiver would.
fn zip(entries: &[Zent]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let packed = if e.deflate {
            let mut enc =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(e.data).unwrap();
            enc.finish().unwrap()
        } else {
            e.data.to_vec()
        };
        let mut crc = flate2::Crc::new();
        crc.update(e.data);
        let method: u16 = if e.deflate { 8 } else { 0 };
        let offset = out.len() as u32;
        let fixed = |b: &mut Vec<u8>| {
            b.extend_from_slice(&20u16.to_le_bytes());
            b.extend_from_slice(&e.flags.to_le_bytes());
            b.extend_from_slice(&method.to_le_bytes());
            b.extend_from_slice(&[0; 4]);
            b.extend_from_slice(&crc.sum().to_le_bytes());
            b.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            b.extend_from_slice(&(e.data.len() as u32).to_le_bytes());
            b.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        fixed(&mut out);
        out.extend_from_slice(e.name.as_bytes());
        out.extend_from_slice(&packed);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&(3u16 << 8 | 20).to_le_bytes());
        fixed(&mut central);
        central.extend_from_slice(&[0; 6]);
        central.extend_from_slice(&(e.mode << 16).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(e.name.as_bytes());
    }
    let offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn scratch(name: &str) -> petramond_util::test_dirs::TestScratchDir {
    petramond_util::test_dirs::TestScratchDir::new(&format!("content-{name}"))
}

fn refusal(bytes: &[u8]) -> String {
    match Archive::open(bytes) {
        Ok(archive) => archive
            .extract(&scratch("refusal").join("out"), &AtomicBool::new(false))
            .expect_err("a hostile archive unpacked"),
        Err(why) => why,
    }
}

#[test]
fn a_pack_folder_unpacks_with_its_root_stripped() {
    let manifest = br#"{"id":"sample_pack","name":"Sample"}"#;
    let bytes = zip(&[
        file("sample_pack/pack.json", manifest),
        file("sample_pack/textures/a.txt", b"hello"),
        file("__MACOSX/sample_pack/._pack.json", b"junk"),
    ]);
    let unpack = scratch("unpack");
    let out = unpack.join("out");
    Archive::open(&bytes)
        .unwrap()
        .extract(&out, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(std::fs::read(out.join("pack.json")).unwrap(), manifest);
    assert_eq!(std::fs::read(out.join("textures/a.txt")).unwrap(), b"hello");
    assert!(!out.join("__MACOSX").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(out.join("pack.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0, "nothing unpacks executable");
    }
}

#[test]
fn hostile_archives_are_refused_with_a_reason() {
    let manifest: &[u8] = br#"{"id":"x","name":"X"}"#;
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (
            zip(&[file("pack.json", manifest), file("../evil", b"x")]),
            "outside its own folder",
        ),
        (
            zip(&[
                file("pack.json", manifest),
                Zent {
                    mode: 0o120777,
                    ..file("link", b"/etc/passwd")
                },
            ]),
            "symbolic link",
        ),
        (
            zip(&[
                file("pack.json", manifest),
                Zent {
                    flags: 1,
                    ..file("secret", b"x")
                },
            ]),
            "encrypted",
        ),
        (
            zip(&[
                file("pack.json", manifest),
                file("A.txt", b"1"),
                file("a.TXT", b"2"),
            ]),
            "two files named",
        ),
        (
            zip(&[file("pack.json", manifest), file("sub/CON.txt", b"x")]),
            "cannot store",
        ),
        (zip(&[file("readme.txt", b"x")]), "no pack.json"),
        (
            zip(&[file("a/pack.json", manifest), file("b/pack.json", manifest)]),
            "more than one pack",
        ),
        (
            zip(&[
                file("pack.json", manifest),
                file("zeros", &vec![0u8; 2 << 20]),
            ]),
            "expands far beyond",
        ),
    ];
    for (bytes, expected) in cases {
        let why = refusal(&bytes);
        assert!(why.contains(expected), "expected '{expected}', got '{why}'");
    }
    // An end record that does not end the file is one another reader might
    // not pick.
    let mut trailing = zip(&[file("pack.json", manifest)]);
    trailing.extend_from_slice(b"junk");
    assert!(refusal(&trailing).contains("damaged"));
}

fn row(mod_id: &str, bytes: &[u8]) -> ListingRow {
    ListingRow {
        content_id: 12,
        mod_id: mod_id.into(),
        kind: Kind::Addon,
        name: "Sample".into(),
        summary: String::new(),
        description: String::new(),
        version: "1.0.0".into(),
        byte_size: bytes.len() as u64,
        sha256: super::sha256_hex(bytes),
        download_path: "/api/v1/content/12/download".into(),
        icon_path: None,
    }
}

#[test]
fn a_listing_is_validated_row_by_row_and_its_text_cleaned() {
    let good = json!({
        "id": 12, "modId": "sample_pack", "kind": "addon", "byteSize": 100,
        "sha256": "A".repeat(64), "downloadPath": "/api/v1/content/12/download",
        "iconPath": "https://elsewhere.example/icon.png",
        "name": "Sam\u{7}ple", "description": "line one\nline two\u{1b}",
        "version": "1.0.0-very-long-suffix"
    });
    let rows = api::parse_listing(&json!({ "items": [
        good,
        { "modId": "Bad Id", "kind": "addon", "byteSize": 1, "sha256": "a".repeat(64),
          "downloadPath": "/api/v1/content/1/download" },
        { "modId": "later", "kind": "texture_pack", "byteSize": 1, "sha256": "a".repeat(64),
          "downloadPath": "/api/v1/content/2/download" },
        { "modId": "offsite", "kind": "mod", "byteSize": 1, "sha256": "a".repeat(64),
          "downloadPath": "https://evil.example/x.zip" },
        { "modId": "huge", "kind": "mod", "byteSize": 1u64 << 40, "sha256": "a".repeat(64),
          "downloadPath": "/api/v1/content/3/download" },
    ]}));
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.mod_id, "sample_pack");
    assert_eq!(row.name, "Sample");
    assert_eq!(row.description, "line one\nline two");
    assert_eq!(row.version.chars().count(), 12);
    assert_eq!(row.sha256, "a".repeat(64));
    assert_eq!(row.icon_path, None, "no host but the site's is followed");
}

#[test]
fn only_a_download_rate_limit_is_busy() {
    let busy = api::classify(
        429,
        &json!({"code": "too_many_downloads", "message": "wait"}),
        Some("7"),
    );
    assert_eq!(
        busy,
        ServiceError::Busy {
            retry_after: std::time::Duration::from_secs(7)
        }
    );
    assert!(matches!(
        api::classify(
            429,
            &json!({"code": "too_many_attempts", "message": "no"}),
            None
        ),
        ServiceError::Refused(_)
    ));
    assert!(matches!(
        api::classify(401, &json!({"signInRequired": true, "message": "m"}), None),
        ServiceError::SignInRequired(_)
    ));
    assert!(matches!(
        api::classify(503, &json!(null), None),
        ServiceError::Unreachable(_)
    ));
    let now = 1_790_000_000;
    let at = jiff::Timestamp::from_second(now + 42).unwrap();
    let date = jiff::fmt::rfc2822::DateTimePrinter::new()
        .timestamp_to_rfc9110_string(&at)
        .unwrap();
    assert_eq!(api::retry_wait(Some(&date), now).as_secs(), 42);
    assert_eq!(api::retry_wait(Some("99999"), now).as_secs(), 300);
    assert_eq!(api::retry_wait(Some("0"), now).as_secs(), 1);
    assert_eq!(api::retry_wait(Some("soon"), now).as_secs(), 10);
    assert_eq!(api::retry_wait(None, now).as_secs(), 10);
}

#[test]
fn only_exactly_the_promised_bytes_are_kept() {
    let dir = scratch("receive");
    let body = b"the archive bytes".to_vec();
    let promised = row("sample_pack", &body);
    let receive = |sent: &[u8], name: &str| {
        let into = dir.join(name);
        api::receive(
            sent,
            &promised,
            &into,
            &Progress::default(),
            &AtomicBool::new(false),
        )
    };
    assert_eq!(receive(&body, "whole"), Ok(()));
    assert!(matches!(
        receive(&body[..5], "short"),
        Err(DownloadError::Broken(_))
    ));
    let mut long = body.clone();
    long.push(b'!');
    assert!(matches!(
        receive(&long, "long"),
        Err(DownloadError::Broken(_))
    ));
    let mut swapped = body.clone();
    swapped[0] ^= 1;
    assert!(matches!(
        receive(&swapped, "swapped"),
        Err(DownloadError::Broken(_))
    ));
}

/// The scratch root, and the content dirs inside it.
fn dirs(name: &str) -> (petramond_util::test_dirs::TestScratchDir, Dirs) {
    let root = scratch(name);
    let dirs = Dirs {
        mods: root.join("mods"),
        content: root.join("content"),
    };
    std::fs::create_dir_all(&dirs.mods).unwrap();
    (root, dirs)
}

fn stage(dirs: &Dirs, id: &str, version: &str) {
    let manifest = format!(r#"{{"id":"{id}","name":"Sample","version":"{version}"}}"#);
    let bytes = zip(&[
        file(&format!("{id}/pack.json"), manifest.as_bytes()),
        file(&format!("{id}/notes.txt"), version.as_bytes()),
    ]);
    let at = dirs.staging().join(format!("{id}-{version}.zip.partial"));
    std::fs::create_dir_all(dirs.staging()).unwrap();
    std::fs::write(&at, &bytes).unwrap();
    let offer = Offer {
        version: version.into(),
        ..Offer::from(&row(id, &bytes))
    };
    install::stage_install(dirs, &at, &offer, &BTreeSet::new(), &AtomicBool::new(false)).unwrap();
    assert!(!at.exists(), "the download is consumed");
}

fn notes(dirs: &Dirs, id: &str) -> String {
    std::fs::read_to_string(dirs.mods.join(id).join("notes.txt")).unwrap_or_default()
}

#[test]
fn a_staged_install_lands_on_apply_and_an_update_replaces_it_whole() {
    let (_root, dirs) = dirs("apply");
    stage(&dirs, "sample_pack", "1");
    assert!(
        !dirs.mods.join("sample_pack").exists(),
        "nothing changes before a restart"
    );
    let report = install::apply_in(&dirs, &BTreeSet::new());
    assert_eq!(report.applied, ["sample_pack"]);
    assert_eq!(notes(&dirs, "sample_pack"), "1");
    let record = records::load(&dirs, "sample_pack").expect("recorded");
    assert!(records::valid(&dirs, &record));
    assert_eq!(record.kind, Kind::Addon);

    stage(&dirs, "sample_pack", "2");
    let report = install::apply_in(&dirs, &BTreeSet::new());
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(notes(&dirs, "sample_pack"), "2");
    assert!(
        std::fs::read_dir(dirs.staging()).unwrap().next().is_none(),
        "nothing is left in staging"
    );

    install::stage_remove(&dirs, "sample_pack").unwrap();
    install::apply_in(&dirs, &BTreeSet::new());
    assert!(!dirs.mods.join("sample_pack").exists());
    assert!(records::load(&dirs, "sample_pack").is_none());
}

#[test]
fn in_process_apply_waits_for_other_readers_then_changes_the_installed_set() {
    let (_root, dirs) = dirs("live-apply-lock");
    stage(&dirs, "sample_pack", "1");
    let mut owner = ContentLock::shared(&dirs).unwrap();
    let other = ContentLock::shared(&dirs).unwrap();
    let deferred = install::apply_pending_live(&dirs, &mut owner);
    assert!(deferred.deferred);
    assert!(deferred.applied.is_empty());
    assert!(!dirs.mods.join("sample_pack").exists());

    drop(other);
    assert!(owner.try_upgrade().unwrap());
    let outsider = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dirs.content.join("lock-gate"))
        .unwrap();
    assert!(matches!(
        outsider.try_lock_shared(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    owner.downgrade_in_place().unwrap();
    outsider.try_lock_shared().unwrap();
    outsider.unlock().unwrap();

    let applied = install::apply_pending_live(&dirs, &mut owner);
    assert_eq!(applied.applied, ["sample_pack"]);
    assert_eq!(notes(&dirs, "sample_pack"), "1");

    install::stage_remove(&dirs, "sample_pack").unwrap();
    let removed = install::apply_pending_live(&dirs, &mut owner);
    assert_eq!(removed.applied, ["sample_pack"]);
    assert!(!dirs.mods.join("sample_pack").exists());
}

#[test]
fn an_apply_cut_short_never_costs_the_player_their_pack() {
    let (_root, dirs) = dirs("interrupted");
    stage(&dirs, "sample_pack", "1");
    install::apply_in(&dirs, &BTreeSet::new());
    stage(&dirs, "sample_pack", "2");
    // A crash right after the old version was moved aside: the journal is
    // written, the old version sits in staging, and the stage is gone.
    let mut change = install::pending(&dirs).remove(0);
    let Op::Install { staged, .. } = &change.op else {
        panic!("an install");
    };
    std::fs::remove_dir_all(dirs.mods.join(staged)).unwrap();
    let old = dirs.staging().join("sample_pack.old-crash");
    change.journal = vec![(old.clone(), dirs.mods.join("sample_pack"))];
    std::fs::rename(dirs.mods.join("sample_pack"), &old).unwrap();
    std::fs::write(
        dirs.content.join("pending").join("sample_pack.json"),
        serde_json::to_vec(&change).unwrap(),
    )
    .unwrap();

    let report = install::apply_in(&dirs, &BTreeSet::new());
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(notes(&dirs, "sample_pack"), "1", "the old version is back");

    // An orphaned aside with its place empty goes back too.
    std::fs::rename(
        dirs.mods.join("sample_pack"),
        dirs.staging().join("sample_pack.old-orphan"),
    )
    .unwrap();
    install::apply_in(&dirs, &BTreeSet::new());
    assert_eq!(notes(&dirs, "sample_pack"), "1");
}

#[test]
fn a_removed_folder_takes_only_its_own_record_and_never_comes_back() {
    let (_root, dirs) = dirs("remove-stray");
    stage(&dirs, "sample_pack", "1");
    install::apply_in(&dirs, &BTreeSet::new());
    let copy = dirs.mods.join("sample_pack-copy");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::copy(
        dirs.mods.join("sample_pack/pack.json"),
        copy.join("pack.json"),
    )
    .unwrap();

    install::stage_remove(&dirs, "sample_pack-copy").unwrap();
    install::apply_in(&dirs, &BTreeSet::new());
    assert!(!copy.exists());
    assert!(
        records::load(&dirs, "sample_pack").is_some(),
        "the real pack of that id keeps its record"
    );

    // A removal whose deletion fails part way (a locked file) is finished
    // by a later start, never put back.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = dirs.mods.join("sample_pack/locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("held"), b"").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        install::stage_remove(&dirs, "sample_pack").unwrap();
        install::apply_in(&dirs, &BTreeSet::new());
        let stuck: Vec<PathBuf> = std::fs::read_dir(dirs.staging())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert!(!stuck.is_empty(), "the locked file held the deletion");
        for dir in &stuck {
            let _ = std::fs::set_permissions(
                dir.join("locked"),
                std::fs::Permissions::from_mode(0o755),
            );
        }
        install::apply_in(&dirs, &BTreeSet::new());
        assert!(
            !dirs.mods.join("sample_pack").exists(),
            "the removed pack stays removed"
        );
        assert!(stuck.iter().all(|d| !d.exists()), "{stuck:?}");
    }
}

#[test]
fn a_shipped_id_is_never_installed() {
    let (_root, dirs) = dirs("shipped");
    stage(&dirs, "forge", "1");
    let shipped = BTreeSet::from(["forge".to_owned()]);
    let report = install::apply_in(&dirs, &shipped);
    assert_eq!(report.failed.len(), 1);
    assert!(!dirs.mods.join("forge").exists());
    assert!(install::pending(&dirs).is_empty());
}

#[test]
fn an_icon_is_squared_and_pixel_art_stays_crisp() {
    let mut wide = image::RgbaImage::new(128, 64);
    for (x, _, p) in wide.enumerate_pixels_mut() {
        *p = if x % 2 == 0 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 255, 255])
        };
    }
    let mut png = Vec::new();
    wide.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let icon = super::icon::normalize(&png).unwrap();
    assert_eq!(icon.dimensions(), (64, 64));
    assert_eq!(icon.get_pixel(0, 0)[3], 0, "letterboxed, not stretched");
    let colors: BTreeSet<[u8; 4]> = (16..48).map(|y| icon.get_pixel(10, y).0).collect();
    assert_eq!(colors.len(), 1, "a halving is nearest-neighbour");
    assert!(super::icon::normalize(&vec![0u8; 600 * 1024]).is_err());
}

fn write_tree(root: &std::path::Path, files: &[(&str, &[u8])]) {
    for (name, data) in files {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }
}

#[test]
fn a_packed_folder_is_the_same_bytes_whatever_its_history_and_passes_check() {
    let manifest: &[u8] = br#"{"id":"sample_pack","name":"Sample","version":"1.0.0"}"#;
    let files: [(&str, &[u8]); 4] = [
        ("pack.json", manifest),
        ("icon.png", b"not really a png"),
        ("textures/b.txt", b"b"),
        ("textures/a/deep.txt", b"deep"),
    ];
    let first = scratch("pack-first");
    write_tree(&first, &files);
    let second = scratch("pack-second");
    let mut reversed = files;
    reversed.reverse();
    write_tree(&second, &reversed);
    std::fs::write(second.join("textures/b.txt"), b"b").unwrap();
    let guest = scratch("pack-wasm");
    let wasm = guest.join("guest.wasm");
    std::fs::write(&wasm, b"\0asm").unwrap();

    let packed = archive::pack(&first, Some(&wasm)).unwrap();
    assert_eq!(packed, archive::pack(&second, Some(&wasm)).unwrap());
    let names: Vec<&[u8]> = (0..packed.len() - 46)
        .filter(|&at| packed[at..at + 4] == 0x0201_4b50u32.to_le_bytes())
        .map(|at| {
            let len = u16::from_le_bytes([packed[at + 28], packed[at + 29]]) as usize;
            &packed[at + 46..at + 46 + len]
        })
        .collect();
    assert!(names.windows(2).all(|w| w[0] < w[1]), "entries are sorted");
    assert!(
        names.iter().all(|n| !n.ends_with(b"/")),
        "no folder entries"
    );

    let checked = archive::check(&packed, &BTreeSet::new()).unwrap();
    assert_eq!(checked.id, "sample_pack");
    assert_eq!(checked.version, "1.0.0");
    let roundtrip = scratch("pack-roundtrip");
    let out = roundtrip.join("out");
    Archive::open(&packed)
        .unwrap()
        .extract(&out, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(std::fs::read(out.join("mod.wasm")).unwrap(), b"\0asm");
    assert_eq!(
        std::fs::read(out.join("textures/a/deep.txt")).unwrap(),
        b"deep"
    );
}

#[test]
fn check_refuses_what_the_website_refuses() {
    let content_packs = BTreeSet::from(["forge".to_owned()]);
    let long_description = format!(
        r#"{{"id":"ok","name":"Ok","description":"{}"}}"#,
        "x".repeat(64 * 1024)
    );
    let cases: Vec<(String, &str)> = vec![
        (r#"{"id":"Studio","name":"S"}"#.into(), "needs an id"),
        (r#"{"name":"S"}"#.into(), "needs an id"),
        (
            format!(r#"{{"id":"{}","name":"S"}}"#, "a".repeat(65)),
            "needs an id",
        ),
        (
            r#"{"id":"forge","name":"S"}"#.into(),
            "belongs to a content pack",
        ),
        (r#"{"id":"ok","name":" \u0007 "}"#.into(), "needs a name"),
        (
            format!(r#"{{"id":"ok","name":"{}"}}"#, "n".repeat(129)),
            "128 characters",
        ),
        (
            format!(r#"{{"id":"ok","name":"N","version":"{}"}}"#, "1".repeat(33)),
            "32 characters",
        ),
        (long_description, "larger than 64 KB"),
        ("{ not json".into(), "not readable JSON"),
    ];
    for (manifest, expected) in cases {
        let bytes = zip(&[file("pack.json", manifest.as_bytes())]);
        let why =
            archive::check(&bytes, &content_packs).expect_err(&manifest[..40.min(manifest.len())]);
        assert!(why.contains(expected), "expected '{expected}', got '{why}'");
    }

    // A corrupt entry is refused even though the container is sound.
    let mut corrupt = zip(&[
        file("pack.json", br#"{"id":"ok","name":"Ok"}"#),
        Zent {
            deflate: false,
            ..file("notes.txt", b"hello")
        },
    ]);
    let at = corrupt.windows(5).position(|w| w == b"hello").unwrap();
    corrupt[at] = b'j';
    assert!(archive::check(&corrupt, &content_packs)
        .unwrap_err()
        .contains("damaged"));
}

/// The cross-repository check: a real website's listing, icon and download,
/// through the whole install pipeline, the startup apply, discovery and the
/// tier classifier, then an uninstall.
///
/// OPT-IN, like `account::tests::live_`: it needs a running website holding
/// the addon `PETRAMOND_TEST_ADDON` and the third-party mod
/// `PETRAMOND_TEST_MOD`, and it INSTALLS them into the data dir, so it
/// refuses to run without `PETRAMOND_DATA_DIR` pointing at a scratch dir.
///
/// ```text
/// PETRAMOND_DATA_DIR=/tmp/scratch PETRAMOND_ACCOUNT_URL=http://localhost:8044 \
///   PETRAMOND_TEST_ACCOUNT=someone PETRAMOND_TEST_PASSWORD=... \
///   PETRAMOND_TEST_ADDON=studio PETRAMOND_TEST_MOD=some_mod \
///   cargo test --profile fasttest -p petramond --lib content::tests::live_ -- --ignored
/// ```
#[test]
#[ignore = "needs a running website with an uploaded addon and mod"]
fn live_download_installs_by_tier_and_uninstall_removes_it() {
    use super::Tier;
    let env = |key: &str| std::env::var(key).unwrap_or_else(|_| panic!("set {key}"));
    let scratch = PathBuf::from(env("PETRAMOND_DATA_DIR"));
    let (addon, third_party) = (env("PETRAMOND_TEST_ADDON"), env("PETRAMOND_TEST_MOD"));
    let dirs = Dirs::installed();
    assert!(
        dirs.mods.starts_with(&scratch),
        "installs stay in the scratch dir"
    );

    crate::account::session::sign_in(
        &env("PETRAMOND_TEST_ACCOUNT"),
        &env("PETRAMOND_TEST_PASSWORD"),
    )
    .expect("sign in");
    let rows = api::listing().expect("the listing");
    let find = |id: &str| {
        rows.iter()
            .find(|r| r.mod_id == id)
            .unwrap_or_else(|| panic!("'{id}' is listed"))
            .clone()
    };
    let (addon_row, mod_row) = (find(&addon), find(&third_party));
    let both: BTreeSet<String> = [addon.clone(), third_party.clone()].into();
    let applied = |report: &install::ApplyReport| -> BTreeSet<String> {
        assert!(report.failed.is_empty() && !report.deferred, "{report:?}");
        report.applied.iter().cloned().collect()
    };
    assert_eq!((addon_row.kind, mod_row.kind), (Kind::Addon, Kind::Mod));

    let icons = super::dir().join("icons");
    let icon_path = addon_row
        .icon_path
        .as_deref()
        .expect("the addon ships an icon");
    let bytes = api::icon(icon_path).expect("the icon");
    super::icon::cache(&icons, &addon, &addon_row.sha256, &bytes).expect("the icon normalizes");
    assert!(super::icon::cache_path(&icons, &addon, &addon_row.sha256).is_file());

    let shipped = petramond_world::assets::shipped_pack_ids();
    let never = AtomicBool::new(false);
    for row in [&addon_row, &mod_row] {
        let progress = Progress::default();
        let partial = dirs.staging().join(format!("{}.zip.partial", row.mod_id));
        let zip = api::download(row, &partial, &progress, &never).expect("the download");
        assert_eq!(
            std::fs::read(&zip).map(|b| super::sha256_hex(&b)).unwrap(),
            row.sha256
        );
        assert_eq!(
            progress.done.load(std::sync::atomic::Ordering::Relaxed),
            row.byte_size
        );
        install::stage_install(&dirs, &zip, &Offer::from(row), &shipped, &never).expect("staged");
        assert!(
            !dirs.mods.join(&row.mod_id).exists(),
            "nothing lands before a restart"
        );
    }

    // "Restart": the startup apply, then discovery.
    let (report, lock) = install::apply_pending();
    assert_eq!(applied(&report), both);
    let pack = |id: &str| {
        petramond_world::assets::packs()
            .iter()
            .find(|p| p.id.as_deref() == Some(id))
            .unwrap_or_else(|| {
                panic!(
                    "'{id}' is discovered; refused: {:?}",
                    petramond_world::assets::refused()
                )
            })
    };
    let (addon_pack, mod_pack) = (pack(&addon), pack(&third_party));
    assert_eq!(
        addon_pack.origin,
        petramond_world::assets::PackOrigin::Installed
    );
    assert_eq!(super::tier(addon_pack), Tier::Addon, "the sheep badge");
    assert_eq!(
        super::tier(mod_pack),
        Tier::Mod,
        "no badge for a third party"
    );
    assert!(
        !super::held_off_at_first_sight().contains(&addon),
        "a presentation-only addon is never held off in an existing world"
    );

    for id in [&addon, &third_party] {
        install::stage_remove(&dirs, id).expect("removal staged");
    }
    drop(lock);
    let exclusive = ContentLock::try_exclusive(&dirs)
        .expect("the lock opens")
        .expect("no other Petramond process");
    let report = install::apply_in(&dirs, &shipped);
    drop(exclusive);
    assert_eq!(applied(&report), both);
    for id in [&addon, &third_party] {
        assert!(!dirs.mods.join(id).exists(), "'{id}' is gone");
        assert!(records::load(&dirs, id).is_none(), "'{id}' has no record");
    }
    assert!(install::pending(&dirs).is_empty());
    let left: Vec<_> = std::fs::read_dir(dirs.staging())
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "nothing is left behind: {left:?}");

    crate::account::session::sign_out();
}
