use super::*;

fn col(cx: i32, cz: i32) -> ChunkPos {
    ChunkPos::new(cx, cz)
}

/// Near columns first, all in view.
fn by_distance(column: ChunkPos) -> UploadPriority {
    (0, (column.cx.abs() + column.cz.abs()) as u32)
}

/// Drain the queue as `sync_meshes` does, uploading everything that is
/// ready. Returns the uploaded columns in order.
fn run_frame(queue: &mut UploadQueue) -> Vec<ChunkPos> {
    queue.begin_frame();
    let mut drain = FrameDrain::default();
    let mut uploaded = Vec::new();
    while let Some((column, _)) = queue.next_ready(&mut drain) {
        queue.take(column);
        drain.record_upload();
        uploaded.push(column);
    }
    queue.finish(drain, by_distance);
    uploaded
}

fn dirty(queue: &mut UploadQueue, column: ChunkPos, revision: u64) {
    queue.mark_dirty(column, revision, || by_distance(column));
}

#[test]
fn a_dirty_column_waits_out_its_quiet_window_then_uploads_once() {
    let mut queue = UploadQueue::default();
    queue.begin_frame();
    dirty(&mut queue, col(0, 0), 1);
    // Level-triggered dirtiness re-reports the same revision every frame.
    dirty(&mut queue, col(0, 0), 1);
    let mut drain = FrameDrain::default();
    assert_eq!(queue.next_ready(&mut drain), None, "still inside its quiet window");
    queue.finish(drain, by_distance);
    assert!(!queue.is_empty());
    assert_eq!(run_frame(&mut queue), [col(0, 0)]);
    assert!(queue.is_empty());
    assert!(run_frame(&mut queue).is_empty());
}

#[test]
fn new_revisions_restart_the_quiet_window_but_not_the_deadline() {
    let mut queue = UploadQueue::default();
    let mut uploaded_on = None;
    for frame in 1..=10u64 {
        queue.begin_frame();
        if uploaded_on.is_none() {
            // A column whose siblings keep finishing: a new revision a frame.
            dirty(&mut queue, col(3, 3), frame);
        }
        let mut drain = FrameDrain::default();
        if let Some((column, revision)) = queue.next_ready(&mut drain) {
            assert_eq!((column, revision), (col(3, 3), frame));
            queue.take(column);
            drain.record_upload();
            uploaded_on = Some(frame);
        }
        queue.finish(drain, by_distance);
    }
    assert_eq!(
        uploaded_on,
        Some(1 + MAX_WAIT_FRAMES),
        "the deadline bounds the coalescing"
    );
}

#[test]
fn an_urgent_column_skips_the_wait() {
    let mut queue = UploadQueue::default();
    queue.begin_frame();
    dirty(&mut queue, col(1, 0), 1);
    dirty(&mut queue, col(2, 0), 1);
    queue.mark_urgent(col(2, 0), || by_distance(col(2, 0)));
    // Urgency for a column that is not pending is a no-op.
    queue.mark_urgent(col(9, 9), || by_distance(col(9, 9)));
    let mut drain = FrameDrain::default();
    assert_eq!(queue.next_ready(&mut drain), Some((col(2, 0), 1)));
    queue.take(col(2, 0));
    drain.record_upload();
    assert_eq!(queue.next_ready(&mut drain), None);
    queue.finish(drain, by_distance);
    assert_eq!(run_frame(&mut queue), [col(1, 0)]);
}

#[test]
fn columns_upload_in_view_first_then_nearest_then_by_position() {
    let mut queue = UploadQueue::default();
    queue.begin_frame();
    let hidden_near = col(0, 1);
    queue.mark_dirty(hidden_near, 1, || (1, 1));
    for column in [col(5, 0), col(-2, 0), col(2, 0), col(1, 0)] {
        dirty(&mut queue, column, 1);
    }
    assert_eq!(
        run_frame(&mut queue),
        [col(1, 0), col(-2, 0), col(2, 0), col(5, 0), hidden_near]
    );
}

#[test]
fn superseded_heap_entries_are_skipped() {
    let mut queue = UploadQueue::default();
    queue.begin_frame();
    for revision in 1..=5 {
        dirty(&mut queue, col(0, 0), revision);
    }
    let mut uploads = Vec::new();
    for _ in 0..3 {
        queue.begin_frame();
        let mut drain = FrameDrain::default();
        while let Some(ready) = queue.next_ready(&mut drain) {
            queue.take(ready.0);
            drain.record_upload();
            uploads.push(ready);
        }
        queue.finish(drain, by_distance);
    }
    assert_eq!(uploads, [(col(0, 0), 5)], "one upload, at the latest revision");
}

#[test]
fn a_frame_uploads_at_most_its_budget_and_keeps_the_rest() {
    let mut queue = UploadQueue::default();
    queue.begin_frame();
    let columns: Vec<_> = (0..(MAX_UPLOADS_PER_FRAME as i32 * 3 / 2))
        .map(|i| col(i, 0))
        .collect();
    for &column in &columns {
        dirty(&mut queue, column, 1);
    }
    // Past the quiet window: everything is ready at once.
    queue.begin_frame();
    let first = run_frame(&mut queue);
    assert_eq!(first.len(), MAX_UPLOADS_PER_FRAME);
    assert_eq!(first, columns[..MAX_UPLOADS_PER_FRAME], "nearest first");
    let second = run_frame(&mut queue);
    assert_eq!(second, columns[MAX_UPLOADS_PER_FRAME..]);
    assert!(queue.is_empty());
}

#[test]
fn a_restarted_column_waits_a_fresh_window() {
    let mut queue = UploadQueue::default();
    queue.begin_frame();
    dirty(&mut queue, col(4, 4), 7);
    queue.begin_frame();
    let mut drain = FrameDrain::default();
    let (column, revision) = queue.next_ready(&mut drain).expect("ready");
    // Its released siblings need re-meshing first.
    queue.take(column);
    queue.restart(column, revision, &mut drain);
    queue.finish(drain, by_distance);
    let mut drain = FrameDrain::default();
    assert_eq!(queue.next_ready(&mut drain), None, "same frame: still waiting");
    queue.finish(drain, by_distance);
    assert_eq!(run_frame(&mut queue), [col(4, 4)]);
}

#[test]
fn clearing_forgets_everything() {
    let mut queue = UploadQueue::default();
    queue.begin_frame();
    dirty(&mut queue, col(0, 0), 1);
    queue.clear();
    assert!(queue.is_empty());
    queue.begin_frame();
    assert!(run_frame(&mut queue).is_empty());
}
