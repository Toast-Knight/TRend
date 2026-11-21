use crate::cell::Color;
use crate::framebuffer::PixelViewMut;
use crate::pixel_buffer::PixelBuffer;

const BAYER_4X4: [[i16; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

pub fn ordered_4x4(color: Color, x: u32, y: u32, strength: u8) -> Color {
    if color.kind == 0 || strength == 0 {
        return color;
    }
    let threshold = BAYER_4X4[(y & 3) as usize][(x & 3) as usize] * 2 - 15;
    let adjustment = threshold * i16::from(strength) / 15;
    Color::rgb(
        adjust(color.red, adjustment),
        adjust(color.green, adjustment),
        adjust(color.blue, adjustment),
    )
}

/// Applies [`ordered_4x4`] in place to every visible pixel (alpha > 0) of a
/// [`PixelBuffer`], using pixel coordinates. Alpha is left unchanged.
pub fn ordered_4x4_in_place(buffer: &mut PixelBuffer, strength: u8) {
    ordered_4x4_region(&mut buffer.view_mut(), 0, 0, strength);
}

/// Applies [`ordered_4x4`] in place to every visible pixel (alpha > 0) of a
/// region or row band, as if its top-left pixel were at `(origin_x,
/// origin_y)` of the full surface. Alpha is left unchanged.
///
/// Pass the same origin used to render the region (for example the band's
/// first row from [`PixelViewMut::into_row_bands`] or the `x, y` given to
/// [`crate::ImageSource::render_region`]); the result then matches dithering
/// the whole surface at once, so band edges never show a seam.
pub fn ordered_4x4_region(
    target: &mut PixelViewMut<'_>,
    origin_x: u32,
    origin_y: u32,
    strength: u8,
) {
    if strength == 0 {
        return;
    }
    for row in 0..target.height() {
        let y = origin_y.wrapping_add(row);
        for (column, pixel) in target.row_mut(row).chunks_exact_mut(4).enumerate() {
            if pixel[3] == 0 {
                continue;
            }
            let color = ordered_4x4(
                Color::rgb(pixel[0], pixel[1], pixel[2]),
                origin_x.wrapping_add(column as u32),
                y,
                strength,
            );
            pixel[..3].copy_from_slice(&[color.red, color.green, color.blue]);
        }
    }
}

fn adjust(channel: u8, adjustment: i16) -> u8 {
    (i16::from(channel) + adjustment).clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_dither_is_deterministic_and_bounded() {
        let color = Color::rgb(250, 4, 128);
        assert_eq!(ordered_4x4(color, 0, 0, 32), Color::rgb(218, 0, 96));
        assert_eq!(ordered_4x4(color, 4, 4, 32), Color::rgb(218, 0, 96));
    }
}
