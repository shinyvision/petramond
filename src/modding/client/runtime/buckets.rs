use std::path::{Path, PathBuf};

pub fn local_session_key(world_dir_name: &str) -> String {
    format!("local:{world_dir_name}")
}

pub fn remote_session_key(server_identity: &str) -> String {
    format!("remote:{server_identity}")
}

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

#[cfg(any(test, feature = "test-support"))]
pub fn pack_files_dir_for_test(mod_id: &str) -> PathBuf {
    pack_storage_dir(mod_id).join("files")
}

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
}

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
        delete_local_world_storage_at(&base, "doomed").unwrap();
    }
}
