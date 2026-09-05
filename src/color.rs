use crate::cell::Color;

const ANSI_16: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (128, 0, 0),
    (0, 128, 0),
    (128, 128, 0),
    (0, 0, 128),
    (128, 0, 128),
    (0, 128, 128),
    (192, 192, 192),
    (128, 128, 128),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (0, 0, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

pub const fn ansi256_index(color: Color) -> u8 {
    let red = quantize_six(color.red);
    let green = quantize_six(color.green);
    let blue = quantize_six(color.blue);
    16 + 36 * red + 6 * green + blue
}

pub fn ansi16_index(color: Color) -> u8 {
    let mut best_index = 0;
    let mut best_distance = u32::MAX;
    for (index, &(red, green, blue)) in ANSI_16.iter().enumerate() {
        let red_delta = i32::from(color.red) - i32::from(red);
        let green_delta = i32::from(color.green) - i32::from(green);
        let blue_delta = i32::from(color.blue) - i32::from(blue);
        let distance =
            (red_delta * red_delta + green_delta * green_delta + blue_delta * blue_delta) as u32;
        if distance < best_distance {
            best_distance = distance;
            best_index = index as u8;
        }
    }
    best_index
}

const fn quantize_six(value: u8) -> u8 {
    ((value as u16 * 5 + 127) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_rgb_cube_extremes_to_ansi256() {
        assert_eq!(ansi256_index(Color::rgb(0, 0, 0)), 16);
        assert_eq!(ansi256_index(Color::rgb(255, 255, 255)), 231);
        assert_eq!(ansi256_index(Color::rgb(255, 0, 0)), 196);
    }

    #[test]
    fn converts_primary_colors_to_ansi16() {
        assert_eq!(ansi16_index(Color::rgb(255, 0, 0)), 9);
        assert_eq!(ansi16_index(Color::rgb(0, 0, 255)), 12);
    }
}
