use core::slice;
use core::time::Duration;

use crate::{
    Alignment, Cell, FitMode, PixelFormat, PixelFrameView, RenderOptions, RenderRequest,
    RenderTransform, Renderer, SamplingFilter, Viewport,
};

pub const TREND_OK: i32 = 0;
pub const TREND_ERROR_NULL_POINTER: i32 = 1;
pub const TREND_ERROR_INVALID_FRAME: i32 = 2;
pub const TREND_ERROR_INVALID_VIEWPORT: i32 = 3;
pub const TREND_ERROR_OUTPUT_TOO_SMALL: i32 = 4;
pub const TREND_ERROR_RENDER: i32 = 5;
pub const TREND_ERROR_INVALID_OPTIONS: i32 = 6;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct TrendRenderOptions {
    pub struct_size: u32,
    pub sampling: u8,
    pub dither_strength: u8,
    pub horizontal_alignment: u8,
    pub vertical_alignment: u8,
    pub zoom_percent: u16,
    pub reserved: u16,
    pub pan_x_cells: i32,
    pub pan_y_cells: i32,
}

#[unsafe(no_mangle)]
pub extern "C" fn trend_version() -> u32 {
    0x0001_0000
}

#[unsafe(no_mangle)]
pub extern "C" fn trend_cell_size() -> usize {
    size_of::<Cell>()
}

/// Renders an RGBA8 frame directly into caller-owned cells.
///
/// # Safety
///
/// `pixels` must reference `pixels_len` readable bytes. `output_cells` must
/// reference `output_capacity` writable, properly aligned `Cell` values. The
/// two regions must remain valid and must not overlap for the duration of the
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn trend_render_rgba8(
    pixels: *const u8,
    pixels_len: usize,
    source_width: u32,
    source_height: u32,
    source_stride: usize,
    viewport_width: u16,
    viewport_height: u16,
    fit_mode: u8,
    generation: u64,
    output_cells: *mut Cell,
    output_capacity: usize,
) -> i32 {
    unsafe {
        render_rgba8(
            pixels,
            pixels_len,
            source_width,
            source_height,
            source_stride,
            viewport_width,
            viewport_height,
            fit_mode,
            generation,
            RenderOptions::default(),
            output_cells,
            output_capacity,
        )
    }
}

/// Renders an RGBA8 frame with explicit quality and transform options.
///
/// # Safety
///
/// The pointer requirements from [`trend_render_rgba8`] apply. `options` must
/// point to a readable [`TrendRenderOptions`] whose `struct_size` matches the
/// current structure size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn trend_render_rgba8_ex(
    pixels: *const u8,
    pixels_len: usize,
    source_width: u32,
    source_height: u32,
    source_stride: usize,
    viewport_width: u16,
    viewport_height: u16,
    fit_mode: u8,
    generation: u64,
    options: *const TrendRenderOptions,
    output_cells: *mut Cell,
    output_capacity: usize,
) -> i32 {
    if options.is_null() {
        return TREND_ERROR_NULL_POINTER;
    }
    let options = unsafe { *options };
    if options.struct_size as usize != size_of::<TrendRenderOptions>() || options.zoom_percent == 0
    {
        return TREND_ERROR_INVALID_OPTIONS;
    }
    let sampling = match options.sampling {
        0 => SamplingFilter::Nearest,
        1 => SamplingFilter::Bilinear,
        2 => SamplingFilter::Area,
        _ => return TREND_ERROR_INVALID_OPTIONS,
    };
    let horizontal_alignment = match parse_alignment(options.horizontal_alignment) {
        Some(value) => value,
        None => return TREND_ERROR_INVALID_OPTIONS,
    };
    let vertical_alignment = match parse_alignment(options.vertical_alignment) {
        Some(value) => value,
        None => return TREND_ERROR_INVALID_OPTIONS,
    };

    unsafe {
        render_rgba8(
            pixels,
            pixels_len,
            source_width,
            source_height,
            source_stride,
            viewport_width,
            viewport_height,
            fit_mode,
            generation,
            RenderOptions {
                sampling,
                dither_strength: options.dither_strength,
                transform: RenderTransform {
                    zoom_percent: options.zoom_percent,
                    pan_x_cells: options.pan_x_cells,
                    pan_y_cells: options.pan_y_cells,
                    horizontal_alignment,
                    vertical_alignment,
                },
                ..RenderOptions::default()
            },
            output_cells,
            output_capacity,
        )
    }
}

fn parse_alignment(value: u8) -> Option<Alignment> {
    match value {
        0 => Some(Alignment::Start),
        1 => Some(Alignment::Center),
        2 => Some(Alignment::End),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn render_rgba8(
    pixels: *const u8,
    pixels_len: usize,
    source_width: u32,
    source_height: u32,
    source_stride: usize,
    viewport_width: u16,
    viewport_height: u16,
    fit_mode: u8,
    generation: u64,
    options: RenderOptions,
    output_cells: *mut Cell,
    output_capacity: usize,
) -> i32 {
    if pixels.is_null() || output_cells.is_null() {
        return TREND_ERROR_NULL_POINTER;
    }
    if viewport_width == 0 || viewport_height == 0 {
        return TREND_ERROR_INVALID_VIEWPORT;
    }
    let required = usize::from(viewport_width) * usize::from(viewport_height);
    if output_capacity < required {
        return TREND_ERROR_OUTPUT_TOO_SMALL;
    }

    let fit = match fit_mode {
        0 => FitMode::Contain,
        1 => FitMode::Cover,
        2 => FitMode::Stretch,
        _ => return TREND_ERROR_INVALID_VIEWPORT,
    };
    let pixel_slice = unsafe { slice::from_raw_parts(pixels, pixels_len) };
    let source = match PixelFrameView::new(
        pixel_slice,
        source_width,
        source_height,
        source_stride,
        PixelFormat::Rgba8,
        Duration::ZERO,
    ) {
        Ok(source) => source,
        Err(_) => return TREND_ERROR_INVALID_FRAME,
    };

    let output = unsafe { slice::from_raw_parts_mut(output_cells, required) };
    if Renderer::new()
        .render_to_slice_with_options(
            source,
            RenderRequest {
                viewport: Viewport {
                    width: viewport_width,
                    height: viewport_height,
                },
                fit,
                generation,
            },
            options,
            output,
        )
        .is_err()
    {
        return TREND_ERROR_RENDER;
    }

    TREND_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    #[test]
    fn c_api_renders_into_caller_buffer() {
        let pixels = [255, 0, 0, 255, 0, 0, 255, 255];
        let mut cells = [Cell::default(); 1];
        let result = unsafe {
            trend_render_rgba8(
                pixels.as_ptr(),
                pixels.len(),
                1,
                2,
                4,
                1,
                1,
                0,
                4,
                cells.as_mut_ptr(),
                cells.len(),
            )
        };

        assert_eq!(result, TREND_OK);
        assert_eq!(cells[0].glyph, '▀' as u32);
        assert_eq!(cells[0].foreground.red, 255);
        assert_eq!(cells[0].background.blue, 255);
    }

    #[test]
    fn c_api_distinguishes_transparent_pixels_from_opaque_black() {
        let transparent = [0, 0, 0, 0, 255, 0, 0, 0];
        let opaque_black = [0, 0, 0, 255, 0, 0, 0, 255];
        let mut transparent_cell = Cell::half_block(Color::rgb(1, 2, 3), Color::rgb(4, 5, 6));
        let mut black_cell = Cell::default();

        for (pixels, output) in [
            (&transparent[..], &mut transparent_cell),
            (&opaque_black[..], &mut black_cell),
        ] {
            let result = unsafe {
                trend_render_rgba8(
                    pixels.as_ptr(),
                    pixels.len(),
                    1,
                    2,
                    4,
                    1,
                    1,
                    0,
                    1,
                    output,
                    1,
                )
            };
            assert_eq!(result, TREND_OK);
        }

        assert_eq!(transparent_cell, Cell::default());
        assert_eq!(black_cell.foreground, Color::rgb(0, 0, 0));
        assert_eq!(black_cell.background, Color::rgb(0, 0, 0));
        assert_ne!(transparent_cell, black_cell);
    }

    #[test]
    fn c_api_rejects_small_output_without_writing() {
        let pixels = [0_u8; 8];
        let mut cell = Cell::default();
        let result = unsafe {
            trend_render_rgba8(
                pixels.as_ptr(),
                pixels.len(),
                1,
                2,
                4,
                2,
                1,
                0,
                0,
                &mut cell,
                1,
            )
        };

        assert_eq!(result, TREND_ERROR_OUTPUT_TOO_SMALL);
        assert_eq!(cell, Cell::default());
    }

    #[test]
    fn extended_c_api_validates_and_applies_options() {
        let pixels = [
            64, 64, 64, 255, 192, 64, 64, 255, 64, 192, 64, 255, 192, 192, 192, 255,
        ];
        let mut cells = [Cell::default(); 6];
        let options = TrendRenderOptions {
            struct_size: size_of::<TrendRenderOptions>() as u32,
            sampling: 1,
            dither_strength: 16,
            horizontal_alignment: 1,
            vertical_alignment: 1,
            zoom_percent: 100,
            reserved: 0,
            pan_x_cells: 0,
            pan_y_cells: 0,
        };
        let result = unsafe {
            trend_render_rgba8_ex(
                pixels.as_ptr(),
                pixels.len(),
                2,
                2,
                8,
                3,
                2,
                2,
                1,
                &options,
                cells.as_mut_ptr(),
                cells.len(),
            )
        };

        assert_eq!(result, TREND_OK);
        assert_ne!(cells[0], Cell::default());
    }
}
