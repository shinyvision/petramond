use super::*;
use crate::worker::JobPool;
use crate::world::WorldRole;

#[test]
fn moving_an_anchor_admits_near_columns_even_with_a_full_generation_queue() {
    for multiplayer in [false, true] {
        let pool = Arc::new(JobPool::new(1));
        let (release, wait) = std::sync::mpsc::channel();
        pool.submit(i64::MIN, move || {
            let _ = wait.recv();
        });
        let mut world = World::new_with_pool(0, 32, WorldRole::ServerHeadless, pool);
        for _ in 0..4 {
            world.update_load(0, 4, 0);
        }
        let was_full =
            world.gen.pending.len() == super::super::requests::MAX_PENDING_COLUMN_GEN_JOBS;
        let destination = ChunkPos::new(12, 0);
        let was_missing = !world.gen.pending.contains_key(&destination);
        if multiplayer {
            world.update_load_multi(&[
                LoadAnchor {
                    cx: 0,
                    cy: 4,
                    cz: 0,
                    radius: 32,
                },
                LoadAnchor {
                    cx: destination.cx,
                    cy: 4,
                    cz: destination.cz,
                    radius: 32,
                },
            ]);
        } else {
            world.update_load(destination.cx, 4, destination.cz);
        }
        let admitted = world.gen.pending.contains_key(&destination);
        let bounded =
            world.gen.pending.len() <= super::super::requests::MAX_PENDING_COLUMN_GEN_JOBS;
        let departed_released = if multiplayer {
            world.update_load(0, 4, 0);
            !world.gen.pending.contains_key(&destination)
        } else {
            true
        };
        for job in world.gen.pending.values().flatten() {
            job.cancel();
        }
        release.send(()).unwrap();
        assert!(
            was_full && was_missing,
            "the destination must start blocked by the backlog"
        );
        assert!(
            admitted && bounded,
            "moving an anchor must replace waiting distant work"
        );
        assert!(
            departed_released,
            "a departing anchor must release its queue priority"
        );
    }
}
