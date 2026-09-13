//! Particle emitters attached to bodies. Every body — mob or player — presents
//! from ONE list of emitter bundle ids ([`body_emitters`]): its attached bundles
//! plus its active condition stages' emitters. Its particles, body tint and
//! body self-lighting all read that list.

use super::*;
use petramond_world::particle_emitters::{self as emitters, EmitterBundle};

pub(super) struct EmitterBody {
    feet: Vec3,
    size: Vec3,
    yaw: f32,
    seed: u64,
    skylight: u8,
    blocklight: petramond_world::light::BlockLight6,
}

/// The emitter ids a body wears: its `attached` bundles and each active
/// condition stage's emitter, sorted, each id once.
pub(super) fn body_emitters(attached: &[u8], conditions: &[(u8, u8)]) -> Vec<u8> {
    let stages = conditions.iter().filter_map(|&(condition, stage)| {
        let def = petramond_world::condition::defs().get(condition as usize)?;
        Some(emitters::by_key(def.stages.get(stage as usize)?.emitter?)?.id)
    });
    let mut ids: Vec<u8> = attached.iter().copied().chain(stages).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn bundles(ids: impl IntoIterator<Item = u8>) -> impl Iterator<Item = &'static EmitterBundle> {
    ids.into_iter().filter_map(emitters::def)
}

/// The multiply body tint of a body's emitters (white when none declares one).
pub(super) fn emitter_tint(ids: impl IntoIterator<Item = u8>) -> [f32; 3] {
    bundles(ids)
        .filter_map(|bundle| bundle.tint)
        .fold([1.0; 3], |t, b| [t[0] * b[0], t[1] * b[1], t[2] * b[2]])
}

/// How much of its light a body provides itself: the strongest emitter wins.
pub(super) fn emitter_self_lit(ids: impl IntoIterator<Item = u8>) -> f32 {
    bundles(ids)
        .map(|bundle| bundle.body_self_lit)
        .fold(0.0, f32::max)
}

fn append_emitters(
    out: &mut Vec<PlacedEmitter>,
    ids: impl IntoIterator<Item = u8>,
    body: &EmitterBody,
    view: &ViewVolume,
) {
    for bundle in bundles(ids) {
        append(out, bundle, body, view);
    }
}

fn append(
    out: &mut Vec<PlacedEmitter>,
    bundle: &EmitterBundle,
    body: &EmitterBody,
    view: &ViewVolume,
) {
    let scale = bundle
        .body_size
        .map(|size| body.size / Vec3::from_array(size));
    for (row, authored) in bundle.rows.iter().enumerate() {
        let mut emitter = *authored;
        if let Some(scale) = scale {
            emitter.offset = (Vec3::from_array(emitter.offset) * scale).to_array();
            let spread = Vec3::from_array(emitter.spawn_box) * scale;
            let (sin, cos) = body.yaw.sin_cos();
            emitter.spawn_box = [
                cos.abs() * spread.x + sin.abs() * spread.z,
                spread.y,
                sin.abs() * spread.x + cos.abs() * spread.z,
            ];
            emitter.velocity = (Vec3::from_array(emitter.velocity) * scale).to_array();
            emitter.velocity_jitter =
                (Vec3::from_array(emitter.velocity_jitter) * scale).to_array();
            let size_scale = (scale.x * scale.y * scale.z).cbrt();
            emitter.size = emitter.size.map(|v| v * size_scale);
            emitter.spiral[0] *= (scale.x * scale.z).sqrt();
        }
        let origin = body.feet + Vec3::from_array(emitter.offset);
        let envelope = petramond::world::emitter_envelope(&emitter);
        let (lo, hi) = (origin - envelope, origin + envelope);
        if !view.covers_a_pixel(lo, hi, emitters::particle_size(&emitter))
            || !view.aabb_visible(lo, hi)
        {
            continue;
        }
        out.push(PlacedEmitter {
            origin,
            emitter,
            seed: body.seed
                ^ ((bundle.id as u64) << 32)
                ^ (row as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15),
            skylight: body.skylight,
            blocklight: body.blocklight,
            floor_y: f32::NEG_INFINITY,
        });
    }
}

impl GamePresentationScratch {
    pub(super) fn collect_mob_emitters(&mut self, tick_alpha: f32, view: &ViewVolume) {
        for m in &self.mobs {
            if m.emitters.is_empty() {
                continue;
            }
            let size = petramond::mob::def(m.kind).size;
            let body = EmitterBody {
                feet: m.prev_pos.lerp(m.pos, tick_alpha),
                size: Vec3::new(
                    size.half_width * 2.0,
                    size.height,
                    size.half_length.unwrap_or(size.half_width) * 2.0,
                ),
                yaw: m.yaw,
                seed: m.id,
                skylight: m.skylight,
                blocklight: m.blocklight,
            };
            append_emitters(
                &mut self.particle_emitters,
                m.emitters.iter().copied(),
                &body,
                view,
            );
        }
    }

    /// The local body's emitters, from the body as presented. A first-person
    /// frame presents no body, yet a burning player must still see their own
    /// flames, so there they follow the simulated body instead.
    pub(super) fn collect_local_player_emitters(
        &mut self,
        game: &Game,
        body: Option<&PlayerPresentation>,
        view: &ViewVolume,
    ) {
        if game.player.is_spectator() || game.self_view.health <= 0 {
            return;
        }
        let body = match body {
            Some(body) => {
                EmitterBody::player(body.pos, body.body_yaw, body.skylight, body.blocklight, 0)
            }
            None => {
                let (skylight, blocklight) = game.held_item_light();
                let mut feet = game.player.pos;
                feet.y += game.camera_step_y_offset;
                EmitterBody::player(feet, local_body_yaw(game), skylight, blocklight, 0)
            }
        };
        self.append_player_emitters(&game.self_view.conditions, &body, view);
    }

    pub(super) fn append_player_emitters(
        &mut self,
        conditions: &[(u8, u8)],
        body: &EmitterBody,
        view: &ViewVolume,
    ) {
        append_emitters(
            &mut self.particle_emitters,
            body_emitters(&[], conditions),
            body,
            view,
        );
    }
}

impl EmitterBody {
    /// A player body at its presented feet and body yaw; `id` seeds its streams.
    pub(super) fn player(
        feet: Vec3,
        yaw: f32,
        skylight: u8,
        blocklight: petramond_world::light::BlockLight6,
        id: u64,
    ) -> Self {
        Self {
            feet,
            yaw,
            skylight,
            blocklight,
            size: Vec3::new(
                petramond::player::HALF_W * 2.0,
                petramond::player::HEIGHT,
                petramond::player::HALF_W * 2.0,
            ),
            seed: id ^ 0xD1B5_4A32_D192_ED03,
        }
    }
}
