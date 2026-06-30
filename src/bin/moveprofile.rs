//! Throwaway "flying" profiler. The throughput bench (`streamprofile`) loads a static
//! region; this one simulates the player MOVING — which is the reported symptom (chunks
//! appear slowly + stutter while moving). It measures the MAIN-THREAD per-frame cost of
//! the streaming work (`update_load`, `poll`, `tick_mesh_budget`) as the load window
//! shifts, plus the dirty-mesh backlog. (Render cost is separate — no GPU here.)
//!
//! Run: `cargo run --release --bin moveprofile [render_dist] [mesh_budget]`

use std::time::{Duration, Instant};

use llamacraft::tooling::stream::World;

fn main() {
    let r: i32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(12);
    let seed = 0x1234_5678u32;
    let cy = 5; // realistic surface section (~y80)
    let mesh_budget: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(128);

    let mut world = World::new(seed, r);

    // Warm up: load + mesh the starting region so we measure steady flight, not cold load.
    world.update_load_facing(0, cy, 0, 1.0, 0.0);
    let warm = Instant::now();
    let mut stable = 0;
    let (mut last_sec, mut last_mesh) = (0usize, 0usize);
    while warm.elapsed() < Duration::from_secs(60) {
        world.poll();
        world.tick_mesh_budget(mesh_budget);
        let sec = world.loaded_section_count();
        let mesh = world.mesh_count();
        // Settled only once sections AND meshes have actually arrived and stopped growing.
        if sec > 0 && sec == last_sec && mesh == last_mesh && !world.has_dirty_meshes() {
            stable += 1;
            if stable >= 150 {
                break;
            }
        } else {
            stable = 0;
        }
        last_sec = sec;
        last_mesh = mesh;
        std::thread::sleep(Duration::from_micros(300));
    }
    eprintln!(
        "warmed: {} sections, {} meshes",
        world.loaded_section_count(),
        world.mesh_count()
    );
    let sections_before = world.loaded_section_count();
    // Reset the off-thread stage counters so we measure only the FLYING phase: how many
    // section gens, light bakes, and mesh builds happen while moving. The ideal (mesh-once,
    // like Minecraft) is ~1 mesh build + 1 light bake per newly-streamed section; a re-bake/
    // re-mesh cascade shows up as builds/bakes far exceeding the new-section count.
    llamacraft::perf::reset_all();

    // Fly +x: one section-boundary crossing per step, a handful of frames between crossings
    // (fast flight). Time the three main-thread streaming calls separately.
    let steps = 40i32;
    let frames_per_step = 8;
    let (mut sum_u, mut max_u) = (0.0f64, 0.0f64);
    let (mut sum_p, mut max_p) = (0.0f64, 0.0f64);
    let (mut sum_m, mut max_m) = (0.0f64, 0.0f64);
    let mut max_dirty = 0usize;
    let mut frames = 0u64;

    for step in 1..=steps {
        let t = Instant::now();
        world.update_load_facing(step, cy, 0, 1.0, 0.0); // player crossed into a new section column
        let u = t.elapsed().as_secs_f64() * 1e3;
        sum_u += u;
        max_u = max_u.max(u);

        for _ in 0..frames_per_step {
            let tp = Instant::now();
            world.poll();
            let p = tp.elapsed().as_secs_f64() * 1e3;

            let tm = Instant::now();
            world.tick_mesh_budget(mesh_budget);
            let m = tm.elapsed().as_secs_f64() * 1e3;

            sum_p += p;
            max_p = max_p.max(p);
            sum_m += m;
            max_m = max_m.max(m);
            max_dirty = max_dirty.max(world.dirty_mesh_count());
            frames += 1;
            std::thread::sleep(Duration::from_micros(500)); // let the off-thread pools work
        }
    }

    let f = frames as f64;
    let s = steps as f64;
    println!(
        "=== FLYING +x, RD {r}, mesh budget {mesh_budget}: {steps} section crossings, {frames_per_step} frames each ===",
    );
    println!(
        "update_load  : avg {:>7.3} ms   MAX {:>7.3} ms   (cost of ONE section-boundary crossing)",
        sum_u / s,
        max_u
    );
    println!(
        "poll         : avg {:>7.3} ms   MAX {:>7.3} ms   (per frame)",
        sum_p / f,
        max_p
    );
    println!("tick_mesh    : avg {:>7.3} ms   MAX {:>7.3} ms   (per frame; includes the dirty-queue sort)", sum_m / f, max_m);
    println!("dirty backlog: MAX {max_dirty} sections queued");
    println!(
        "per-frame main-thread total: avg {:>7.3} ms (poll+mesh) + update on crossings",
        (sum_p + sum_m) / f
    );

    // Work done while flying vs. new sections streamed in. mesh/section ≈ 1.0 is "mesh once"
    // (Minecraft); much higher means a re-bake/re-mesh cascade.
    let new_sections = world.loaded_section_count() as i64 - sections_before as i64;
    let (_, gen_n) = llamacraft::perf::GEN_SECTION.snapshot();
    let (_, light_n) = llamacraft::perf::LIGHT.snapshot();
    let (_, mesh_n) = llamacraft::perf::MESH.snapshot();
    println!(
        "FLYING work: {gen_n} section gens, {light_n} light bakes, {mesh_n} mesh builds  (net new loaded sections: {new_sections})"
    );
    if gen_n > 0 {
        println!(
            "  ratios per gen'd section: {:.2} light bakes, {:.2} mesh builds",
            light_n as f64 / gen_n as f64,
            mesh_n as f64 / gen_n as f64
        );
    }
}
