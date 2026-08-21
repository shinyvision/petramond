//! The BLOCK-LIGHT CELL: three colour channels in one `u16`.
//!
//! (The light BAKE — flood, scheduling, persistence — lives in
//! [`crate::world::light`]; this is only the value it stores.)
//!
//! Channels ride the engine's existing ×2 integer light scale, the same one
//! [`crate::chunk::SKY_FULL`] (= 30) and the flood's decay-of-2 per step are
//! written in, so an uncoloured (white) emitter reproduces the pre-colour
//! scalar cell exactly. 0..=30 needs 5 bits, so 5:5:5 fits a `u16` with one
//! bit to spare and DOUBLES rather than triples every light buffer, memset and
//! blob in the pipeline.
//!
//! Composition is per-channel MAXIMA ([`LightRgb::max_with`]) — the only rule
//! under which two differently-coloured emitters overlap into a sensible
//! third colour instead of one drowning the other.

/// Bits per colour channel. 5 covers `0..=31`, which contains the 0..=30 range
/// exactly.
const CH_BITS: u32 = 5;
const CH_MASK: u16 = (1 << CH_BITS) - 1;
/// Every bit a canonical cell may set. Bit 15 is spare and must stay CLEAR:
/// black needs exactly one representation or `cube_region_changes` reports
/// phantom differences and republishes/remeshes on every bake.
const CANONICAL: u16 = (1 << (3 * CH_BITS)) - 1;

/// Light lost per orthogonal step, per channel — one light level on the ×2
/// scale. Uniform across channels, which is what keeps the batch bake's
/// 16-cell halo sufficient for every channel independently.
pub const DECAY: u8 = 2;

/// One cell of block light: red, green and blue on the ×2 scale, packed
/// 5:5:5 (r in bits 0..5, g in 5..10, b in 10..15).
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct LightRgb(u16);

impl LightRgb {
    /// Unlit. The canonical zero — the ONLY bit pattern that reads as dark.
    pub const ZERO: Self = Self(0);

    /// A channel above `CH_MASK` does not clamp, it WRAPS — the brightest
    /// possible lamp would silently go out. The real guard is the block
    /// loader, which rejects an over-bright row before it can reach here; the
    /// assert is a debug-build backstop for any other producer.
    #[inline]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        debug_assert!(
            (r as u16) <= CH_MASK && (g as u16) <= CH_MASK && (b as u16) <= CH_MASK,
            "light channel exceeds the 5-bit cell"
        );
        Self(
            ((r as u16) & CH_MASK)
                | (((g as u16) & CH_MASK) << CH_BITS)
                | (((b as u16) & CH_MASK) << (2 * CH_BITS)),
        )
    }

    /// Colourless light at level `v` — bit-for-bit what an emitter with no
    /// `light_color` row field produces, and what every pre-colour cell meant.
    #[inline]
    pub const fn grey(v: u8) -> Self {
        Self::new(v, v, v)
    }

    #[inline]
    pub const fn r(self) -> u8 {
        (self.0 & CH_MASK) as u8
    }

    #[inline]
    pub const fn g(self) -> u8 {
        ((self.0 >> CH_BITS) & CH_MASK) as u8
    }

    #[inline]
    pub const fn b(self) -> u8 {
        ((self.0 >> (2 * CH_BITS)) & CH_MASK) as u8
    }

    #[inline]
    pub const fn channels(self) -> [u8; 3] {
        [self.r(), self.g(), self.b()]
    }

    /// The scalar level everything that has not learned about colour reads:
    /// the brightest channel. For grey light this is the value itself, so
    /// consumers that keep using it are unchanged by colour.
    #[inline]
    pub const fn luminance(self) -> u8 {
        let (r, g, b) = (self.r(), self.g(), self.b());
        let m = if r > g { r } else { g };
        if m > b {
            m
        } else {
            b
        }
    }

    #[inline]
    pub const fn is_dark(self) -> bool {
        self.0 == 0
    }

    /// Per-channel maximum: how two lights that reach the same cell compose.
    #[inline]
    pub fn max_with(self, o: Self) -> Self {
        Self::new(
            self.r().max(o.r()),
            self.g().max(o.g()),
            self.b().max(o.b()),
        )
    }

    /// One orthogonal step of travel: every channel loses [`DECAY`],
    /// saturating at zero. Applied per channel so a saturated colour keeps its
    /// hue all the way out to its own reach.
    #[inline]
    pub fn decayed(self) -> Self {
        Self::new(
            self.r().saturating_sub(DECAY),
            self.g().saturating_sub(DECAY),
            self.b().saturating_sub(DECAY),
        )
    }

    /// Raw packed bits — canonical by construction (bit 15 clear).
    #[inline]
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Rebuild from packed bits, forcing canonical form. Save and wire decode
    /// go through this so a corrupt or hostile blob cannot introduce a second
    /// spelling of any colour.
    #[inline]
    pub const fn from_bits(v: u16) -> Self {
        Self(v & CANONICAL)
    }
}

impl std::fmt::Debug for LightRgb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LightRgb({},{},{})", self.r(), self.g(), self.b())
    }
}

/// Bits per channel on the RENDER scale.
const CH6_BITS: u32 = 6;
/// Brightest value a render-scale channel holds.
pub const FULL6: u32 = (1 << CH6_BITS) - 1;
const CH6_MASK: u32 = FULL6;

/// Block light on the 6-bit RENDER scale (`0` dark ..= `63` full), three
/// channels — the presentation twin of [`LightRgb`].
///
/// The bake works on the ×2 scale (0..=30); everything that hands light to the
/// GPU — vertices, entity instances, particles — works on this one, because
/// that is the width the vertex format's light lane has always been. Packed
/// `r | g<<6 | b<<12` so it is ONE `u32` in the greedy merge key and one field
/// on an instance.
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
pub struct BlockLight6(u32);

impl BlockLight6 {
    /// Unlit — the canonical zero, the only spelling of "no block light".
    pub const DARK: Self = Self(0);

    /// As [`LightRgb::new`], an over-range channel wraps rather than clamping,
    /// which would read as a DARKER light than the one that produced it.
    #[inline]
    pub const fn new(r: u32, g: u32, b: u32) -> Self {
        debug_assert!(
            r <= CH6_MASK && g <= CH6_MASK && b <= CH6_MASK,
            "render-scale light channel exceeds 6 bits"
        );
        Self((r & CH6_MASK) | ((g & CH6_MASK) << CH6_BITS) | ((b & CH6_MASK) << (2 * CH6_BITS)))
    }

    /// Colourless light at level `v` — what every pre-colour block-light value
    /// meant, and the value a white emitter still produces.
    #[inline]
    pub const fn grey(v: u32) -> Self {
        Self::new(v, v, v)
    }

    /// Convert a baked ×2-scale cell to the render scale, per channel with the
    /// same rounding the scalar conversion has always used.
    #[inline]
    pub fn from_x2(c: LightRgb) -> Self {
        let q = |v: u8| {
            (v as u32 * FULL6 + crate::chunk::SKY_FULL as u32 / 2) / crate::chunk::SKY_FULL as u32
        };
        Self::new(q(c.r()), q(c.g()), q(c.b()))
    }

    /// The three packed channels as one 18-bit word (and back), so a consumer
    /// that must fold this into a wider bit field does not have to know the
    /// layout.
    #[inline]
    pub const fn bits(self) -> u32 {
        self.0
    }

    #[inline]
    pub const fn from_bits(v: u32) -> Self {
        Self(v & ((1 << (3 * CH6_BITS)) - 1))
    }

    #[inline]
    pub const fn r(self) -> u32 {
        self.0 & CH6_MASK
    }
    #[inline]
    pub const fn g(self) -> u32 {
        (self.0 >> CH6_BITS) & CH6_MASK
    }
    #[inline]
    pub const fn b(self) -> u32 {
        (self.0 >> (2 * CH6_BITS)) & CH6_MASK
    }
    #[inline]
    pub const fn channels(self) -> [u32; 3] {
        [self.r(), self.g(), self.b()]
    }

    /// The scalar level a colour-unaware consumer reads: the brightest channel.
    /// Equal to the value itself for colourless light.
    #[inline]
    pub const fn luminance(self) -> u32 {
        let (r, g, b) = (self.r(), self.g(), self.b());
        let m = if r > g { r } else { g };
        if m > b {
            m
        } else {
            b
        }
    }

    #[inline]
    pub const fn is_dark(self) -> bool {
        self.0 == 0
    }

    /// Per-channel fractions in `0..=1` — the form the shaders and the CPU
    /// light mirror apply their curve to.
    #[inline]
    pub fn fractions(self) -> [f32; 3] {
        self.channels().map(|c| c as f32 / FULL6 as f32)
    }
}

impl std::fmt::Debug for BlockLight6 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlockLight6({},{},{})", self.r(), self.g(), self.b())
    }
}

/// The process-wide all-dark 16³ cube. The overwhelming majority of bakes
/// reach no emitter and produce exactly this, and `Section::set_blocklight`
/// then collapses it to `None` — so handing back the shared buffer costs
/// nothing where allocating and clearing one per bake costs 8 KiB of churn.
pub fn dark_cube() -> std::sync::Arc<[LightRgb]> {
    static DARK: std::sync::OnceLock<std::sync::Arc<[LightRgb]>> = std::sync::OnceLock::new();
    DARK.get_or_init(|| vec![LightRgb::ZERO; crate::chunk::SECTION_VOLUME].into())
        .clone()
}

/// Pack a light cube into little-endian bytes (save blob / TCP frame).
pub fn to_le_bytes(cube: &[LightRgb]) -> Vec<u8> {
    let mut out = Vec::with_capacity(cube.len() * 2);
    for c in cube {
        out.extend_from_slice(&c.bits().to_le_bytes());
    }
    out
}

/// Inverse of [`to_le_bytes`]; `None` when `bytes` is not a whole number of
/// cells.
pub fn from_le_bytes(bytes: &[u8]) -> Option<Box<[LightRgb]>> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    Some(
        bytes
            .chunks_exact(2)
            .map(|c| LightRgb::from_bits(u16::from_le_bytes([c[0], c[1]])))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::SKY_FULL;

    #[test]
    fn a_channel_holds_the_whole_light_scale_and_black_has_one_spelling() {
        // 5 bits must cover the ×2 scale exactly, or a full-strength emitter
        // silently wraps.
        assert_eq!(LightRgb::grey(SKY_FULL).channels(), [SKY_FULL; 3]);
        assert_eq!(LightRgb::grey(SKY_FULL).luminance(), SKY_FULL);

        // Canonical zero: no other bit pattern may read as dark, and decoding
        // must not be able to manufacture one.
        assert!(LightRgb::ZERO.is_dark());
        assert_eq!(LightRgb::from_bits(0x8000), LightRgb::ZERO);
        assert_eq!(LightRgb::default(), LightRgb::ZERO);
        for v in 1..=31u8 {
            assert!(!LightRgb::new(v, 0, 0).is_dark());
            assert!(!LightRgb::new(0, v, 0).is_dark());
            assert!(!LightRgb::new(0, 0, v).is_dark());
        }
    }

    #[test]
    fn channels_never_bleed_into_each_other() {
        for r in 0..=31u8 {
            for g in [0u8, 1, 15, 30, 31] {
                for b in [0u8, 1, 15, 30, 31] {
                    let c = LightRgb::new(r, g, b);
                    assert_eq!(c.channels(), [r, g, b]);
                    assert_eq!(LightRgb::from_bits(c.bits()), c);
                }
            }
        }
    }

    #[test]
    fn grey_light_behaves_exactly_like_the_old_scalar_cell() {
        // Every operation the flood performs on a colourless cell must agree
        // with the pre-colour scalar arithmetic, level for level.
        for v in 0..=SKY_FULL {
            let c = LightRgb::grey(v);
            assert_eq!(c.luminance(), v);
            assert_eq!(c.decayed(), LightRgb::grey(v.saturating_sub(2)));
            assert_eq!(c.is_dark(), v == 0);
            for w in 0..=SKY_FULL {
                assert_eq!(c.max_with(LightRgb::grey(w)), LightRgb::grey(v.max(w)));
            }
        }
    }

    #[test]
    fn two_colours_compose_by_channel_rather_than_one_winning() {
        let purple = LightRgb::new(20, 4, 30);
        let blue = LightRgb::new(4, 12, 30);
        let mixed = purple.max_with(blue);
        assert_eq!(mixed.channels(), [20, 12, 30]);
        // Commutative and idempotent, so bake order can never change a result.
        assert_eq!(blue.max_with(purple), mixed);
        assert_eq!(mixed.max_with(purple), mixed);
    }

    #[test]
    fn a_saturated_colour_keeps_its_hue_as_it_decays() {
        // The green channel of a purple light must run out on its own, not
        // drag the others down with it.
        let mut c = LightRgb::new(22, 6, 30);
        c = c.decayed().decayed().decayed();
        assert_eq!(c.channels(), [16, 0, 24]);
        assert!(!c.is_dark());
    }

    #[test]
    fn the_render_scale_conversion_keeps_hue_and_the_dark_cell() {
        // Every channel converts with the SAME rounding the scalar conversion
        // has always used, so a colourless cell stays colourless and the scalar
        // consumers see exactly what they saw before colour.
        for v in 0..=SKY_FULL {
            let g = BlockLight6::from_x2(LightRgb::grey(v));
            assert_eq!(g, BlockLight6::grey(g.r()));
            assert_eq!(g.luminance(), g.r());
        }
        assert_eq!(BlockLight6::from_x2(LightRgb::grey(SKY_FULL)).r(), FULL6);
        // Dark stays canonically dark — nothing else may read as unlit.
        assert!(BlockLight6::from_x2(LightRgb::ZERO).is_dark());
        assert!(!BlockLight6::from_x2(LightRgb::new(0, 0, 1)).is_dark());
        // A saturated cell keeps its ordering and its brightest channel.
        let c = BlockLight6::from_x2(LightRgb::new(12, 8, 30));
        assert!(c.b() > c.r() && c.r() > c.g());
        assert_eq!(c.luminance(), c.b());
    }

    #[test]
    fn le_bytes_round_trip_a_cube() {
        let cube: Vec<LightRgb> = (0..64u8)
            .map(|i| {
                let i = u16::from(i);
                LightRgb::new((i % 31) as u8, (i * 3 % 31) as u8, (i * 7 % 31) as u8)
            })
            .collect();
        let bytes = to_le_bytes(&cube);
        assert_eq!(bytes.len(), cube.len() * 2);
        assert_eq!(&from_le_bytes(&bytes).unwrap()[..], &cube[..]);
        assert!(from_le_bytes(&bytes[..bytes.len() - 1]).is_none());
    }
}
