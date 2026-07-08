//! Throwaway end-to-end streaming profiler (perf session scratch, do not commit).
//! Drives the REAL `World` streaming pipeline (per-section worker pool + async mesh
//! pool + async light pool) and separates wall-clock generation vs meshing+lighting.
//!
//! Run: `cargo run --release --bin streamprofile [render_dist]`

use std::time::{Duration, Instant};

use petramond::tooling::stream::{stage_stats, World};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(12);
    let seed = 0x1234_5678u32;
    // argv[3] = "conc": skip the split phases and run ONLY the game-like concurrent
    // load, so thermal preheating from the earlier phases doesn't confound it.
    let concurrent_only = args.get(3).is_some_and(|s| s == "conc");

    // Default camera section cy 8 (world y ~128) loads the surface band from above;
    // pass argv[2]=4 for a grounded player whose window reaches below-surface depth.
    let cam_cy: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);
    if concurrent_only {
        run_concurrent(seed, r, cam_cy);
        return;
    }
    let mut world = World::new(seed, r);

    // --- Phase 1: GENERATION (poll-only, no meshing). ---
    let t_gen = Instant::now();
    world.update_load(0, cam_cy, 0);
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

    // --- Phase 2: MESHING + LIGHTING (tick_mesh_budget-only). ---
    let t_mesh = Instant::now();
    let mut idle = 0;
    let mut last_meshes = 0usize;
    loop {
        world.tick_mesh_budget(4096);
        world.poll();
        let now = world.iter_meshes().count();
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
    let meshes = world.iter_meshes().count();
    let verts: usize = world
        .iter_meshes()
        .map(|(_, m)| m.opaque.len() + m.transparent.len() + m.model.len())
        .sum();

    println!("render_dist {r}, seed {seed:08x}, player section cy {cam_cy}");
    println!("loaded: {columns} columns, {sections} sections, {meshes} meshes ({polls} polls)");
    println!("total mesh vertices: {verts}");
    let (deep, vis, parked) = world.deep_visibility_counts();
    println!("deep sections: {deep} ({vis} visible, {parked} parked hidden)");
    println!(
        "GENERATION (worker pool)   : {gen_ms:>8.1} ms wall   ({:.3} ms/section, {:.2} ms/column)",
        gen_ms / sections.max(1) as f64,
        gen_ms / columns.max(1) as f64
    );
    println!(
        "MESH + LIGHT (async pools) : {mesh_ms:>8.1} ms wall   ({:.3} ms/section)",
        mesh_ms / sections.max(1) as f64
    );
    println!(
        "TOTAL                      : {:>8.1} ms wall",
        gen_ms + mesh_ms
    );

    let (mesh_ns, mesh_jobs, light_ns, light_jobs) = stage_stats();
    let per = |ns: u64, n: u64| ns as f64 / 1e6 / n.max(1) as f64;
    println!(
        "mesh  worker CPU: {:>8.1} ms  {:>6} jobs  {:.3} ms/job",
        mesh_ns as f64 / 1e6,
        mesh_jobs,
        per(mesh_ns, mesh_jobs)
    );
    println!(
        "light worker CPU: {:>8.1} ms  {:>6} jobs  {:.3} ms/job",
        light_ns as f64 / 1e6,
        light_jobs,
        per(light_ns, light_jobs)
    );

    // --- Phase 3: update_load crossing costs (the synchronous main-thread call). ---
    let time_call = |world: &mut World, label: &str, x: i32, cy: i32, z: i32| {
        let t = Instant::now();
        world.update_load(x, cy, z);
        println!(
            "update_load {label:<22}: {:>8.3} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
    };
    time_call(&mut world, "(horizontal +1)", 1, cam_cy, 0);
    time_call(&mut world, "(horizontal +2)", 2, cam_cy, 0);
    time_call(&mut world, "(vertical +1)", 2, cam_cy + 1, 0);
    time_call(&mut world, "(vertical -1)", 2, cam_cy, 0);
    time_call(&mut world, "(same target)", 2, cam_cy, 0);
    drop(world);
    run_concurrent(seed, r, cam_cy);
}

/// GAME-LIKE CONCURRENT LOAD — gen + light + mesh pumped together, frame-paced at
/// ~60 fps like the real game loop, on a fresh world. This is the number the player
/// feels on a fresh spawn / long flight; the split phases isolate per-stage costs
/// but hide cross-stage churn and per-frame cap effects.
fn run_concurrent(seed: u32, r: i32, cam_cy: i32) {
    let (mesh_ns0, mesh_jobs0, light_ns0, light_jobs0) = stage_stats();
    let mut world = World::new(seed, r);
    world.update_load(0, cam_cy, 0);
    let t_conc = Instant::now();
    let mut frames = 0u64;
    let mut idle = 0;
    let mut last = (0usize, 0usize);
    let mut settle_ms = 0.0f64;
    loop {
        world.poll();
        world.tick_mesh_budget(64); // the game-loop budget
        frames += 1;
        let now = (world.loaded_section_count(), world.iter_meshes().count());
        if now == last && now.0 > 0 && !world.has_dirty_meshes() {
            idle += 1;
            if idle == 1 {
                settle_ms = t_conc.elapsed().as_secs_f64() * 1e3;
            }
            if idle >= 20 {
                break;
            }
        } else {
            idle = 0;
            last = now;
        }
        std::thread::sleep(Duration::from_micros(16_600));
        if t_conc.elapsed() > Duration::from_secs(180) {
            eprintln!("concurrent phase timed out");
            break;
        }
    }
    let (mesh_ns1, mesh_jobs1, light_ns1, light_jobs1) = stage_stats();
    println!(
        "CONCURRENT @60fps          : {settle_ms:>8.1} ms wall to settled ({} frames, {} meshes)",
        frames,
        world.iter_meshes().count()
    );
    println!(
        "  mesh  jobs {:>6}  {:>8.1} ms CPU   light jobs {:>6}  {:>8.1} ms CPU",
        mesh_jobs1 - mesh_jobs0,
        (mesh_ns1 - mesh_ns0) as f64 / 1e6,
        light_jobs1 - light_jobs0,
        (light_ns1 - light_ns0) as f64 / 1e6,
    );
}
