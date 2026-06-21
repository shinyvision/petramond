//! Shared render-side skylight helpers for dynamic geometry.

/// Full skylight on the 6-bit packed scale used by `mesh::Vertex`.
pub(super) const FULL_SKYLIGHT: u8 = 63;

const SKY_MIN: f32 = 0.05;
const FINAL_MIN: f32 = 0.02;

#[inline]
pub(super) fn skylight_bits(skylight: u8) -> u32 {
    (skylight.min(FULL_SKYLIGHT) as u32) << 23
}

#[inline]
pub(super) fn sky_light_factor(skylight: u8) -> f32 {
    let sky = skylight.min(FULL_SKYLIGHT) as f32 / FULL_SKYLIGHT as f32;
    let sky_term = SKY_MIN + (1.0 - SKY_MIN) * (sky * sky * sky);
    sky_term.max(FINAL_MIN)
}
