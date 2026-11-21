//! Pixel compositing: placing one image over another before encoding.
//!
//! This is the one blend TRend needs to put sprites, HUD graphics, or a
//! logo over rendered or decoded frames. It is not a compositor: there are
//! no layers or scene, just an operation on two views that the host calls
//! in whatever order it wants.

use crate::framebuffer::{PixelFrameView, PixelViewMut, decode, with_decoder};
use crate::geometry::PixelRect;

/// Draws `source` over `target` with its top-left at `(x, y)` in `target`,
/// using straight-alpha source-over compositing, and returns the target
/// rectangle that was touched (`None` if the source lies entirely outside).
///
/// - `source` may use any [`crate::PixelLayout`]; it's read through
///   [`PixelFrameView::rgba`], so premultiplied or BGRA overlays work.
/// - `(x, y)` may be negative or extend past the target; the source is
///   clipped.
/// - Each result is the exactly rounded straight-alpha "over" of the two
///   pixels: `αo = αs + αd(1 − αs)`, `Co = (Cs·αs + Cd·αd(1 − αs)) / αo`.
///   Opaque source pixels replace the target and transparent ones leave it
///   untouched, so an opaque source is a plain copy.
///
/// The returned rectangle is the damage to hand to
/// [`crate::Surface::encode_region`]. Nothing is allocated.
pub fn blend_over(
    source: PixelFrameView<'_>,
    target: &mut PixelViewMut<'_>,
    x: i32,
    y: i32,
) -> Option<PixelRect> {
    // Clip in i64 so no offset can overflow.
    let left = i64::from(x).max(0);
    let top = i64::from(y).max(0);
    let right = (i64::from(x) + i64::from(source.width())).min(i64::from(target.width()));
    let bottom = (i64::from(y) + i64::from(source.height())).min(i64::from(target.height()));
    if left >= right || top >= bottom {
        return None;
    }
    let area = PixelRect::new(
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    );
    let source_x = (left - i64::from(x)) as u32;
    let source_y = (top - i64::from(y)) as u32;
    with_decoder!(source, D => {
        for row in 0..area.height {
            let target_row = &mut target.row_mut(area.y + row)
                [area.x as usize * 4..(area.x + area.width) as usize * 4];
            let Some(source_row) = source.row(source_y + row) else {
                continue;
            };
            let source_row =
                &source_row[source_x as usize * 4..(source_x + area.width) as usize * 4];
            for (pixel, over) in target_row
                .chunks_exact_mut(4)
                .zip(source_row.chunks_exact(4))
            {
                blend_pixel(decode::<D>(over), pixel);
            }
        }
    });
    Some(area)
}

/// `pixel = source over pixel`, straight alpha, exactly rounded.
#[inline]
fn blend_pixel(source: [u8; 4], pixel: &mut [u8]) {
    let source_alpha = u32::from(source[3]);
    match source_alpha {
        0 => {}
        255 => pixel.copy_from_slice(&source),
        _ => {
            let target_alpha = u32::from(pixel[3]);
            let remaining = target_alpha * (255 - source_alpha);
            // 255 × the output alpha, kept unrounded for the color division.
            let alpha_255 = source_alpha * 255 + remaining;
            for channel in 0..3 {
                let numerator = u32::from(source[channel]) * source_alpha * 255
                    + u32::from(pixel[channel]) * remaining;
                pixel[channel] = ((numerator + alpha_255 / 2) / alpha_255) as u8;
            }
            pixel[3] = ((alpha_255 + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same operation in floating point, rounded.
    fn reference(source: [u8; 4], target: [u8; 4]) -> [u8; 4] {
        let source_alpha = f64::from(source[3]) / 255.0;
        let target_alpha = f64::from(target[3]) / 255.0;
        let alpha = source_alpha + target_alpha * (1.0 - source_alpha);
        if alpha == 0.0 {
            return target;
        }
        let channel = |index: usize| {
            ((f64::from(source[index]) * source_alpha
                + f64::from(target[index]) * target_alpha * (1.0 - source_alpha))
                / alpha)
                .round() as u8
        };
        [
            channel(0),
            channel(1),
            channel(2),
            (alpha * 255.0).round() as u8,
        ]
    }

    #[test]
    fn blending_matches_the_rounded_formula() {
        let mut state = 0x9E37_79B9_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let mut mismatched = 0;
        for _ in 0..200_000 {
            let (a, b) = (next(), next());
            let source = a.to_le_bytes();
            let mut target = b.to_le_bytes();
            let expected = reference(source, target);
            blend_pixel(source, &mut target);
            // Floating point can land on the other side of an exact .5.
            let off = target
                .iter()
                .zip(expected)
                .any(|(got, want)| got.abs_diff(want) > 1);
            assert!(!off, "{source:?} over {b:?}: {target:?} vs {expected:?}");
            mismatched += usize::from(target != expected);
        }
        assert!(mismatched < 200, "{mismatched} rounding differences");
    }

    #[test]
    fn opaque_and_transparent_sources_copy_and_skip() {
        let mut pixel = [10, 20, 30, 40];
        blend_pixel([1, 2, 3, 0], &mut pixel);
        assert_eq!(pixel, [10, 20, 30, 40]);
        blend_pixel([1, 2, 3, 255], &mut pixel);
        assert_eq!(pixel, [1, 2, 3, 255]);
        // Over a transparent target, the source is unchanged.
        let mut pixel = [99, 99, 99, 0];
        blend_pixel([200, 100, 50, 77], &mut pixel);
        assert_eq!(pixel, [200, 100, 50, 77]);
        // Over an opaque target, the result is opaque.
        let mut pixel = [0, 0, 0, 255];
        blend_pixel([255, 255, 255, 128], &mut pixel);
        assert_eq!(pixel, [128, 128, 128, 255]);
    }
}
