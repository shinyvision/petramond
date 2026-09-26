# Streaming worker (GOD-12, SEC-03, SIM-06, SIM-09, NET-03, NET-08)

Branch: `worktree-agent-ac0caee8f263edc0a`
Worktree: /home/rachel/build/petramond/.claude/worktrees/agent-ac0caee8f263edc0a
SEC-03 and the indexed queue landed in `2164216d`; SIM-09 landed in `63a2f520`.

Nothing has been compiled. Rule 4 of the brief forbade any cargo commands.

## Status per issue

- **SEC-03: DONE.**
  - `worker::ReportSlot<T>` makes every mesh and light job report exactly once. A job that panics, or that the pool drops before it starts, sends a failure value.
  - The job pool now contains panics with `run_contained`, in inline mode as well as on worker threads.
  - Mesh: `MeshDone.outcome` is now `MeshOutcome::{Built, Cancelled, Failed}`. On failure the drain frees the job's in-flight slot and cancel entry, and clears the section's dirty flag while keeping its old mesh.
  - Light: `LightBakeQueue::try_recv` now returns `LightBakeEvent::{Baked, Failed{pos, revision}}`. A failure frees the pending slot. `drain_light_bakes` then settles the section's light (`settle_failed_light_bake`) if the failure is current, or asks for a new bake if it is stale.
  - Tests are in worker/tests.rs, mesh_pool.rs, light/queue.rs and mesh_queue/tests.rs.
- **SIM-06: DONE.** The JobPool queue is indexed by key and submission sequence, so reprioritizing/removing waiting jobs costs O(k log n) without rebuilding the heap. Column generation has a 192-slot cap; section generation and disk-primary reads now share a 512-slot cap, with 64 new section admissions per load/poll phase. The streamer remembers when a bounded pass omitted wanted sections and refills nearest-first after draining results. Queued, no-longer-wanted section jobs are discarded on anchor moves, and only confirmed removals release their pending slots.
- **SIM-09: DONE (commit 63a2f520, fork).** Fluid checks have their own queue, capped at 2048 per tick with oldest-first carry-over. Reads go through SectionCursor. Announces are batched per section. The random-tick de-dup (part of NET-08) is fixed with a visited set.
  - The missing `LoadAnchors` reference in tick.rs now reads the existing primary and extra load-target fields, avoiding a dependency on the unstarted anchor redesign.
  - TickState API changed: `scheduled_set` and `scheduled_seq` are gone, `scheduled` is now a `ScheduledQueue`, and there is a new `fluid` queue. It also touched crates/petramond-world/src/world/tick_state.rs and its tests.rs.
- **GOD-12: NOT STARTED.** poll_inner has not been split.
- **NET-08: NOT STARTED** (apart from the tick.rs piece above). The design is worked out; see below.
- **NET-03: NOT STARTED.** The design is worked out; see below.

## Design for the remaining work (worked out, not implemented)

- Replace `WorldData.last_load_target` and `extra_load_targets` with a `LoadAnchors { targets, epoch }` type in petramond-world load_targets.rs. It would provide the min-over-anchors key helpers `nearest_to_column` and `nearest_to_section`.
  - Change `loaded_area()` to `loaded_area_near(cp)`. Today mob/spawn.rs uses player 1's disc for every player.
  - The fork was told this API exists. Its tick.rs code may already call `data.load_anchors.targets()` and `.set(..)`, and that will not compile until LoadAnchors lands.
- Add a `ColumnInterest` type to ServerWorld's gen state: reference counts over the union of all anchors' wanted and kept discs. It would be updated by pairing each anchor slot with its previous position and diffing the two discs, and would report which columns entered and which left the wanted and kept sets.
  - It would replace `column_wanted_by_any_target`, `within_current_keep_radius` and the settle check that currently looks only at the first anchor.
  - Unloading would come from columns that left the kept set, plus a per-column check of vertical windows limited to the moved anchor's old disc.
- Give each anchor its own admission lane: nearest-first heaps of that anchor's missing columns and queued sections, where queued sections are a per-column bitset of cy values. Lanes take turns (round robin) when admitting work, with caps: 192 columns and about 512 sections.
  - Only the lane of an anchor that moved gets rebuilt, which costs O(r²).
  - This would remove `missing_columns_settled` (and the line in store/evict.rs that sets it) and the reclaim scan.
- Unify the five section-request loops into one `request_sections(columns, scope)`, where scope is Full or a vertical window.
- NET-03: replace the global `bump_terrain_revision` with `note_terrain_changed(column)`, called from `note_section_loaded`/`unloaded`, `settle_stream_nonfinal`, light landing and materialize. The server would drain changed columns once per pump and route them to each connection whose keep shape covers them.
  - Each connection would re-plan only the columns routed to it, from a heap of pending sends. A full rescan of its own disc would happen only when that connection's own anchor moves.

## Merge notes

- Public API changes:
  - `MeshDone.mesh` is now `MeshDone.outcome`.
  - `LightBakeQueue::try_recv` returns `LightBakeEvent`, re-exported from `world::light`.
  - New `worker::ReportSlot`.
- Files touched outside the owned areas: none by the parent. Check the fork's commits for its own.
