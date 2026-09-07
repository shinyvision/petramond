use super::*;

#[test]
fn readback_strips_row_padding_and_orders_channels() {
    // 2×2 pixels: 8 real bytes per row, padded out to 256.
    let (width, height) = (2u32, 2u32);
    let padded_row = (width * TEXEL_BYTES).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    assert_eq!(padded_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);

    let rows = [
        [1u8, 2, 3, 4, 5, 6, 7, 8],
        [9u8, 10, 11, 12, 13, 14, 15, 16],
    ];
    let mut mapped = vec![0xEEu8; (padded_row * height) as usize];
    for (y, row) in rows.iter().enumerate() {
        let start = y * padded_row as usize;
        mapped[start..start + row.len()].copy_from_slice(row);
    }

    let packed = pack_rows(&mapped, width, height, padded_row, false);
    assert_eq!(packed, [rows[0].as_slice(), rows[1].as_slice()].concat());

    let swizzled = pack_rows(&mapped, width, height, padded_row, true);
    assert_eq!(
        swizzled,
        vec![3, 2, 1, 4, 7, 6, 5, 8, 11, 10, 9, 12, 15, 14, 13, 16]
    );
}
