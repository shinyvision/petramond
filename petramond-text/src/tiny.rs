pub const GLYPH_W: u32 = 3;
pub const GLYPH_H: u32 = 5;
pub const GLYPH_ADVANCE: u32 = GLYPH_W + 1;

const DIGITS: [[u8; GLYPH_H as usize]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

#[inline]
pub fn number_width(n: u32) -> u32 {
    let digits = digit_count(n);
    digits * GLYPH_ADVANCE - 1
}

#[inline]
pub fn digit_count(mut n: u32) -> u32 {
    let mut count = 1;
    while n >= 10 {
        n /= 10;
        count += 1;
    }
    count
}

#[inline]
pub fn digit_cell(digit: u8, col: u32, row: u32) -> bool {
    if col >= GLYPH_W || row >= GLYPH_H {
        return false;
    }
    let bits = DIGITS[(digit.min(9)) as usize][row as usize];
    (bits >> (GLYPH_W - 1 - col)) & 1 == 1
}

pub fn for_each_lit_cell(n: u32, mut f: impl FnMut(u32, u32)) {
    let digits = digit_count(n);
    let mut place = 10u32.pow(digits - 1);
    let mut x_off = 0u32;
    loop {
        let digit = ((n / place) % 10) as u8;
        for row in 0..GLYPH_H {
            for col in 0..GLYPH_W {
                if digit_cell(digit, col, row) {
                    f(x_off + col, row);
                }
            }
        }
        x_off += GLYPH_ADVANCE;
        if place == 1 {
            break;
        }
        place /= 10;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_count_and_width() {
        assert_eq!(digit_count(0), 1);
        assert_eq!(digit_count(9), 1);
        assert_eq!(digit_count(10), 2);
        assert_eq!(digit_count(64), 2);
        assert_eq!(digit_count(100), 3);
        assert_eq!(number_width(64), 7);
        assert_eq!(number_width(7), 3);
    }

    #[test]
    fn one_glyph_has_expected_lit_cells() {
        assert!(digit_cell(1, 1, 0));
        assert!(!digit_cell(1, 0, 0));
        assert!(!digit_cell(1, 2, 0));
        assert!(digit_cell(1, 0, 4));
        assert!(digit_cell(1, 1, 4));
        assert!(digit_cell(1, 2, 4));
    }

    #[test]
    fn out_of_range_cells_are_unlit() {
        assert!(!digit_cell(0, GLYPH_W, 0));
        assert!(!digit_cell(0, 0, GLYPH_H));
    }

    #[test]
    fn for_each_lit_cell_covers_two_digits() {
        let mut min_x = u32::MAX;
        let mut max_x = 0;
        for_each_lit_cell(64, |px, _py| {
            min_x = min_x.min(px);
            max_x = max_x.max(px);
        });
        assert!(min_x < GLYPH_W, "first glyph cells near x=0");
        assert!(
            max_x >= GLYPH_ADVANCE,
            "second glyph cells past the advance"
        );
        assert!(max_x < number_width(64) + 1);
    }

    #[test]
    fn every_digit_has_at_least_one_lit_cell() {
        for d in 0u8..=9 {
            let mut lit = 0;
            for row in 0..GLYPH_H {
                for col in 0..GLYPH_W {
                    if digit_cell(d, col, row) {
                        lit += 1;
                    }
                }
            }
            assert!(lit > 0, "digit {d} should be visible");
        }
    }
}
