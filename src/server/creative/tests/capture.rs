use super::*;
use crate::schematic::SelectionBox;

#[test]
fn capture_completes_asynchronously_without_reading_later_world_edits() {
    let mut server = server();
    let pos = IVec3::new(8, 65, 8);
    server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone);
    let expected = CellData::capture(&server.world.snapshot_cell(pos).unwrap());
    server.sessions[0]
        .creative
        .pending
        .push_back(Pending::Action(CreativeAction::Capture {
            name: "Snapshot".into(),
            regions: vec![SelectionBox::between(pos.to_array(), pos.to_array()).unwrap()],
            include_air: false,
        }));
    server.tick_creative(0, &mut TickEvents::default());
    assert!(server.sessions[0].creative.capture.is_some());
    assert!(server.sessions[0].creative.replies.is_empty());
    server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Dirt);
    let deadline = std::time::Instant::now();
    while server.sessions[0].creative.capture.is_some() {
        server.tick_creative(0, &mut TickEvents::default());
        assert!(
            deadline.elapsed().as_secs_f32() < 2.0,
            "capture worker did not finish"
        );
        std::thread::yield_now();
    }
    use crate::schematic::share::{BlobReceiver, SchematicNotice, SchematicRequest};
    let mut captured = None;
    let mut receiver: Option<BlobReceiver> = None;
    let schematic = loop {
        for reply in server.sessions[0].creative.take_replies() {
            match reply {
                CreativeReply::Captured { digest } => captured = Some(digest),
                CreativeReply::Message(message) => panic!("{message}"),
            }
        }
        server.tick_schematics();
        for notice in server.sessions[0].schematic.take_notices() {
            let SchematicNotice::Blob(packet) = notice else {
                continue;
            };
            match receiver.as_mut() {
                None => {
                    let digest = captured.expect("the capture is announced before it streams");
                    receiver = Some(BlobReceiver::begin(&packet, digest, u64::MAX).unwrap());
                }
                Some(receiver) => receiver.receive(packet).unwrap(),
            }
        }
        if let Some(receiver) = receiver.as_mut() {
            if let Some(credit) = receiver.take_credit() {
                server.apply_schematic_request(0, SchematicRequest::Blob(credit));
            }
            if let Some(bytes) = receiver.finish() {
                break crate::schematic::archive::decode(&bytes.unwrap()).unwrap();
            }
        }
        assert!(deadline.elapsed().as_secs() < 3);
        std::thread::yield_now();
    };
    assert_eq!(schematic.cells().next().unwrap().data, &expected);
    assert_eq!(server.world.snapshot_cell(pos).unwrap().block, Block::Dirt);
}

#[test]
fn a_paste_names_its_design_and_holds_its_place_while_the_archive_arrives() {
    use crate::schematic::share::{BlobSender, SchematicNotice, SchematicRequest};
    let mut server = server();
    let pos = IVec3::new(9, 65, 9);
    let bytes: std::sync::Arc<[u8]> =
        crate::schematic::archive::encode_bare(&plan(data(Block::Stone)))
            .unwrap()
            .into();
    let digest = crate::schematic::store::digest(&bytes);
    server.request_placement(0, digest, pos.to_array(), 0);
    // An edit asked for after the paste waits behind it.
    server.sessions[0]
        .creative
        .try_enqueue(Pending::Action(CreativeAction::Undo));
    assert!(
        server.sessions[0]
            .schematic
            .take_notices()
            .contains(&SchematicNotice::Want { digest }),
        "the world lacks the design, so the server asks for it"
    );
    let mut upload = BlobSender::new(digest, bytes);
    let deadline = std::time::Instant::now();
    while server.world.snapshot_cell(pos).unwrap().block != Block::Stone {
        for packet in upload.packets() {
            server.apply_schematic_request(0, SchematicRequest::Blob(packet));
        }
        server.tick_schematics();
        for notice in server.sessions[0].schematic.take_notices() {
            if let SchematicNotice::Blob(crate::schematic::share::BlobPacket::Credit {
                count,
                ..
            }) = notice
            {
                assert!(upload.credit(count));
            }
        }
        server.tick_creative(0, &mut TickEvents::default());
        assert_eq!(
            server.sessions[0].creative.replies,
            Vec::new(),
            "nothing behind the paste ran ahead of it"
        );
        assert!(deadline.elapsed().as_secs() < 3, "the paste never landed");
        std::thread::yield_now();
    }
}
