use super::*;
use crate::schematic::{CellData, SchematicCell};

fn archive_bytes(name: &str) -> Vec<u8> {
    let png = petramond_world::assets::read_bytes("textures/schematic_wand.png")
        .unwrap()
        .0;
    let schematic = Schematic::from_cells(
        name.into(),
        [1; 3],
        vec![SchematicCell {
            pos: [0; 3],
            data: CellData {
                block: "petramond:stone".into(),
                state: Vec::new(),
                state_ids: Default::default(),
                fluid: 0,
                kv: Default::default(),
                container: None,
                furnace: None,
            },
        }],
    )
    .unwrap();
    archive::encode(&schematic, &png).unwrap()
}

fn wait_published(store: &mut Store) -> Result<Digest, String> {
    let deadline = std::time::Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
    loop {
        if let Some(Finished::Published(result)) = store.poll().pop() {
            return result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "publication never finished"
        );
        std::thread::yield_now();
    }
}

fn wait_ready(store: &mut Store, digest: &Digest) -> Arc<Asset> {
    let deadline = std::time::Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
    loop {
        match store.lookup(digest) {
            Lookup::Ready(asset) => return asset,
            Lookup::Loading => std::thread::yield_now(),
            Lookup::Missing => panic!("the asset is missing"),
            Lookup::Failed(error) => panic!("the asset failed: {error}"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "decode never finished"
        );
    }
}

#[test]
fn a_published_asset_outlives_the_store_that_received_it() {
    let dir = std::env::temp_dir().join(format!(
        "petramond-schematic-store-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let bytes = archive_bytes("Store fixture");
    let id = digest(&bytes);
    let mut store = Store::new(Some(&dir));
    assert!(matches!(store.lookup(&id), Lookup::Missing));
    store.publish(id, bytes.clone());
    assert_eq!(wait_published(&mut store), Ok(id));

    let mut reopened = Store::new(Some(&dir));
    assert!(reopened.contains(&id));
    assert_eq!(
        wait_ready(&mut reopened, &id).schematic.name,
        "Store fixture"
    );
    assert_eq!(&*reopened.read_bytes(&id).unwrap(), &bytes[..]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bytes_that_do_not_match_their_digest_are_never_published() {
    let mut store = Store::new(None);
    let bytes = archive_bytes("Honest");
    let claimed = digest(&archive_bytes("Other"));
    store.publish(claimed, bytes.clone());
    assert!(wait_published(&mut store).is_err());
    assert!(!store.contains(&claimed));

    let mut damaged = bytes;
    let last = damaged.len() - 1;
    damaged[last] ^= 0xff;
    let id = digest(&damaged);
    store.publish(id, damaged);
    assert!(
        wait_published(&mut store).is_err(),
        "an archive that does not decode is refused even under its own digest"
    );
    assert!(!store.contains(&id));
}
