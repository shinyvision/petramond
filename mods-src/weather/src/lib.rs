//! weather — localized clouds, wind, rain, and snow as pure mod policy.
//!
//! One wasm serves both sides (`pack.json` points `wasm` and `client_wasm`
//! at it); `mod_init` branches on the runtime side.
//!
//! - **Server** (deterministic tick): integrates the global wind into the
//!   field's advection offset, publishes the replicated `weather:*` shader
//!   params (the WHOLE weather state is those few vec4s — see
//!   `weather-core`), publishes the same field to other server mods on the
//!   session-scoped `weather:field` event (`weather_core::feed`), and
//!   accumulates snow layers on cold-biome surfaces while it snows there.
//! - **Client** (presentation): reads the same params back
//!   (`client_env_params`), evaluates the same field at the camera, and
//!   drives the rain/snow ambient particle volumes, the rain sound bed, and
//!   the sky-gated rainy mood grade. The pack's `clouds.wgsl` evaluates the
//!   field per pixel from the identical params — sim, presentation, and sky
//!   always agree.
//!
//! Weather TIME is the persisted `petramond:clock` (frozen time = frozen
//! weather); the advection offset persists in world KV, so a storm front
//! survives a reload mid-crossing.

mod keys;
mod offset;

use mod_sdk::*;
use weather_core::{coverage, field_params, rain_from_coverage, wind, FieldParams};

use offset::Advection;

/// The one tick system: advance + publish + accumulate.
const TICK_WEATHER: u32 = 1;

/// Where earlier versions mirrored the field into the PERSISTENT world KV
/// every tick; the row now travels on the `weather:field` session event, and
/// init clears the leftover so saves stop carrying a frozen sky.
const LEGACY_KV_FIELD: &str = "weather:field";

/// Snow-accumulation probes per tick, round-robin over connected players.
const SNOW_PROBES_PER_TICK: u32 = 8;
/// Probes land within this radius of a player (blocks).
const SNOW_RADIUS: i32 = 48;

/// How often the client re-samples its column biome / roof cover (frames).
const CLIENT_BIOME_INTERVAL: u64 = 30;
const CLIENT_COVER_INTERVAL: u64 = 20;
/// Audio duck factor while the camera is under cover.
const COVER_DUCK: f32 = 0.3;
/// Cells above the head the sky probe scans before calling the overhead mass
/// a roof without reading it. Must comfortably exceed the tallest canopy so a
/// giant redwood still reads "under a tree"; anything deeper overhead is a
/// cave or megastructure.
const SKY_SCAN_MAX: i32 = 96;

/// The biomes where precipitation falls as snow and settles as layers. The
/// pack's `weather:snow` emitter row lists the same biomes (and
/// `weather:rain` excludes them) so particles and accumulation agree; every
/// one of them is an engine biome whose worldgen lays snow cover. Both facts
/// are pinned by this crate's tests against the shipped JSON.
const SNOWY_BIOMES: [u8; 5] = [
    biome::SNOWY_PLAINS,
    biome::SNOWY_TUNDRA,
    biome::SNOWY_TAIGA,
    biome::SNOWY_PEAKS,
    biome::SNOWY_SLOPES,
];

fn is_snowy_biome(biome: u8) -> bool {
    SNOWY_BIOMES.contains(&biome)
}

fn snow_onset(intensity: f32, cell_gate: f32, tick_gate: f32) -> bool {
    intensity > 0.0 && cell_gate <= intensity && tick_gate <= 0.35
}

fn snow_support(
    support: BlockId,
    leaves: &[BlockId],
    water: Option<BlockId>,
    ice: Option<BlockId>,
    packed_ice: Option<BlockId>,
    shape: Option<CollisionShape>,
) -> bool {
    Some(support) != water
        && Some(support) != ice
        && Some(support) != packed_ice
        && (leaves.contains(&support) || shape == Some(CollisionShape::Full))
}

#[derive(Default)]
struct Weather {
    side_is_client: bool,
    /// World-seed-derived field seed (24-bit so it rides a shader param
    /// exactly), drawn once from a deterministic RNG stream.
    seed: u32,
    // --- server state ---------------------------------------------------
    /// Advection offset accumulated in f64 (wrapped into [0, WRAP)), and the
    /// clock it last followed; the params carry the offset as f32.
    advection: Advection,
    snow_layer: Option<BlockId>,
    /// Bare-ice invariant: worldgen deliberately keeps sea/pond ice snowless
    /// (`frozen_ponds_carry_bare_sea_ice_without_a_snow_layer`); accumulation
    /// must too.
    ice: Option<BlockId>,
    packed_ice: Option<BlockId>,
    /// Engine water — excluded from snow footing (composed into
    /// [`Weather::full_solid_support`]).
    water: Option<BlockId>,
    /// The [`keys::LEAF_TAG`] member ids, queried at init — both sides use them:
    /// the server's snow accumulation rests layers on canopy tops, the
    /// client's sky probe sees through them.
    leaves: Vec<BlockId>,
    /// Round-robin cursor over players for snow probes.
    probe_cursor: u64,
    // --- client state ---------------------------------------------------
    frame: u64,
    /// Cached column biome under the camera (refreshed on an interval).
    cam_biome: Option<u8>,
    /// Camera is under cover (anything overhead, canopy included) — ducks
    /// the rain bed.
    covered: bool,
    /// Camera has no sky access even with leaves transparent (building,
    /// overhang, cave ceiling) — suppresses the rainy mood grade.
    roofed: bool,
    /// Which chunk the cover cache describes, its raw cells, and the last
    /// FULLY-KNOWN revision (echoed to skip refetching unchanged bytes).
    cover_chunk: Option<[i32; 2]>,
    cover_cells: Option<Vec<u8>>,
    cover_revision: u64,
}

impl Weather {
    /// The weather clock: the persisted absolute day/night clock when core
    /// publishes one (frozen time freezes weather too), else the session
    /// tick counter.
    fn clock(&self) -> u64 {
        world_kv_get(weather_core::CLOCK_KEY)
            .and_then(|b| weather_core::decode_clock(&b))
            .unwrap_or_else(current_tick)
    }

    fn server_tick(&mut self) {
        let clock = self.clock();
        let w = wind(clock, self.seed);
        // Advance only while the clock does ("frozen time = frozen weather"),
        // persisting whenever the deck moved: it moves up to 0.3 blocks a
        // tick, and a reload must not visibly rewind it.
        if self.advection.step(clock, self.seed) {
            self.advection.store();
        }
        let params = field_params(self.advection.off, clock, self.seed);

        // The replicated visual/param state: everything the shader and every
        // client instance needs to evaluate the field locally.
        shader_set_param(keys::WIND_PARAM, [params.off[0], params.off[1], w[0], w[1]]);
        shader_set_param(
            keys::SKY_PARAM,
            [
                params.storm,
                weather_core::RAIN_START,
                weather_core::FEATURE_SIZE,
                self.seed as f32,
            ],
        );
        // The morph lane: which epoch pair the field is blending between.
        shader_set_param(
            keys::FLUX_PARAM,
            [params.epoch as f32, params.epoch_frac, 0.0, 0.0],
        );

        // Cross-mod interop (server mods cannot read shader params): the
        // COMPLETE field in one row on the session-scoped channel, so a
        // foreign mod evaluates weather-core locally from what it heard. A
        // session event, not world KV: nothing persists, and a world without
        // this mod simply hears no weather.
        weather_core::feed::publish(&weather_core::FieldRow {
            params,
            wind: w,
            clock,
        });

        self.accumulate_snow(&params);
    }

    /// Budgeted snow accumulation: a few hash-scattered probes around
    /// connected players; where the field precipitates over a snowy biome,
    /// eligible bare surfaces gain a snow layer. The per-probe positional
    /// threshold makes onset patchy and organic instead of a sweeping fill.
    /// Full-solid snow footing, composed from the generic collision-shape
    /// query: exactly one full unit collision cube, not water, not leaves
    /// (canopy is accepted separately at the call site). An unresolved shape
    /// (`None` — unloaded / not stream-final) is never footing.
    fn accumulate_snow(&mut self, params: &FieldParams) {
        let Some(snow_layer) = self.snow_layer else {
            return;
        };
        let players = players();
        if players.is_empty() {
            return;
        }
        for _ in 0..SNOW_PROBES_PER_TICK {
            self.probe_cursor = self.probe_cursor.wrapping_add(1);
            let p = &players[(self.probe_cursor % players.len() as u64) as usize];
            if p.state.spectator {
                continue;
            }
            let roll = rng_u64("snow_scatter");
            let dx = (roll & 0xFFFF) as i32 % (2 * SNOW_RADIUS + 1) - SNOW_RADIUS;
            let dz = ((roll >> 16) & 0xFFFF) as i32 % (2 * SNOW_RADIUS + 1) - SNOW_RADIUS;
            let x = p.state.pos[0] as i32 + dx;
            let z = p.state.pos[2] as i32 + dz;
            let intensity = rain_from_coverage(coverage(f64::from(x), f64::from(z), params));
            if intensity <= 0.0 {
                continue;
            }
            let Some(biome) = biome_at([x, z]) else {
                continue;
            };
            if !is_snowy_biome(biome) {
                continue;
            }
            // Positional onset threshold: a column joins the cover only once
            // the LOCAL intensity clears its own hash — light snowfall dusts
            // scattered patches, a storm whites everything out. The extra
            // per-tick roll on top keeps even a storm's fill-in gradual.
            let cell_gate = splitmix64_mix(((x as u64) << 32) ^ (z as u64 & 0xFFFF_FFFF)) as f32
                / u64::MAX as f32;
            let tick_gate = (roll >> 32) as f32 / u32::MAX as f32;
            if !snow_onset(intensity, cell_gate, tick_gate) {
                continue;
            }
            let Some(surface_y) = surface_y_at([x, z]) else {
                continue;
            };
            let Some(support) = get_block([x, surface_y, z]) else {
                continue;
            };
            // Snow rests on full solid cubes AND canopy tops (`surface_y_at`
            // already lands on the treetop — leaves block movement), but
            // never on frozen water: worldgen keeps sea/pond ice bare and
            // its parity tests pin it.
            let shape = if self.leaves.contains(&support) {
                None
            } else {
                collision_shape_at([x, surface_y, z])
            };
            if !snow_support(
                support,
                &self.leaves,
                self.water,
                self.ice,
                self.packed_ice,
                shape,
            ) {
                continue;
            }
            let above = [x, surface_y + 1, z];
            if get_block(above) != Some(BlockId::AIR) {
                continue;
            }
            set_block(above, snow_layer);
        }
    }

    fn client_frame_impl(&mut self, frame: &ClientFrameData) {
        self.frame = self.frame.wrapping_add(1);
        let read = client_env_params(&[keys::WIND_PARAM, keys::SKY_PARAM, keys::FLUX_PARAM]);
        let (Some(wind_p), Some(sky_p)) = (read[0], read[1]) else {
            // No weather server mod publishing (or params not landed yet):
            // everything idles at zero and eases out on its own.
            client_ambient_set(keys::RAIN_BUNDLE, 0.0, [0.0, 0.0]);
            client_ambient_set(keys::SNOW_BUNDLE, 0.0, [0.0, 0.0]);
            client_loop_set(keys::RAIN_LOOP, 0.0);
            client_mood_set(0.0, 0.0);
            return;
        };
        let flux = read[2].unwrap_or([0.0, 0.0, 0.0, 0.0]);
        let params = FieldParams {
            off: [wind_p[0], wind_p[1]],
            storm: sky_p[0],
            seed: sky_p[3] as u32,
            epoch: flux[0] as u32,
            epoch_frac: flux[1],
        };
        let wind_v = [wind_p[2], wind_p[3]];
        let (x, z) = (frame.player_pos[0], frame.player_pos[2]);
        let intensity = rain_from_coverage(coverage(x, z, &params));

        // floor(), not truncation: at fractional negative coords `as i32`
        // probes the neighbouring column.
        let cell = [x.floor() as i32, z.floor() as i32];
        if self.cam_biome.is_none() || self.frame.is_multiple_of(CLIENT_BIOME_INTERVAL) {
            self.cam_biome = client_biome_at(cell);
        }
        if self.frame.is_multiple_of(CLIENT_COVER_INTERVAL) {
            self.refresh_cover(frame);
        }
        let snowy = self.cam_biome.is_some_and(is_snowy_biome);
        // Superlinear density curve: drizzle stays sparse, a downpour REALLY
        // pours (the bundle's count budget is sized for the top end).
        let poured = intensity.powf(1.4);
        // BOTH bundles run at the same intensity: each filters itself per
        // column through its `biomes`/`exclude_biomes` row, so a biome
        // border shows rain and snow side by side, column-exact.
        client_ambient_set(keys::RAIN_BUNDLE, poured, wind_v);
        client_ambient_set(keys::SNOW_BUNDLE, poured, wind_v);
        // Audio and mood follow the CAMERA's column: standing in the snowy
        // column, the rain bed hushes.
        let rain_i = if snowy { 0.0 } else { poured };

        // The rainy-mood grade: a touch darker and greyer exactly where it
        // POURS (the camera's column — snowy columns stay bright), fading
        // back to the untouched image under clear sky. SKY-GATED: only with
        // sky access, leaves transparent (`refresh_cover`) — under a tree
        // stays moody, caves and interiors look normal. Pure post-process —
        // gameplay light (mob spawning!) never changes.
        let mood_i = if self.roofed { 0.0 } else { rain_i };
        client_mood_set(0.1 * mood_i, 0.22 * mood_i);

        let duck = if self.covered { COVER_DUCK } else { 1.0 };
        client_loop_set(keys::RAIN_LOOP, rain_i * duck);
    }

    /// Roof probes, two verdicts from one column: `covered` (audio duck) is
    /// the visible surface — ANYTHING overhead, canopy included, hushes the
    /// rain bed. `roofed` (mood gate) re-scans the column with leaves
    /// transparent, so only a real roof — building, overhang, cave ceiling —
    /// counts as "no sky access".
    fn refresh_cover(&mut self, frame: &ClientFrameData) {
        let wx = frame.player_pos[0].floor() as i32;
        let wz = frame.player_pos[2].floor() as i32;
        let (cx, cz) = (wx >> 4, wz >> 4);
        // The revision only skips REFETCHING an unchanged column's bytes;
        // the verdict is re-evaluated from the cached cells every probe, so
        // walking out from under a roof un-ducks even when the terrain never
        // changed. Crossing a chunk drops the cache (its bytes belong to
        // another column).
        if self.cover_chunk != Some([cx, cz]) {
            self.cover_chunk = Some([cx, cz]);
            self.cover_cells = None;
            self.cover_revision = 0;
        }
        let reply = client_surface_columns(vec![ClientSurfaceQuery {
            coord: [cx, cz],
            revision: self.cover_revision,
        }]);
        if let Some(Some(column)) = reply.into_iter().next() {
            if let Some(cells) = column.cells {
                // The surface-columns contract: only echo a revision from a
                // reply whose EVERY cell was known — else keep asking for
                // fresh bytes (an unknown cell may finalize without a
                // revision bump).
                let all_known = cells
                    .chunks_exact(CLIENT_SURFACE_CELL_BYTES)
                    .all(|c| i16::from_le_bytes([c[0], c[1]]) != CLIENT_SURFACE_UNKNOWN_HEIGHT);
                self.cover_revision = if all_known { column.revision } else { 0 };
                self.cover_cells = Some(cells);
            }
        }
        let Some(cells) = &self.cover_cells else {
            return; // no data yet: keep the previous verdict
        };
        let (lx, lz) = ((wx & 15) as usize, (wz & 15) as usize);
        let idx = (lz * 16 + lx) * CLIENT_SURFACE_CELL_BYTES;
        let h = i16::from_le_bytes([cells[idx], cells[idx + 1]]);
        self.covered =
            h != CLIENT_SURFACE_UNKNOWN_HEIGHT && f64::from(h) > frame.player_pos[1] + 2.0;
        if !self.covered {
            // Nothing overhead at all — trivially sky access.
            self.roofed = false;
            return;
        }
        let y0 = frame.player_pos[1].floor() as i32 + 2;
        if h as i32 - y0 >= SKY_SCAN_MAX {
            // Deeper overhead mass than any canopy reaches: a cave or
            // megastructure, roofed without reading the cells.
            self.roofed = true;
            return;
        }
        let blocks = client_blocks_at((y0..=h as i32).map(|y| [wx, y, wz]).collect());
        for block in blocks {
            match block {
                // Unknown cell (unloaded / streamed content not final): keep
                // the previous verdict instead of flickering the grade.
                None => return,
                Some(b) if b == BlockId::AIR || self.leaves.contains(&b) => {}
                Some(_) => {
                    self.roofed = true;
                    return;
                }
            }
        }
        self.roofed = false;
    }
}

impl Mod for Weather {
    fn init(&mut self) {
        self.side_is_client = runtime_side() == RuntimeSide::Client;
        self.leaves = blocks_by_tag(keys::LEAF_TAG);
        if self.side_is_client {
            // The client never rolls its own seed — it reconstructs the
            // field entirely from the replicated params.
            return;
        }
        // Deterministic per-world field seed (24-bit: rides a shader param
        // f32 exactly). The first draw of a named stream is a pure function
        // of (world seed, mod id, stream) — same value every session.
        self.seed = (rng_u64("field_seed") & 0xFF_FFFF) as u32;
        // After core day/night (priority 0 in the same window), so the clock
        // read is this tick's. No consumer depends on running after this:
        // the field channel carries the clock and consumers advance to theirs.
        register_tick_system(Stage::Spawning, AttachSide::After, 10, TICK_WEATHER);
        self.snow_layer = resolve_block_logged(keys::SNOW_LAYER);
        self.ice = resolve_block_logged(keys::ICE);
        self.packed_ice = resolve_block_logged(keys::PACKED_ICE);
        self.water = resolve_block_logged(keys::WATER);
        world_kv_delete(LEGACY_KV_FIELD);
        self.advection = Advection::new(offset::load());
    }

    fn tick_system(&mut self, system_id: u32) {
        if system_id == TICK_WEATHER {
            self.server_tick();
        }
    }

    fn client_frame(&mut self, frame: &ClientFrameData) {
        self.client_frame_impl(frame);
    }
}

register_mod!(Weather);

#[cfg(test)]
mod snow_tests {
    use super::*;

    #[test]
    fn local_snow_onset_is_gradual_and_respects_biome() {
        assert!(is_snowy_biome(biome::SNOWY_PLAINS));
        assert!(!is_snowy_biome(biome::PLAINS));
        assert!(!snow_onset(0.0, 0.0, 0.0));
        assert!(!snow_onset(0.4, 0.5, 0.0));
        assert!(!snow_onset(0.8, 0.5, 0.36));
        assert!(snow_onset(0.8, 0.5, 0.35));
    }

    #[test]
    fn snow_rests_on_solid_ground_and_canopy_but_spares_water_and_ice() {
        let leaves = BlockId(1);
        let rock = BlockId(2);
        let water = BlockId(3);
        let ice = BlockId(4);
        let packed_ice = BlockId(5);
        let allowed = |block, shape| {
            snow_support(
                block,
                &[leaves],
                Some(water),
                Some(ice),
                Some(packed_ice),
                shape,
            )
        };
        assert!(allowed(leaves, None));
        assert!(allowed(rock, Some(CollisionShape::Full)));
        assert!(!allowed(rock, Some(CollisionShape::Empty)));
        assert!(!allowed(water, Some(CollisionShape::Full)));
        assert!(!allowed(ice, Some(CollisionShape::Full)));
        assert!(!allowed(packed_ice, Some(CollisionShape::Full)));
    }
}
