//! Throwaway end-to-end streaming profiler. Drives the REAL `World` streaming
//! pipeline (the per-section worker pool + rayon meshing + the async light pool) and
//! separates the wall-clock cost of generation vs meshing+lighting, so we can see
//! where the live runtime cost actually goes — not just isolated gen microbenchmarks.
//!
//! Run: `cargo run --release --bin streamprofile [render_dist] [disc|view]`

use std::time::{Duration, Instant};

use llamacraft::tooling::stream::World;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(12);
    let view_biased = args.get(2).is_some_and(|s| s == "view");
    let seed = 0x1234_5678u32;

    // Player at the surface: section cy 8 (world y ~128) so the vertical window covers
    // the surface band, exactly like a freshly-spawned player.
    let mut world = World::new(seed, r);
    let cam_cy = 8;
    llamacraft::perf::reset_all();

    // --- Phase 1: GENERATION (poll-only, no meshing). Wall-clock to stream the whole
    //     disc of columns + their vertical windows through the worker pool. ---
    let t_gen = Instant::now();
    if view_biased {
        world.update_load_facing(0, cam_cy, 0, 1.0, 0.0);
    } else {
        world.update_load(0, cam_cy, 0);
    }
    let mut settled = 0;
    let mut last = 0usize;
    let mut polls = 0u64;
    loop {
        world.poll();
        polls += 1;
        let now = world.loaded_section_count();
        if now == last && now > 0 {
            settled += 1;
            if settled >= 200 {
                break;
            }
        } else {
            settled = 0;
            last = now;
        }
        std::thread::sleep(Duration::from_micros(200));
        if t_gen.elapsed() > Duration::from_secs(120) {
            eprintln!("gen phase timed out");
            break;
        }
    }
    let gen_ms = t_gen.elapsed().as_secs_f64() * 1e3;
    let columns = world.loaded_column_count();
    let sections = world.loaded_section_count();

    // --- Phase 2: MESHING + LIGHTING (tick_mesh_budget-only). Wall-clock to mesh every
    //     loaded section, including the async skylight/block-light bakes it waits on. ---
    let t_mesh = Instant::now();
    let mut idle = 0;
    let mut last_meshes = 0usize;
    loop {
        world.tick_mesh_budget(4096);
        // Meshing is async: the dirty queue empties when jobs are SUBMITTED, so also wait
        // for the mesh count to stop growing (the pool has drained).
        let now = world.mesh_count();
        if !world.has_dirty_meshes() && now == last_meshes {
            idle += 1;
            if idle >= 50 {
                break;
            }
        } else {
            idle = 0;
            last_meshes = now;
        }
        std::thread::sleep(Duration::from_micros(200));
        if t_mesh.elapsed() > Duration::from_secs(120) {
            eprintln!("mesh phase timed out");
            break;
        }
    }
    let mesh_ms = t_mesh.elapsed().as_secs_f64() * 1e3;
    let meshes = world.mesh_count();

    println!(
        "render_dist {r}, seed {seed:08x}, player section cy {cam_cy}, mode {}",
        if view_biased { "view" } else { "disc" }
    );
    println!("loaded: {columns} columns, {sections} sections, {meshes} meshes ({polls} polls)");
    println!();
    println!(
        "GENERATION (worker pool)   : {gen_ms:>8.1} ms wall   ({:.3} ms/section, {:.2} ms/column)",
        gen_ms / sections.max(1) as f64,
        gen_ms / columns.max(1) as f64
    );
    println!(
        "MESH + LIGHT (rayon+pool)  : {mesh_ms:>8.1} ms wall   ({:.3} ms/section)",
        mesh_ms / sections.max(1) as f64
    );
    println!(
        "TOTAL                      : {:>8.1} ms wall",
        gen_ms + mesh_ms
    );

    // Per-stage CPU time (summed across all worker threads) + the unit COUNT — this is
    // what exposes the cubic per-section multiplication vs the old per-column work.
    let (gc_ns, gc_n) = llamacraft::perf::GEN_COLUMN.snapshot();
    let (gs_ns, gs_n) = llamacraft::perf::GEN_SECTION.snapshot();
    let (l_ns, l_n) = llamacraft::perf::LIGHT.snapshot();
    let (m_ns, m_n) = llamacraft::perf::MESH.snapshot();
    let ms = |ns: u64| ns as f64 / 1e6;
    let per = |ns: u64, n: u64| {
        if n > 0 {
            ns as f64 / 1e6 / n as f64
        } else {
            0.0
        }
    };
    let cols = columns.max(1) as f64;
    println!();
    println!("=== CPU time per stage (summed across worker threads) ===");
    println!(
        "gen column : {:>9.1} ms  {:>6} jobs  {:.3} ms/job  ({:.2} ms/column)",
        ms(gc_ns),
        gc_n,
        per(gc_ns, gc_n),
        ms(gc_ns) / cols
    );
    println!(
        "gen section: {:>9.1} ms  {:>6} jobs  {:.3} ms/job  ({:.2} ms/column)",
        ms(gs_ns),
        gs_n,
        per(gs_ns, gs_n),
        ms(gs_ns) / cols
    );
    println!(
        "light bake : {:>9.1} ms  {:>6} jobs  {:.3} ms/job  ({:.2} ms/column)",
        ms(l_ns),
        l_n,
        per(l_ns, l_n),
        ms(l_ns) / cols
    );
    println!(
        "mesh build : {:>9.1} ms  {:>6} jobs  {:.3} ms/job  ({:.2} ms/column)",
        ms(m_ns),
        m_n,
        per(m_ns, m_n),
        ms(m_ns) / cols
    );
    let total_cpu = gc_ns + gs_ns + l_ns + m_ns;
    println!(
        "TOTAL CPU  : {:>9.1} ms  = {:.2} ms/column over {} columns",
        ms(total_cpu),
        ms(total_cpu) / cols,
        columns
    );
}
