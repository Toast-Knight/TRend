//! Shared machinery for encoders that draw a cell's pixels with two colors
//! (`Quadrant`, `Sextant`, `Braille`): reading a block of pixels, the
//! partially transparent case, and choosing the two-color split.
//!
//! A split is a bit mask over the block's pixels in row-major order: set
//! bits take the foreground color, clear bits the background. Colors are the
//! rounded means of their pixels.

use crate::cell::{Cell, CellViewMut, Color};
use crate::framebuffer::{PixelFrameView, decode, with_decoder};

/// A block's pixels, row-major, as RGB.
pub(crate) type Block<const N: usize> = [[u8; 3]; N];

/// A two-color split of a block. `mask == 0` means one uniform color, and
/// then `foreground == background`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Split {
    pub(crate) mask: u32,
    pub(crate) foreground: Color,
    pub(crate) background: Color,
}

/// Encodes every cell from `COLUMNS`-wide blocks of `N` pixels.
///
/// - No visible pixel: the default cell.
/// - Some visible: the visible pixels' glyph in their mean color, over the
///   terminal default.
/// - All visible: `fit` chooses the split, drawn with its glyph.
///
/// `glyph` maps a foreground mask to its character; mask 0 must be ' '.
#[inline]
pub(crate) fn encode_blocks<const N: usize, const COLUMNS: usize>(
    pixels: &PixelFrameView<'_>,
    cells: &mut CellViewMut<'_>,
    glyph: impl Fn(u32) -> char,
    fit: impl Fn(&Block<N>) -> Split,
) {
    let rows = N / COLUMNS;
    with_decoder!(pixels, D => {
        for cell_y in 0..cells.height() {
            let top = u32::from(cell_y) * rows as u32;
            // Borrow the block's pixel rows once per row of cells; `row`
            // handles bottom-up order and `decode` the format and alpha.
            let mut lines: [&[u8]; 4] = [&[]; 4];
            for (offset, line) in lines.iter_mut().enumerate().take(rows) {
                *line = pixels.row(top + offset as u32).unwrap_or(&[]);
            }
            encode_row::<N, COLUMNS>(cells.row_mut(cell_y), &glyph, &fit, |cell_x, index| {
                let start = (cell_x * COLUMNS + index % COLUMNS) * 4;
                decode::<D>(&lines[index / COLUMNS][start..start + 4])
            });
        }
    });
}

/// Encodes one row of cells; `read(cell_x, index)` returns straight RGBA for
/// pixel `index` (row-major) of the block under cell `cell_x`.
#[inline(always)]
fn encode_row<const N: usize, const COLUMNS: usize>(
    row: &mut [Cell],
    glyph: &impl Fn(u32) -> char,
    fit: &impl Fn(&Block<N>) -> Split,
    read: impl Fn(usize, usize) -> [u8; 4],
) {
    let all = (1_u32 << N) - 1;
    for (cell_x, cell) in row.iter_mut().enumerate() {
        let mut block = [[0_u8; 3]; N];
        let mut visible = 0_u32;
        for (index, color) in block.iter_mut().enumerate() {
            let [red, green, blue, alpha] = read(cell_x, index);
            *color = [red, green, blue];
            visible |= u32::from(alpha != 0) << index;
        }
        *cell = if visible == 0 {
            Cell::default()
        } else if visible != all {
            Cell::with_glyph(glyph(visible), mean(&block, visible), Color::DEFAULT)
        } else {
            let split = fit(&block);
            Cell::with_glyph(glyph(split.mask), split.foreground, split.background)
        };
    }
}

/// Rounded mean color of the pixels selected by `mask` (at least one).
#[inline]
pub(crate) fn mean<const N: usize>(block: &Block<N>, mask: u32) -> Color {
    let mut sums = [0_u32; 3];
    let mut count = 0;
    for (index, color) in block.iter().enumerate() {
        if mask >> index & 1 == 1 {
            for (sum, &channel) in sums.iter_mut().zip(color) {
                *sum += u32::from(channel);
            }
            count += 1;
        }
    }
    let channel = |sum: u32| divide_rounded(sum as i32, count) as u8;
    Color::rgb(channel(sums[0]), channel(sums[1]), channel(sums[2]))
}

/// The exact best split of a 2×2 block under Quadrant's original
/// criterion: the summed squared RGB error of each part against its
/// *rounded* mean color. Every split is tried, uniform first and then masks
/// `1..8` (the last pixel always in the background) in ascending order; the
/// first minimum wins.
///
/// Errors come from sums instead of revisiting pixels: a single pixel has
/// none; a pair differing by `d` in a channel has `(d² + (d & 1)) / 2`
/// there; larger parts use `Σ(c - m)² = Σc² - m(2Σc - n·m)` for the rounded
/// mean `m`. Colors are computed once, for the winning split.
pub(crate) fn fit_exact(block: &Block<4>) -> Split {
    if let Some(split) = uniform(block) {
        return split;
    }
    let colors =
        block.map(|[red, green, blue]| [i32::from(red), i32::from(green), i32::from(blue)]);
    let pair = |first: usize, second: usize| {
        let mut error = 0;
        for (left, right) in colors[first].iter().zip(&colors[second]) {
            let delta = left - right;
            error += (delta * delta + (delta & 1)) >> 1;
        }
        error
    };
    let larger = |mask: usize| {
        let count = i32::from(POPCOUNT[mask]);
        let mut error = 0;
        for channel in 0..3 {
            let (mut sum, mut squares) = (0, 0);
            for (index, color) in colors.iter().enumerate() {
                if mask >> index & 1 == 1 {
                    sum += color[channel];
                    squares += color[channel] * color[channel];
                }
            }
            let mean = divide_rounded(sum, count);
            error += squares - mean * (2 * sum - count * mean);
        }
        error
    };

    // Splits 1..8 in ascending order: each is (part with pixel 3 clear,
    // its complement). Masks 1, 2, 4 are single pixels against a triple;
    // 3, 5, 6 are two pairs; 7 is a triple against pixel 3.
    let splits = [
        larger(0b1110),
        larger(0b1101),
        pair(0, 1) + pair(2, 3),
        larger(0b1011),
        pair(0, 2) + pair(1, 3),
        pair(1, 2) + pair(0, 3),
        larger(0b0111),
    ];
    let mut best_mask = 0;
    let mut best_error = larger(0b1111);
    for (index, &error) in splits.iter().enumerate() {
        if error < best_error {
            best_error = error;
            best_mask = index as u32 + 1;
        }
    }
    if best_mask == 0 {
        let uniform = mean(block, 0b1111);
        Split {
            mask: 0,
            foreground: uniform,
            background: uniform,
        }
    } else {
        Split {
            mask: best_mask,
            foreground: mean(block, best_mask),
            background: mean(block, best_mask ^ 0b1111),
        }
    }
}

/// Bits set in each 8-bit mask, computed at compile time: without a
/// popcount instruction in the baseline target, `count_ones` is a loop.
pub(crate) const POPCOUNT: [u8; 256] = {
    let mut table = [0; 256];
    let mut mask = 0;
    while mask < 256 {
        table[mask] = (mask as u32).count_ones() as u8;
        mask += 1;
    }
    table
};

/// `ceil(2^16 / n)`: `x * RECIPROCAL[n] >> 16 == x / n` for every sum a
/// block of up to 8 pixels can produce (checked by a test).
const RECIPROCAL: [u32; 9] = [0, 65536, 32768, 21846, 16384, 13108, 10923, 9363, 8192];

/// `(sum + count / 2) / count` without a division, for `count <= 8`.
#[inline]
fn divide_rounded(sum: i32, count: i32) -> i32 {
    (((sum + count / 2) as u32 * RECIPROCAL[count as usize]) >> 16) as i32
}

/// `840 / n`: scales `|Σc|² / n` to an exact integer for any `n <= 8`.
const SCALE: [i64; 9] = [0, 840, 420, 280, 210, 168, 140, 120, 105];

/// A fast, near-optimal split for blocks of up to 8 pixels.
///
/// Two far-apart pixels define an axis in RGB (the pixel farthest from the
/// first, then the pixel farthest from that). Pixels are ordered by their
/// projection onto the axis, and every prefix of that order is scored as a
/// split. The best split maximizes `|ΣA|²/nA + |ΣB|²/nB`, which is the same
/// as minimizing the squared error against each part's exact mean. Uniform
/// wins unless a split is strictly better. The mask is returned with the
/// last pixel in the background.
///
/// On photographs and synthetic scenes the total error is within 0.03% of
/// trying every split, at a fraction of the cost (see `docs/benchmarks.md`).
/// All arithmetic is integer and ties resolve by pixel order, so results
/// are the same on every platform.
pub(crate) fn fit_projected<const N: usize>(block: &Block<N>) -> Split {
    debug_assert!((2..=8).contains(&N));
    if let Some(split) = uniform(block) {
        return split;
    }
    let colors =
        block.map(|[red, green, blue]| [i32::from(red), i32::from(green), i32::from(blue)]);

    // An axis through two far-apart pixels: the pixel farthest from pixel 0,
    // then the pixel farthest from that one (the first on ties). Zero
    // distance means a uniform block.
    let farthest_from = |origin: usize| {
        let mut best = (origin, 0);
        for (index, color) in colors.iter().enumerate() {
            let distance = distance_squared(colors[origin], *color);
            if distance > best.1 {
                best = (index, distance);
            }
        }
        best
    };
    let (first, _) = farthest_from(0);
    let (second, spread) = farthest_from(first);
    let farthest = (first, second, spread);
    let total = colors.iter().fold([0; 3], |sum, color| add(sum, *color));
    if farthest.2 == 0 {
        let [red, green, blue] = block[0];
        let uniform = Color::rgb(red, green, blue);
        return Split {
            mask: 0,
            foreground: uniform,
            background: uniform,
        };
    }
    let axis = [
        colors[farthest.1][0] - colors[farthest.0][0],
        colors[farthest.1][1] - colors[farthest.0][1],
        colors[farthest.1][2] - colors[farthest.0][2],
    ];

    // Order pixels by projection (insertion sort, stable for ties).
    let mut order = [0_usize; N];
    let mut keys = [0_i32; N];
    for (index, color) in colors.iter().enumerate() {
        let key = color[0] * axis[0] + color[1] * axis[1] + color[2] * axis[2];
        let mut position = index;
        while position > 0 && keys[position - 1] > key {
            keys[position] = keys[position - 1];
            order[position] = order[position - 1];
            position -= 1;
        }
        keys[position] = key;
        order[position] = index;
    }

    let mut best_score = score(total, N);
    let mut best_mask = 0_u32;
    let mut prefix = [0; 3];
    let mut mask = 0_u32;
    for (count, &index) in order.iter().enumerate().take(N - 1) {
        prefix = add(prefix, colors[index]);
        mask |= 1 << index;
        let rest = [
            total[0] - prefix[0],
            total[1] - prefix[1],
            total[2] - prefix[2],
        ];
        let candidate = score(prefix, count + 1) + score(rest, N - count - 1);
        if candidate > best_score {
            best_score = candidate;
            best_mask = mask;
        }
    }
    if best_mask == 0 {
        let uniform = mean(block, (1 << N) - 1);
        return Split {
            mask: 0,
            foreground: uniform,
            background: uniform,
        };
    }
    if best_mask >> (N - 1) & 1 == 1 {
        best_mask ^= (1 << N) - 1;
    }
    Split {
        mask: best_mask,
        foreground: mean(block, best_mask),
        background: mean(block, best_mask ^ ((1 << N) - 1)),
    }
}

/// A block of one color, which is most blocks of rendered scenes (flat
/// backgrounds, black space). Both fitters give it the uniform split in that
/// color, so they return early with exactly that.
#[inline(always)]
fn uniform<const N: usize>(block: &Block<N>) -> Option<Split> {
    if block.iter().any(|color| *color != block[0]) {
        return None;
    }
    let [red, green, blue] = block[0];
    let color = Color::rgb(red, green, blue);
    Some(Split {
        mask: 0,
        foreground: color,
        background: color,
    })
}

#[inline]
fn add(left: [i32; 3], right: [i32; 3]) -> [i32; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

#[inline]
fn distance_squared(left: [i32; 3], right: [i32; 3]) -> i32 {
    let delta = [left[0] - right[0], left[1] - right[1], left[2] - right[2]];
    delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]
}

#[inline]
fn score(sum: [i32; 3], count: usize) -> i64 {
    let squared = i64::from(sum[0]) * i64::from(sum[0])
        + i64::from(sum[1]) * i64::from(sum[1])
        + i64::from(sum[2]) * i64::from(sum[2]);
    squared * SCALE[count]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rounded-mean squared error of a split: the quantity both fitters
    /// approximate or minimize.
    fn split_error<const N: usize>(block: &Block<N>, mask: u32) -> i64 {
        let all = (1_u32 << N) - 1;
        let part = |mask: u32| -> i64 {
            if mask == 0 {
                return 0;
            }
            let mean = mean(block, mask);
            block
                .iter()
                .enumerate()
                .filter(|(index, _)| mask >> index & 1 == 1)
                .map(|(_, color)| {
                    let delta = |value: u8, mean: u8| (i64::from(value) - i64::from(mean)).pow(2);
                    delta(color[0], mean.red)
                        + delta(color[1], mean.green)
                        + delta(color[2], mean.blue)
                })
                .sum()
        };
        part(mask) + part(all & !mask)
    }

    fn random_blocks<const N: usize>(count: usize, colors: u32) -> Vec<Block<N>> {
        let mut state = 0x2545_F491_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let palette: Vec<[u8; 3]> = (0..colors)
            .map(|_| {
                let value = next();
                [value as u8, (value >> 8) as u8, (value >> 16) as u8]
            })
            .collect();
        (0..count)
            .map(|_| core::array::from_fn(|_| palette[next() as usize % palette.len()]))
            .collect()
    }

    #[test]
    fn reciprocal_division_is_exact_for_every_block_sum() {
        for count in 1..=8 {
            for sum in 0..=8 * 255 {
                assert_eq!(divide_rounded(sum, count), (sum + count / 2) / count);
            }
        }
    }

    #[test]
    fn exact_fit_finds_the_first_minimum_of_every_split() {
        for colors in [2, 3, 5, 1_000_000] {
            for block in random_blocks::<4>(20_000, colors) {
                let split = fit_exact(&block);
                let mut best = (0, split_error(&block, 0));
                for mask in 1..8 {
                    let error = split_error(&block, mask);
                    if error < best.1 {
                        best = (mask, error);
                    }
                }
                assert_eq!(split.mask, best.0, "{block:?}");
                let all = 0b1111;
                if split.mask == 0 {
                    assert_eq!(split.foreground, mean(&block, all));
                    assert_eq!(split.background, split.foreground);
                } else {
                    assert_eq!(split.foreground, mean(&block, split.mask));
                    assert_eq!(split.background, mean(&block, all ^ split.mask));
                }
            }
        }
    }

    #[test]
    fn projected_fit_is_near_optimal_and_well_formed() {
        fn check<const N: usize>() {
            let all = (1_u32 << N) - 1;
            let (mut found, mut optimal) = (0_i64, 0_i64);
            for colors in [2, 3, 1_000_000] {
                for block in random_blocks::<N>(4_000, colors) {
                    let split = fit_projected(&block);
                    assert_eq!(split.mask >> (N - 1) & 1, 0, "last pixel is background");
                    if split.mask == 0 {
                        assert_eq!(split.foreground, split.background);
                    } else {
                        assert_eq!(split.foreground, mean(&block, split.mask));
                        assert_eq!(split.background, mean(&block, all ^ split.mask));
                    }
                    found += split_error(&block, split.mask);
                    optimal += (0..1 << (N - 1))
                        .map(|mask| split_error(&block, mask))
                        .min()
                        .unwrap();
                }
            }
            // Random colors are the hardest case; real images are closer.
            assert!(
                found as f64 <= optimal as f64 * 1.05,
                "{N}: {found} vs {optimal}"
            );
        }
        check::<4>();
        check::<6>();
        check::<8>();
    }

    #[test]
    fn two_color_blocks_split_exactly() {
        // Any block of exactly two colors splits into those two colors.
        for block in random_blocks::<8>(5_000, 2) {
            let split = fit_projected(&block);
            if block.iter().all(|color| *color == block[0]) {
                assert_eq!(split.mask, 0);
                continue;
            }
            assert_eq!(split_error(&block, split.mask), 0, "{block:?}");
        }
        assert_eq!(fit_projected(&[[7, 8, 9]; 6]).mask, 0);
        assert_eq!(fit_exact(&[[7, 8, 9]; 4]).mask, 0);
    }
}
