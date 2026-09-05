use crate::cell::Color;

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
