const CH_BITS: u32 = 5;
const CH_MASK: u16 = (1 << CH_BITS) - 1;
const CANONICAL: u16 = (1 << (3 * CH_BITS)) - 1;

pub const DECAY: u8 = 2;

#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct LightRgb(u16);

impl LightRgb {
    pub const ZERO: Self = Self(0);

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

    #[inline]
    pub fn max_with(self, o: Self) -> Self {
        Self::new(
            self.r().max(o.r()),
            self.g().max(o.g()),
            self.b().max(o.b()),
        )
    }

    #[inline]
    pub fn decayed(self) -> Self {
        Self::new(
            self.r().saturating_sub(DECAY),
            self.g().saturating_sub(DECAY),
            self.b().saturating_sub(DECAY),
        )
    }

    #[inline]
    pub const fn bits(self) -> u16 {
        self.0
    }

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

const CH6_BITS: u32 = 6;
pub const FULL6: u32 = (1 << CH6_BITS) - 1;
const CH6_MASK: u32 = FULL6;

#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
pub struct BlockLight6(u32);

impl BlockLight6 {
    pub const DARK: Self = Self(0);

    #[inline]
    pub const fn new(r: u32, g: u32, b: u32) -> Self {
        debug_assert!(
            r <= CH6_MASK && g <= CH6_MASK && b <= CH6_MASK,
            "render-scale light channel exceeds 6 bits"
        );
        Self((r & CH6_MASK) | ((g & CH6_MASK) << CH6_BITS) | ((b & CH6_MASK) << (2 * CH6_BITS)))
    }

    #[inline]
    pub const fn grey(v: u32) -> Self {
        Self::new(v, v, v)
    }

    #[inline]
    pub fn from_x2(c: LightRgb) -> Self {
        let q = |v: u8| {
            (v as u32 * FULL6 + crate::chunk::SKY_FULL as u32 / 2) / crate::chunk::SKY_FULL as u32
        };
        Self::new(q(c.r()), q(c.g()), q(c.b()))
    }

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

pub fn dark_cube() -> std::sync::Arc<[LightRgb]> {
    static DARK: std::sync::OnceLock<std::sync::Arc<[LightRgb]>> = std::sync::OnceLock::new();
    DARK.get_or_init(|| vec![LightRgb::ZERO; crate::chunk::SECTION_VOLUME].into())
        .clone()
}

pub fn to_le_bytes(cube: &[LightRgb]) -> Vec<u8> {
    let mut out = Vec::with_capacity(cube.len() * 2);
    for c in cube {
        out.extend_from_slice(&c.bits().to_le_bytes());
    }
    out
}

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
        assert_eq!(LightRgb::grey(SKY_FULL).channels(), [SKY_FULL; 3]);
        assert_eq!(LightRgb::grey(SKY_FULL).luminance(), SKY_FULL);

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
        assert_eq!(blue.max_with(purple), mixed);
        assert_eq!(mixed.max_with(purple), mixed);
    }

    #[test]
    fn a_saturated_colour_keeps_its_hue_as_it_decays() {
        let mut c = LightRgb::new(22, 6, 30);
        c = c.decayed().decayed().decayed();
        assert_eq!(c.channels(), [16, 0, 24]);
        assert!(!c.is_dark());
    }

    #[test]
    fn the_render_scale_conversion_keeps_hue_and_the_dark_cell() {
        for v in 0..=SKY_FULL {
            let g = BlockLight6::from_x2(LightRgb::grey(v));
            assert_eq!(g, BlockLight6::grey(g.r()));
            assert_eq!(g.luminance(), g.r());
        }
        assert_eq!(BlockLight6::from_x2(LightRgb::grey(SKY_FULL)).r(), FULL6);
        assert!(BlockLight6::from_x2(LightRgb::ZERO).is_dark());
        assert!(!BlockLight6::from_x2(LightRgb::new(0, 0, 1)).is_dark());
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
