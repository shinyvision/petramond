//! The tiny 3×5 numeric font used for compact item-stack counts.
//!
//! Each glyph is 3 px wide × 5 px tall, encoded as 5 rows of 3 bits (MSB = left
//! column). The UI renderer scales one "pixel" up to the chosen GUI scale and
//! emits a small solid quad per lit cell, so counts read crisp at any GUI scale.

/// Glyph width in font-pixels.
pub const GLYPH_W: u32 = 3;
/// Glyph height in font-pixels.
pub const GLYPH_H: u32 = 5;
/// Horizontal advance per digit in font-pixels (glyph + 1px gap).
pub const GLYPH_ADVANCE: u32 = GLYPH_W + 1;

/// 3×5 bitmaps for `0..=9`. Row 0 is the top; within a row bit 2 (value 4) is the
/// left column, bit 0 (value 1) the right.
const DIGITS: [[u8; GLYPH_H as usize]; 10] = [
    // 0
    [0b111, 0b101, 0b101, 0b101, 0b111],
    // 1
    [0b010, 0b110, 0b010, 0b010, 0b111],
    // 2
    [0b111, 0b001, 0b111, 0b100, 0b111],
    // 3
    [0b111, 0b001, 0b111, 0b001, 0b111],
    // 4
    [0b101, 0b101, 0b111, 0b001, 0b001],
    // 5
    [0b111, 0b100, 0b111, 0b001, 0b111],
    // 6
    [0b111, 0b100, 0b111, 0b101, 0b111],
    // 7
    [0b111, 0b001, 0b010, 0b010, 0b010],
    // 8
    [0b111, 0b101, 0b111, 0b101, 0b111],
    // 9
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// Pixel width (in font-pixels) of the decimal rendering of `n` (no leading gap).
#[inline]
pub fn number_width(n: u32) -> u32 {
    let digits = digit_count(n);
    digits * GLYPH_ADVANCE - 1 // drop the trailing inter-glyph gap
}

/// Number of decimal digits in `n` (at least 1, for `0`).
#[inline]
pub fn digit_count(mut n: u32) -> u32 {
    let mut count = 1;
    while n >= 10 {
        n /= 10;
        count += 1;
    }
    count
}

/// `true` if cell `(col, row)` of `digit`'s 3×5 glyph is lit. `digit` is clamped
/// to `0..=9`; out-of-range cells are treated as unlit.
#[inline]
pub fn digit_cell(digit: u8, col: u32, row: u32) -> bool {
    if col >= GLYPH_W || row >= GLYPH_H {
        return false;
    }
    let bits = DIGITS[(digit.min(9)) as usize][row as usize];
    // Bit (GLYPH_W - 1 - col): leftmost column is the high bit.
    (bits >> (GLYPH_W - 1 - col)) & 1 == 1
}

/// Visit every lit cell of the decimal rendering of `n`, calling `f(px, py)` with
/// the cell's offset in font-pixels from the top-left of the number (x grows
/// right, y grows down). Used by the UI renderer to emit one solid quad per cell.
pub fn for_each_lit_cell(n: u32, mut f: impl FnMut(u32, u32)) {
    let digits = digit_count(n);
    // Extract digits most-significant first.
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
        // 2 digits: 2*4 - 1 = 7 font-pixels wide.
        assert_eq!(number_width(64), 7);
        // 1 digit: 3 wide.
        assert_eq!(number_width(7), 3);
    }

    #[test]
    fn one_glyph_has_expected_lit_cells() {
        // '1' top row is the single middle column.
        assert!(digit_cell(1, 1, 0));
        assert!(!digit_cell(1, 0, 0));
        assert!(!digit_cell(1, 2, 0));
        // Bottom row of '1' is all three columns.
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
        // '64' -> some cells in [0,3) for '6' and [4,7) for '4'.
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
