//! Colour space helpers: lighting happens in linear light, textures and the screen are sRGB.

use std::sync::OnceLock;

/// sRGB byte to linear 0..1.
pub fn srgb_to_linear(value: u8) -> f32 {
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let value = index as f32 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        })
    })[usize::from(value)]
}

/// sRGB channel given as a 0..255 float (from bilinear sampling) to linear 0..1.
pub fn srgb_float_to_linear(value: f32) -> f32 {
    let clamped = value.clamp(0.0, 255.0);
    let low = clamped.floor();
    let fraction = clamped - low;
    let a = srgb_to_linear(low as u8);
    if fraction == 0.0 || low >= 255.0 {
        return a;
    }
    a + (srgb_to_linear(low as u8 + 1) - a) * fraction
}

/// Linear 0..1 to an sRGB byte.
pub fn linear_to_srgb(value: f32) -> u8 {
    const STEPS: usize = 4096;
    static TABLE: OnceLock<Vec<u8>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        (0..=STEPS)
            .map(|index| {
                let value = index as f32 / STEPS as f32;
                let encoded = if value <= 0.003_130_8 {
                    value * 12.92
                } else {
                    1.055 * value.powf(1.0 / 2.4) - 0.055
                };
                (encoded * 255.0).round().clamp(0.0, 255.0) as u8
            })
            .collect()
    });
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    table[((value.min(1.0)) * STEPS as f32) as usize]
}

/// Filmic tone curve (ACES fit by Narkowicz): bright light rolls off instead of clipping.
pub fn tone_map(value: f32) -> f32 {
    let x = value.max(0.0);
    ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_round_trip_and_tone_map_is_monotonic() {
        for value in [0_u8, 1, 10, 128, 200, 255] {
            assert_eq!(linear_to_srgb(srgb_to_linear(value)), value);
        }
        assert!(
            (srgb_float_to_linear(127.5) - (srgb_to_linear(127) + srgb_to_linear(128)) / 2.0).abs()
                < 1e-6
        );
        let mut last = 0.0;
        for step in 0..100 {
            let mapped = tone_map(step as f32 * 0.1);
            assert!(mapped >= last);
            last = mapped;
        }
        assert!(tone_map(100.0) <= 1.0);
        assert_eq!(linear_to_srgb(f32::NAN), 0);
    }
}
