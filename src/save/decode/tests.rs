use super::*;

#[test]
fn decoders_publish_in_request_order_and_shutdown_drains_accepted_jobs() {
    let (tx, rx) = mpsc::channel();
    let (columns, _) = mpsc::channel();
    let mut decoders = Decoders::new(tx, columns);
    for x in 0..32 {
        decoders.submit(DecodeJob::Section {
            pos: SectionPos::new(x, 0, 0),
            store: SectionStore::ExploredCache,
            bytes: None,
        });
    }
    drop(decoders);
    let results: Vec<_> = rx.try_iter().collect();
    assert_eq!(results.len(), 32);
    for (x, result) in results.iter().enumerate() {
        assert_eq!(result.pos.cx, x as i32);
        assert!(result.section.is_none());
    }
}
