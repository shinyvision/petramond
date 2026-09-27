//! Where client-mod storage lives: one bucket per session identity (a local
//! world's directory, a server's address) holding each mod's WORLD data, and
//! one per mod for its PACK data.

use std::path::{Path, PathBuf};

/// The client-storage identity of a LOCAL world: its save-DIRECTORY name,
/// never the display name. A world's directory never changes after creation
/// (renames rewrite only `world.json`), so personal mod data — minimap
/// exploration, waypoints — follows a renamed world with zero migration.
pub fn local_session_key(world_dir_name: &str) -> String {
    format!("local:{world_dir_name}")
}

/// The client-storage identity of a remote server (its address string).
pub fn remote_session_key(server_identity: &str) -> String {
    format!("remote:{server_identity}")
}

/// The ONE bucket holding every client mod's sandboxed storage for a session
/// identity: `<base>/client_mod_data/<fnv1a64(session_key)>/<mod_id>/...` —
/// this is the `<mod_id>`s' parent, the unit world deletion removes.
fn session_storage_bucket(base: &Path, session_key: &str) -> PathBuf {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in session_key.bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x1_0000_0000_01b3);
    }
    base.join("client_mod_data").join(format!("{hash:016x}"))
}

pub(super) fn client_storage_dir(session_key: &str, mod_id: &str) -> PathBuf {
    session_storage_bucket(&petramond_util::paths::base_data_dir(), session_key).join(mod_id)
}

/// A mod's PACK bucket: `<base>/client_mod_data/packs/<mod_id>/`, beside the
/// session buckets (whose names are 16 hex digits, so never `packs`). It is
/// keyed by the mod alone, so it follows the pack across every world, the
/// shell and every presentation.
pub(super) fn pack_storage_dir(mod_id: &str) -> PathBuf {
    petramond_util::paths::base_data_dir()
        .join("client_mod_data")
        .join("packs")
        .join(mod_id)
}

#[cfg(any(test, feature = "test-support"))]
pub fn client_storage_dir_for_test(session_key: &str, mod_id: &str) -> PathBuf {
    client_storage_dir(session_key, mod_id)
}

/// Where mod `mod_id`'s pack-bucket files are.
#[cfg(any(test, feature = "test-support"))]
pub fn pack_files_dir_for_test(mod_id: &str) -> PathBuf {
    pack_storage_dir(mod_id).join("files")
}

/// Test seeding: write entries into a mod's session storage bucket through
/// the ordered worker, flushed before return — perf harnesses fabricate a
/// large explored world without driving real exploration.
#[cfg(any(test, feature = "test-support"))]
pub fn seed_client_storage_for_test(
    session_key: &str,
    mod_id: &str,
    entries: Vec<(String, Vec<u8>)>,
) {
    let mut storage =
        super::super::storage::ClientStorage::new(client_storage_dir(session_key, mod_id));
    let batch = entries.into_iter().map(|(k, v)| (k, Some(v))).collect();
    if let Err(error) = storage.set_many(batch) {
        panic!("seed client storage: {error}");
    }
    // Drop flushes the worker, so files exist when the test proceeds.
}

/// Delete every client mod's sandboxed storage for a LOCAL world — the
/// world-deletion hook. Exploration maps and waypoints live OUTSIDE the save
/// (personal data, keyed on the world's directory name); without this, a
/// future world reusing the deleted world's directory name would inherit
/// them — explored terrain from a dead seed on a supposed-to-be-black map.
/// Safe against in-flight writes: the storage worker drains synchronously on
/// session drop, and deletion is only reachable from the world-select menu.
pub fn delete_local_world_storage(world_dir_name: &str) -> std::io::Result<()> {
    delete_local_world_storage_at(&petramond_util::paths::base_data_dir(), world_dir_name)
}

fn delete_local_world_storage_at(base: &Path, world_dir_name: &str) -> std::io::Result<()> {
    let bucket = session_storage_bucket(base, &local_session_key(world_dir_name));
    match std::fs::remove_dir_all(bucket) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deleting a world's client-mod storage removes that world's WHOLE
    /// bucket — every mod's data, nothing of any other identity — and a
    /// world that never stored anything deletes cleanly. Guards the
    /// world-deletion hook against key-derivation drift: a renamed hash or
    /// session-key format that no longer matches what the runtime writes
    /// would silently orphan (or worse, miss) the data again.
    #[test]
    fn world_deletion_removes_exactly_its_own_storage_bucket() {
        let base = petramond_util::test_dirs::TestScratchDir::new("client-storage-delete");
        let dir_for = |session_key: &str, mod_id: &str| {
            session_storage_bucket(&base, session_key).join(mod_id)
        };
        for (key, mod_id) in [
            (local_session_key("doomed"), "minimap"),
            (local_session_key("doomed"), "othermod"),
            (local_session_key("kept"), "minimap"),
            (remote_session_key("play.example.org"), "minimap"),
        ] {
            let dir = dir_for(&key, mod_id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("blob"), b"tile").unwrap();
        }

        delete_local_world_storage_at(&base, "doomed").unwrap();
        assert!(
            !session_storage_bucket(&base, &local_session_key("doomed")).exists(),
            "the deleted world's bucket goes whole — every mod's data"
        );
        assert!(
            dir_for(&local_session_key("kept"), "minimap")
                .join("blob")
                .exists(),
            "another world's bucket is untouched"
        );
        assert!(
            dir_for(&remote_session_key("play.example.org"), "minimap")
                .join("blob")
                .exists(),
            "server buckets are untouched"
        );
        // Idempotent: a world with no client-mod data deletes cleanly.
        delete_local_world_storage_at(&base, "doomed").unwrap();
    }
}
