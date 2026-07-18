//! weather — localized clouds, wind, rain, and snow as pure mod policy.
//!
//! One wasm serves both sides (`pack.json` points `wasm` and `client_wasm`
//! at it); `mod_init` branches on the runtime side.
//!
//! - **Server** (deterministic tick): integrates the global wind into the
//!   field's advection offset, publishes the replicated `weather:*` shader
//!   params (the WHOLE weather state is those few vec4s — see
//!   `weather-core`), mirrors them into world KV for other server mods, and
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

use mod_sdk::*;
use weather_core::{coverage, rain_from_coverage, storm, wind, FieldParams, WRAP};

/// The one tick system: advance + publish + accumulate.
const TICK_WEATHER: u32 = 1;

/// World-KV key persisting the advection offset (two LE f64).
const KV_OFF: &str = "weather:off";

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

/// Leaf-block policy shared by both sides, keyed off the `leaves` block tag:
/// the server lets snow rest on canopy tops; the client's sky probe treats
/// these as TRANSPARENT so a canopy never suppresses the rainy mood. A pack
/// leaf block joins by tagging its row `leaves` — no list to maintain here.
const LEAF_TAG: &str = "petramond:leaves";

/// The precipitation visuals this pack ships.
const RAIN_BUNDLE: &str = "weather:rain";
const SNOW_BUNDLE: &str = "weather:snow";
const RAIN_LOOP: &str = "weather:rain_loop";

fn is_snowy_biome(biome: u8) -> bool {
    matches!(
        biome,
        biome::SNOWY_PLAINS
            | biome::SNOWY_TUNDRA
            | biome::SNOWY_TAIGA
            | biome::SNOWY_PEAKS
            | biome::SNOWY_SLOPES
    )
}

#[derive(Default)]
struct Weather {
    side_is_client: bool,
    /// World-seed-derived field seed (24-bit so it rides a shader param
    /// exactly), drawn once from a deterministic RNG stream.
    seed: u32,
    // --- server state ---------------------------------------------------
    /// Advection offset accumulated in f64 (wrapped into [0, WRAP)); the
    /// params carry it as f32.
    off: [f64; 2],
    /// Clock value at the last tick — a frozen `petramond:clock` freezes the
    /// advection too ("frozen time = frozen weather").
    last_clock: Option<u64>,
    snow_layer: Option<BlockId>,
    /// Bare-ice invariant: worldgen deliberately keeps sea/pond ice snowless
    /// (`frozen_ponds_carry_bare_sea_ice_without_a_snow_layer`); accumulation
    /// must too.
    ice: Option<BlockId>,
    packed_ice: Option<BlockId>,
    /// Engine water — excluded from snow footing (composed into
    /// [`Weather::full_solid_support`]).
    water: Option<BlockId>,
    /// The [`LEAF_TAG`] member ids, queried at init — both sides use them:
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
    fn field_params(&self, clock: u64) -> FieldParams {
        let (epoch, epoch_frac) = weather_core::epoch_at(clock);
        FieldParams {
            off: [
                weather_core::wrap_coord(self.off[0]),
                weather_core::wrap_coord(self.off[1]),
            ],
            storm: storm(clock, self.seed),
            seed: self.seed,
            epoch,
            epoch_frac,
        }
    }

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
        // Advance only while the clock does: `time freeze` freezes the whole
        // sky, not just the storm phase ("frozen time = frozen weather").
        // The FIRST tick after load only latches the clock — advancing on it
        // would leak one step of drift into a frozen world.
        if self.last_clock.is_some() && self.last_clock != Some(clock) {
            self.off[0] = (self.off[0] + w[0] as f64 / 20.0).rem_euclid(WRAP as f64);
            self.off[1] = (self.off[1] + w[1] as f64 / 20.0).rem_euclid(WRAP as f64);
        }
        self.last_clock = Some(clock);
        let params = self.field_params(clock);

        // The replicated visual/param state: everything the shader and every
        // client instance needs to evaluate the field locally.
        shader_set_param("weather:wind", [params.off[0], params.off[1], w[0], w[1]]);
        shader_set_param(
            "weather:sky",
            [
                params.storm,
                weather_core::RAIN_START,
                weather_core::FEATURE_SIZE,
                self.seed as f32,
            ],
        );
        // The morph lane: which epoch pair the field is blending between.
        shader_set_param(
            "weather:flux",
            [params.epoch as f32, params.epoch_frac, 0.0, 0.0],
        );

        // Cross-mod interop mirror (server mods read KV, not shader params):
        // the COMPLETE field in one row, so a foreign mod evaluates
        // weather-core locally from one read. The clock stamp is the row's
        // freshness lane — world KV persists, and a reader must be able to
        // tell a live sky from the frozen row an uninstalled weather mod
        // leaves behind.
        let row = weather_core::FieldRow {
            params,
            wind: w,
            clock,
        };
        world_kv_set(weather_core::KV_FIELD, row.encode().to_vec());

        // Persist every tick: the offset moves up to 0.3 blocks/tick, and a
        // reload must not visibly rewind the deck (world KV rides the normal
        // save path; this is one small buffered write).
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(&self.off[0].to_le_bytes());
        bytes.extend_from_slice(&self.off[1].to_le_bytes());
        world_kv_set(KV_OFF, bytes);

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
    fn full_solid_support(&self, block: BlockId, pos: [i32; 3]) -> bool {
        collision_shape_at(pos) == Some(CollisionShape::Full)
            && Some(block) != self.water
            && !self.leaves.contains(&block)
    }

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
            let intensity = rain_from_coverage(coverage(x as f32, z as f32, params));
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
            let cell_gate = splitmix64_mix(((x as u64) << 32) ^ (z as u64 & 0xFFFF_FFFF))
                as f32
                / u64::MAX as f32;
            let tick_gate = (roll >> 32) as f32 / u32::MAX as f32;
            if cell_gate > intensity || tick_gate > 0.35 {
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
            if !self.leaves.contains(&support) && !self.full_solid_support(support, [x, surface_y, z])
            {
                continue;
            }
            if Some(support) == self.ice || Some(support) == self.packed_ice {
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
        let read = client_env_params(&["weather:wind", "weather:sky", "weather:flux"]);
        let (Some(wind_p), Some(sky_p)) = (read[0], read[1]) else {
            // No weather server mod publishing (or params not landed yet):
            // everything idles at zero and eases out on its own.
            client_ambient_set(RAIN_BUNDLE, 0.0, [0.0, 0.0]);
            client_ambient_set(SNOW_BUNDLE, 0.0, [0.0, 0.0]);
            client_loop_set(RAIN_LOOP, 0.0);
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
        if self.cam_biome.is_none() || self.frame % CLIENT_BIOME_INTERVAL == 0 {
            self.cam_biome = client_biome_at(cell);
        }
        if self.frame % CLIENT_COVER_INTERVAL == 0 {
            self.refresh_cover(frame);
        }
        let snowy = self.cam_biome.is_some_and(is_snowy_biome);
        // Superlinear density curve: drizzle stays sparse, a downpour REALLY
        // pours (the bundle's count budget is sized for the top end).
        let poured = intensity.powf(1.4);
        // BOTH bundles run at the same intensity: each filters itself per
        // column through its `biomes`/`exclude_biomes` row, so a biome
        // border shows rain and snow side by side, column-exact.
        client_ambient_set(RAIN_BUNDLE, poured, wind_v);
        client_ambient_set(SNOW_BUNDLE, poured, wind_v);
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
        client_loop_set(RAIN_LOOP, rain_i * duck);
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
            h != CLIENT_SURFACE_UNKNOWN_HEIGHT && (h as f32) > frame.player_pos[1] + 2.0;
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
        self.leaves = blocks_by_tag(LEAF_TAG);
        if self.side_is_client {
            // The client never rolls its own seed — it reconstructs the
            // field entirely from the replicated params.
            return;
        }
        // Deterministic per-world field seed (24-bit: rides a shader param
        // f32 exactly). The first draw of a named stream is a pure function
        // of (world seed, mod id, stream) — same value every session.
        self.seed = (rng_u64("field_seed") & 0xFF_FFFF) as u32;
        register_tick_system(Stage::Spawning, AttachSide::After, 10, TICK_WEATHER);
        self.snow_layer = resolve_block_logged("petramond:snow_layer");
        self.ice = resolve_block_logged("petramond:ice");
        self.packed_ice = resolve_block_logged("petramond:packed_ice");
        self.water = resolve_block_logged("petramond:water");
        if let Some(bytes) = world_kv_get(KV_OFF) {
            if bytes.len() == 16 {
                self.off = [
                    f64::from_le_bytes(bytes[0..8].try_into().unwrap()),
                    f64::from_le_bytes(bytes[8..16].try_into().unwrap()),
                ];
            }
        }
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
