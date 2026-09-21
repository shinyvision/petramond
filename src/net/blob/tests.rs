use super::*;

const MAX: u64 = 1 << 20;

fn blob(len: usize) -> Arc<[u8]> {
    (0..len)
        .map(|i| (i * 31 % 251) as u8)
        .collect::<Vec<_>>()
        .into()
}

/// Pump a stream to completion, the receiver crediting what it consumed.
fn stream(sender: &mut BlobSender, receiver: &mut Option<BlobReceiver>) -> Result<Vec<u8>, String> {
    for _ in 0..1000 {
        for packet in sender.packets() {
            match receiver {
                None => *receiver = Some(BlobReceiver::begin(&packet, sender.digest(), MAX)?),
                Some(r) => r.receive(packet)?,
            }
        }
        let r = receiver.as_mut().expect("begun");
        if let Some(done) = r.finish() {
            return done;
        }
        if let Some(BlobPacket::Credit { count, .. }) = r.take_credit() {
            assert!(sender.credit(count));
        }
    }
    panic!("the stream never finished");
}

#[test]
fn a_stream_larger_than_its_window_arrives_whole() {
    let bytes = blob(PACKET_BYTES * WINDOW * 3 + 17);
    let mut sender = BlobSender::new(digest(&bytes), bytes.clone());
    let mut receiver = None;
    assert_eq!(stream(&mut sender, &mut receiver).unwrap(), bytes.to_vec());
    assert!(sender.finished());
}

#[test]
fn a_stream_is_refused_when_it_does_not_match_its_name() {
    let bytes = blob(1000);
    let other = digest(&blob(999));
    let mut sender = BlobSender::new(other, bytes);
    let mut receiver = None;
    assert!(stream(&mut sender, &mut receiver).is_err());

    let begin = BlobPacket::Begin {
        digest: other,
        len: 4,
    };
    assert!(
        BlobReceiver::begin(&begin, digest(&blob(3)), MAX).is_err(),
        "only what was asked for"
    );
    let mut receiver = BlobReceiver::begin(&begin, other, MAX).unwrap();
    assert!(
        receiver
            .receive(BlobPacket::Data {
                digest: other,
                bytes: vec![0; 5],
            })
            .is_err(),
        "no more bytes than declared"
    );
}

#[test]
fn a_sender_never_exceeds_the_credit_it_was_granted() {
    let bytes = blob(PACKET_BYTES * 10);
    let mut sender = BlobSender::new(digest(&bytes), bytes);
    let data = |packets: Vec<BlobPacket>| {
        packets
            .iter()
            .filter(|p| matches!(p, BlobPacket::Data { .. }))
            .count()
    };
    assert_eq!(data(sender.packets()), WINDOW);
    assert_eq!(data(sender.packets()), 0);
    assert!(sender.credit(2));
    assert_eq!(data(sender.packets()), 2);
    assert!(
        !sender.credit(WINDOW + 1),
        "a grant beyond the window is refused"
    );
}

#[test]
fn a_stream_declaring_an_absurd_length_is_refused_at_begin() {
    let bytes = blob(10);
    let name = digest(&bytes);
    let begin = |len| BlobPacket::Begin { digest: name, len };
    assert!(BlobReceiver::begin(&begin(10), name, MAX).is_ok());
    assert!(BlobReceiver::begin(&begin(MAX + 1), name, MAX).is_err());
    assert!(BlobReceiver::begin(&begin(u64::MAX), name, MAX).is_err());
}
