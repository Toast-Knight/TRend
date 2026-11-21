//! Deterministic inputs and hashing shared by the golden and equivalence tests.
//!
//! Every value here is fixed. Changing a pattern, the matrix, or the hash
//! invalidates the recorded constants in `golden_render.rs` and
//! `golden_ansi.rs`, so treat this file as part of the baseline.

#![allow(dead_code)]

pub mod screen;

use core::time::Duration;

use trend::{
    Alignment, Cell, Color, FitMode, PixelFormat, PixelFrameView, RenderOptions, RenderRequest,
    RenderTransform, SamplingFilter, Viewport,
};

/// Environment variable that makes golden tests print their current values
/// instead of asserting. Only use it to record an intentional baseline change.
pub const RECORD_ENV: &str = "TREND_RECORD_GOLDEN";

pub fn recording() -> bool {
    std::env::var_os(RECORD_ENV).is_some()
}

/// An owned RGBA8 test image, optionally with padded rows.
pub struct Pattern {
    pub name: &'static str,
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
}

impl Pattern {
    fn build(
        name: &'static str,
        width: u32,
        height: u32,
        row_padding: usize,
        pixel: impl Fn(u32, u32) -> [u8; 4],
    ) -> Self {
        let stride = width as usize * 4 + row_padding;
        let mut pixels = vec![0xA5; stride * height as usize];
        for y in 0..height {
            for x in 0..width {
                let offset = y as usize * stride + x as usize * 4;
                pixels[offset..offset + 4].copy_from_slice(&pixel(x, y));
            }
        }
        Self {
            name,
            pixels,
            width,
            height,
            stride,
        }
    }

    pub fn view(&self) -> PixelFrameView<'_> {
        PixelFrameView::new(
            &self.pixels,
            self.width,
            self.height,
            self.stride,
            PixelFormat::Rgba8,
            Duration::ZERO,
        )
        .unwrap()
    }
}

/// Five inputs covering opaque gradients, every alpha class, degenerate
/// aspect ratios, padded strides, and pseudo-random content.
pub fn patterns() -> Vec<Pattern> {
    vec![
        Pattern::build("gradient", 37, 23, 0, |x, y| {
            [
                (x * 255 / 36) as u8,
                (y * 255 / 22) as u8,
                ((x * 7 + y * 11) % 256) as u8,
                255,
            ]
        }),
        Pattern::build("alpha-checker", 16, 12, 4, |x, y| {
            let alpha = [0, 64, 128, 255][((x / 4 + y / 4) % 4) as usize];
            [(x * 16) as u8, (200 - y * 10) as u8, 90, alpha]
        }),
        Pattern::build("tall", 1, 4, 0, |_, y| {
            [
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 0],
                [250, 250, 10, 255],
            ][y as usize]
        }),
        Pattern::build("wide", 300, 7, 0, |x, y| {
            if (x / 5 + y) % 2 == 0 {
                [x as u8, 30, 200, 255]
            } else {
                [10, (x / 2) as u8, (y * 30) as u8, 255]
            }
        }),
        Pattern::build("noise", 64, 64, 8, |x, y| {
            let mut state = (u64::from(y) * 64 + u64::from(x)).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ 0xD1B5_4A32_D192_ED03;
            state ^= state >> 29;
            state = state.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            state ^= state >> 32;
            let bytes = state.to_le_bytes();
            let alpha = match bytes[3] % 4 {
                0 => 0,
                1 => bytes[4],
                _ => 255,
            };
            [bytes[0], bytes[1], bytes[2], alpha]
        }),
    ]
}

pub const VIEWPORTS: [Viewport; 4] = [
    Viewport {
        width: 1,
        height: 1,
    },
    Viewport {
        width: 3,
        height: 2,
    },
    Viewport {
        width: 17,
        height: 9,
    },
    Viewport {
        width: 80,
        height: 24,
    },
];

pub const FITS: [FitMode; 3] = [FitMode::Contain, FitMode::Cover, FitMode::Stretch];

pub const FILTERS: [SamplingFilter; 3] = [
    SamplingFilter::Nearest,
    SamplingFilter::Bilinear,
    SamplingFilter::Area,
];

pub fn transforms() -> [RenderTransform; 4] {
    [
        RenderTransform::default(),
        RenderTransform {
            zoom_percent: 250,
            pan_x_cells: 3,
            pan_y_cells: -2,
            horizontal_alignment: Alignment::Start,
            vertical_alignment: Alignment::End,
        },
        RenderTransform {
            zoom_percent: 50,
            ..RenderTransform::default()
        },
        RenderTransform {
            zoom_percent: 100,
            pan_x_cells: -5,
            pan_y_cells: 4,
            horizontal_alignment: Alignment::End,
            vertical_alignment: Alignment::Start,
        },
    ]
}

pub const DITHERS: [u8; 2] = [0, 16];

pub const MATTES: [Option<Color>; 2] = [None, Some(Color::rgb(20, 40, 60))];

/// One combination of the Phase 0 matrix.
#[derive(Clone, Copy, Debug)]
pub struct Case {
    pub pattern: usize,
    pub filter: usize,
    pub viewport: Viewport,
    pub fit: FitMode,
    pub transform: RenderTransform,
    pub dither: u8,
    pub matte: Option<Color>,
    pub generation: u64,
}

impl Case {
    pub fn request(&self) -> RenderRequest {
        RenderRequest {
            viewport: self.viewport,
            fit: self.fit,
            generation: self.generation,
        }
    }

    pub fn options(&self) -> RenderOptions {
        RenderOptions {
            sampling: FILTERS[self.filter],
            dither_strength: self.dither,
            transform: self.transform,
            alpha_background: self.matte,
        }
    }
}

/// The full 5 × 4 × 3 × 3 × 4 × 2 × 2 = 2,880-case matrix, in a fixed order.
pub fn cases() -> Vec<Case> {
    let mut cases = Vec::with_capacity(2_880);
    let mut generation = 0;
    for pattern in 0..5 {
        for filter in 0..FILTERS.len() {
            for viewport in VIEWPORTS {
                for fit in FITS {
                    for transform in transforms() {
                        for dither in DITHERS {
                            for matte in MATTES {
                                generation += 1;
                                cases.push(Case {
                                    pattern,
                                    filter,
                                    viewport,
                                    fit,
                                    transform,
                                    dither,
                                    matte,
                                    generation,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    cases
}

/// 64-bit FNV-1a. Stable across platforms and Rust versions.
#[derive(Clone, Copy, Debug)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xCBF2_9CE4_8422_2325)
    }
}

impl Fnv {
    pub fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    pub fn write_u64(&mut self, value: u64) {
        self.write(&value.to_le_bytes());
    }

    pub fn write_color(&mut self, color: Color) {
        self.write(&[color.red, color.green, color.blue, color.kind]);
    }

    pub fn write_cells(&mut self, cells: &[Cell]) {
        self.write_u64(cells.len() as u64);
        for cell in cells {
            self.write(&cell.glyph.to_le_bytes());
            self.write_color(cell.foreground);
            self.write_color(cell.background);
            self.write(&[cell.attributes]);
        }
    }

    pub fn finish(self) -> u64 {
        self.0
    }
}
