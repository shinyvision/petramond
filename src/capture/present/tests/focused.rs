//! One mechanism per test: a drag back lands from memory, applies land in
//! issue order, the window keeps resident only what it covers, and a
//! positioned write invalidates only what it overlaps.

use super::*;

/// A drag back over pieces presented moments ago needs no read: it is
/// prepared and landed in the frame it was issued in, and reinstalls only
/// the sections that differ.
#[test]
fn an_apply_reinstalls_only_what_differs_and_lands_from_memory_in_the_same_frame() {
    let h = harness("present-drag", 4, 30, 0x5DEE_CE66_D1CE_4E5B, 1000);
    let pool = Arc::new(JobPool::new(2));
    let mut p = Presentation::new(crate::net::remap::local_name_tables(), pool);
    let mut replica = replica();
    p.set_window(window_over(4));
    let (state, events) = seek(&h, None, 22.25);
    p.push(Op::Apply {
        id: 1,
        state,
        events: vec![events],
        at: 22.25,
    });
    settle(&mut p, &mut replica);
    let (state, events) = seek(&h, Some(23), 23.25);
    p.push(Op::Apply {
        id: 2,
        state,
        events: vec![events],
        at: 23.25,
    });
    settle(&mut p, &mut replica);
    let before = resident_arcs(&replica);

    let (state, events) = seek(&h, Some(24), 22.25);
    p.push(Op::Apply {
        id: 3,
        state,
        events: vec![events],
        at: 22.25,
    });
    let out = p.drive(&mut replica, 1.0 / 60.0);
    assert_eq!(
        out.landed,
        vec![3],
        "landed in the frame it was issued in (it asked for {:?})",
        p.requested
    );
    assert!(p.requested.is_empty(), "no read was asked for");

    let after = resident_arcs(&replica);
    // Tick 24's writes are what the seek back undoes.
    let written: BTreeSet<(i32, i32, i32)> = h.session.restated[23..24]
        .iter()
        .flat_map(|r| r.pieces.iter())
        .filter_map(|(k, _)| match k {
            ClientStateKey::Section([x, y, z]) => Some((*x, *y, *z)),
            _ => None,
        })
        .collect();
    let swapped: BTreeSet<(i32, i32, i32)> = before
        .iter()
        .filter(|(k, prior)| !Arc::ptr_eq(&prior.section, &after[k].section))
        .map(|(k, _)| *k)
        .collect();
    assert!(!written.is_empty());
    for k in &swapped {
        assert!(
            written.iter().any(|w| (w.0 - k.0).abs() <= 1
                && (w.1 - k.1).abs() <= 1
                && (w.2 - k.2).abs() <= 1),
            "section {k:?} reinstalled though nothing near it changed"
        );
    }
    let mut naive = Naive::new();
    let played = h.decoded_frames(1, 23);
    naive.queue = played.into_iter().collect();
    naive.time(22.25);
    assert_eq!(
        first_difference(&world_terrain(&replica), &world_terrain(&naive.world)),
        None
    );
}

/// Applies land in issue order, each onto the world the one before it left;
/// a cancelled one never lands and the next lands onto the world before it.
#[test]
fn applies_land_in_issue_order_and_a_cancelled_one_never_lands() {
    let h = harness("present-order", 3, 30, 0xA076_1D64_78BD_642F, 1000);
    let pool = Arc::new(JobPool::new(2));
    let mut p = Presentation::new(crate::net::remap::local_name_tables(), pool);
    let mut replica = replica();
    p.set_window(window_over(3));
    let (state, events) = seek(&h, None, 10.25);
    p.push(Op::Apply {
        id: 1,
        state,
        events: vec![events],
        at: 10.25,
    });
    // Relative: events only, continuing from what apply 1 leaves.
    p.push(Op::Apply {
        id: 2,
        state: Vec::new(),
        events: vec![h.frames_from(12, 3)],
        at: 13.25,
    });
    // An absolute jump elsewhere, cancelled before it lands.
    let (state, events) = seek(&h, None, 25.25);
    p.push(Op::Apply {
        id: 3,
        state,
        events: vec![events],
        at: 25.25,
    });
    assert!(p.cancel(3));
    let out = settle(&mut p, &mut replica);
    assert_eq!(out.landed, vec![1, 2]);
    assert_eq!(p.applied(), 2);
    assert!(!p.cancel(2), "a landed apply cannot be cancelled");
    let mut naive = Naive::new();
    naive.queue = h.decoded_frames(1, 14).into_iter().collect();
    naive.time(13.25);
    assert_eq!(
        first_difference(&world_terrain(&replica), &world_terrain(&naive.world)),
        None
    );
    assert_eq!(p.position(), 13.25);
}

/// Only the window is resident: a stated column outside it holds no
/// section in the replica, and moving the window reads it in.
#[test]
fn the_window_keeps_resident_only_the_stated_columns_it_covers() {
    let h = harness("present-window", 4, 12, 0x2545_F491_4F6C_DD1D, 1000);
    let pool = Arc::new(JobPool::new(2));
    let mut p = Presentation::new(crate::net::remap::local_name_tables(), pool);
    let mut replica = replica();
    let (state, events) = seek(&h, None, 5.25);
    p.push(Op::Apply {
        id: 1,
        state,
        events: vec![events],
        at: 5.25,
    });
    settle(&mut p, &mut replica);
    assert!(
        replica.data().sections.is_empty(),
        "no window, nothing resident"
    );
    let everything = presented_terrain(&p, &replica, &h.files);

    let corner = Some(Window {
        center: ChunkPos::new(0, 0),
        radius: 1,
    });
    p.set_window(corner);
    settle(&mut p, &mut replica);
    let resident: BTreeSet<ChunkPos> = replica.data().columns.keys().copied().collect();
    let expected: BTreeSet<ChunkPos> = [(0, 0), (1, 0), (0, 1)]
        .into_iter()
        .map(|(x, z)| ChunkPos::new(x, z))
        .collect();
    assert_eq!(resident, expected);
    let far = Window {
        center: ChunkPos::new(3, 3),
        radius: 1,
    };
    p.set_window(Some(far));
    settle(&mut p, &mut replica);
    assert!(replica.data().columns.keys().all(|c| far.contains(*c)));
    assert!(replica.data().columns.contains_key(&ChunkPos::new(3, 3)));
    assert_eq!(
        first_difference(&presented_terrain(&p, &replica, &h.files), &everything),
        None,
        "moving the window changes where the world is held, never what it is"
    );
}

/// A positioned write drops exactly the origins and cached pieces it
/// overlaps: the key it hit is read again, every other one is still known.
#[test]
fn a_positioned_write_invalidates_only_the_pieces_it_overlaps() {
    let h = harness("present-invalidate", 2, 6, 0x9FB2_1C65_1E98_DF25, 1000);
    let pool = Arc::new(JobPool::new(2));
    let mut p = Presentation::new(crate::net::remap::local_name_tables(), pool);
    let mut replica = replica();
    p.set_window(window_over(2));
    let (state, events) = seek(&h, None, 3.25);
    p.push(Op::Apply {
        id: 1,
        state,
        events: vec![events],
        at: 3.25,
    });
    settle(&mut p, &mut replica);
    let full = &h.session.snapshots[0];
    let (hit_key, hit) = full
        .pieces
        .iter()
        .find(|(k, _)| matches!(k, ClientStateKey::Section(_)))
        .copied()
        .unwrap();
    let hit_sp = match hit_key {
        ClientStateKey::Section([x, y, z]) => SectionPos::new(x, y, z),
        _ => unreachable!(),
    };
    let origins_before: Vec<(SectionPos, Option<PieceRange>)> = replica
        .data()
        .sections
        .keys()
        .map(|&sp| (sp, replica.origin_of(crate::world::Resident::Section(sp))))
        .collect();
    assert!(
        replica.origin_of(crate::world::Resident::Section(hit_sp)) == Some(h.range(hit)),
        "the section holds the piece it was stated by"
    );
    // The same bytes, written back in place: a positioned write all the same.
    let (tx, rx) = std::sync::mpsc::channel();
    let bytes = h.session.state.bytes[hit[0] as usize..(hit[0] + hit[1]) as usize].to_vec();
    crate::modding::client::files::write(
        &h.state_file.file,
        hit[0] + 1,
        bytes[1..2].to_vec(),
        false,
        move |r| {
            let _ = tx.send(r.map(|_| ()));
        },
    )
    .unwrap();
    rx.recv().unwrap().unwrap();
    p.drive(&mut replica, 1.0 / 60.0);
    for (sp, origin) in origins_before {
        let now = replica.origin_of(crate::world::Resident::Section(sp));
        if sp == hit_sp {
            assert_eq!(now, None, "the write overlapped this piece");
        } else {
            assert_eq!(now, origin, "section {sp:?} kept its origin");
        }
    }
}

/// A frame applying two batches is due at its newest: before then, with it
/// read and waiting, nothing the position needs is missing and the
/// presentation is ready. Not ready is only a frame not read yet.
#[test]
fn a_frame_of_two_batches_waiting_for_its_due_leaves_the_presentation_ready() {
    let h = harness("present-ready", 2, 4, 0x9E37_79B9_7F4A_7C15, 1000);
    let pool = Arc::new(JobPool::new(1));
    let mut p = Presentation::new(crate::net::remap::local_name_tables(), pool);
    let [at, len] = h.session.frames[0];
    p.queue.push(&[FileRanges {
        file: h.events_file.clone(),
        ranges: vec![[at, len]],
    }]);
    p.position = 10.25;
    p.released_through = Some(10);
    assert!(!p.ready(), "the next frame is not read yet");

    let record = PieceRange {
        incarnation: h.events_file.incarnation,
        offset: at,
        len,
    };
    p.frames.insert(
        (record.incarnation, record.offset),
        Ok(Arc::new(Frame {
            record,
            presented_tick: 11.5,
            due: 12,
            batches: vec![11, 12],
            items: Vec::new(),
        })),
    );
    assert!(p.ready(), "batch 11 comes with the frame due at 12");
}

/// The view samples of frames not yet due are handed out ahead of them, up
/// to the first past the next tick: a frame that applies a batch waits for
/// that tick, and the captured view would otherwise hold still, then jump.
#[test]
fn view_samples_ahead_of_the_position_come_from_frames_already_read() {
    let h = harness("present-views-ahead", 2, 6, 0x2545_F491_4F6C_DD1D, 1000);
    let pool = Arc::new(JobPool::new(1));
    let mut p = Presentation::new(crate::net::remap::local_name_tables(), pool);
    let mut replica = replica();
    let cue = |at: f64| {
        FrameItem::View(Box::new(crate::capture::view::ViewCue {
            at,
            pos: petramond_math::world_pos::WorldPos::new(0.0, 70.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            fov_y: 1.2,
            shake: Default::default(),
            motion: Default::default(),
            hands: Default::default(),
            hotbar: 0,
            animator: Default::default(),
            events: Vec::new(),
        }))
    };
    // Three queued frames, each holding one sample; due at 12, 12 and 13.
    let mut spans = Vec::new();
    for (i, (due, at)) in [(12, 10.4), (12, 11.1), (13, 11.4)].into_iter().enumerate() {
        let [offset, len] = h.session.frames[i];
        let record = PieceRange {
            incarnation: h.events_file.incarnation,
            offset,
            len,
        };
        p.frames.insert(
            (record.incarnation, offset),
            Ok(Arc::new(Frame {
                record,
                presented_tick: at,
                due,
                batches: vec![due],
                items: vec![cue(at)],
            })),
        );
        spans.push([offset, len]);
    }
    let [first, ..] = spans[0];
    let [last, last_len] = spans[2];
    p.queue.push(&[FileRanges {
        file: h.events_file.clone(),
        ranges: vec![[first, last + last_len - first]],
    }]);
    p.position = 10.5;
    let mut out = crate::capture::present::DriveOut::default();
    p.release(&mut replica, &mut out);
    let ahead: Vec<f64> = out.views.iter().map(|v| v.at).collect();
    assert_eq!(ahead, [10.4, 11.1], "up to the first sample past tick 11");
    assert_eq!(
        p.queue.front().map(|s| s.offset),
        Some(first),
        "nothing was released"
    );
}
