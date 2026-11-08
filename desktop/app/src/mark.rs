// SPDX-License-Identifier: GPL-3.0-or-later
//! The G7 Link icon, for the tray, window and toast icons at runtime and the executable's icon at build time
//! (`build.rs` includes this file). Every size is averaged down from one 256 x 256 picture.

const SIDE: usize = 256;
const ICON: &[u8; SIDE * SIDE * 4] = include_bytes!("../../../assets/brand/icon-256.rgba");

/// The icon at `size` pixels, as straight-alpha RGBA, row by row.
pub fn render(size: usize) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        let (top, bottom) = span(y, size);
        for x in 0..size {
            let (left, right) = span(x, size);
            let mut color = [0u32; 3];
            let mut alpha = 0u32;
            let mut samples = 0u32;
            for sy in top..bottom {
                for sx in left..right {
                    let i = (sy * SIDE + sx) * 4;
                    let a = u32::from(ICON[i + 3]);
                    for (c, total) in color.iter_mut().enumerate() {
                        *total += u32::from(ICON[i + c]) * a;
                    }
                    alpha += a;
                    samples += 1;
                }
            }
            if alpha == 0 {
                continue;
            }
            let i = (y * size + x) * 4;
            for (c, total) in color.iter().enumerate() {
                out[i + c] = ((total + alpha / 2) / alpha) as u8;
            }
            out[i + 3] = ((alpha + samples / 2) / samples) as u8;
        }
    }
    out
}

/// The source rows or columns that output index `i` of `size` covers; at least one.
fn span(i: usize, size: usize) -> (usize, usize) {
    let start = i * SIDE / size;
    let end = ((i + 1) * SIDE).div_ceil(size).min(SIDE);
    (start, end.max(start + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_transparent_and_the_middle_is_not() {
        let size = 64;
        let image = render(size);
        assert_eq!(image.len(), size * size * 4);
        assert_eq!(image[3], 0, "the top left corner");
        assert_eq!(image[(size / 2 * size + size / 2) * 4 + 3], 255, "the middle");
    }

    #[test]
    fn renders_every_tray_size() {
        for size in [16, 20, 24, 32, 40, 48, 64, 256, 512] {
            let image = render(size);
            assert_eq!(image.len(), size * size * 4);
            assert!(image.as_chunks::<4>().0.iter().any(|px| px[3] == 255), "{size} has solid pixels");
        }
    }
}
