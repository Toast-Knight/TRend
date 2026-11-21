//! M2a contracts for borrowed frames: layout validation, explicit alpha
//! semantics, BGRA, bottom-up rows, padded strides, and the rule that every
//! consumer (encoders and `ImageSource`) reads any layout identically.

mod common;

use core::time::Duration;

use trend::{
    AlphaMode, CellAspect, CellEncoder, CellFrame, FitMode, FrameError, HalfBlock, ImageSource,
    PixelBuffer, PixelFormat, PixelFrameView, PixelLayout, PixelRect, Quadrant, RasterSpec,
    RegionError, RowOrder, SamplingFilter, Viewport,
};

const FORMATS: [PixelFormat; 2] = [PixelFormat::Rgba8, PixelFormat::Bgra8];
const ROW_ORDERS: [RowOrder; 2] = [RowOrder::TopDown, RowOrder::BottomUp];
const ALPHA_MODES: [AlphaMode; 3] = [
    AlphaMode::Straight,
    AlphaMode::Premultiplied,
    AlphaMode::Ignore,
];

fn one(bytes: [u8; 4], format: PixelFormat, alpha: AlphaMode) -> [u8; 4] {
    let layout = PixelLayout::new(1, 1, format, alpha);
    PixelFrameView::from_layout(&bytes, layout)
        .unwrap()
        .rgba(0, 0)
        .unwrap()
}

// ---------------------------------------------------------------------------
// Alpha and channel semantics
// ---------------------------------------------------------------------------

#[test]
fn straight_alpha_is_read_as_stored_including_color_under_zero_alpha() {
    assert_eq!(
        one([10, 20, 30, 40], PixelFormat::Rgba8, AlphaMode::Straight),
        [10, 20, 30, 40]
    );
    assert_eq!(
        one([10, 20, 30, 0], PixelFormat::Rgba8, AlphaMode::Straight),
        [10, 20, 30, 0]
    );
}

#[test]
fn bgra_swaps_red_and_blue_only() {
    assert_eq!(
        one([30, 20, 10, 40], PixelFormat::Bgra8, AlphaMode::Straight),
        [10, 20, 30, 40]
    );
}

#[test]
fn ignored_alpha_makes_every_pixel_opaque() {
    // The typical BGRX capture: the fourth byte is 0 but means nothing.
    assert_eq!(
        one([30, 20, 10, 0], PixelFormat::Bgra8, AlphaMode::Ignore),
        [10, 20, 30, 255]
    );
    assert_eq!(
        one([1, 2, 3, 77], PixelFormat::Rgba8, AlphaMode::Ignore),
        [1, 2, 3, 255]
    );
}

#[test]
fn premultiplied_alpha_is_divided_out_with_rounding() {
    assert_eq!(
        one(
            [64, 32, 0, 128],
            PixelFormat::Rgba8,
            AlphaMode::Premultiplied
        ),
        [128, 64, 0, 128]
    );
    assert_eq!(
        one(
            [0, 32, 64, 128],
            PixelFormat::Bgra8,
            AlphaMode::Premultiplied
        ),
        [128, 64, 0, 128]
    );
    assert_eq!(
        one([9, 8, 7, 255], PixelFormat::Rgba8, AlphaMode::Premultiplied),
        [9, 8, 7, 255]
    );
}

#[test]
fn premultiplied_zero_alpha_is_fully_transparent_and_invalid_channels_clamp() {
    assert_eq!(
        one([5, 6, 7, 0], PixelFormat::Rgba8, AlphaMode::Premultiplied),
        [0, 0, 0, 0]
    );
    // A channel above alpha is invalid premultiplied data; it clamps to alpha.
    assert_eq!(
        one(
            [200, 50, 0, 100],
            PixelFormat::Rgba8,
            AlphaMode::Premultiplied
        ),
        [255, 128, 0, 100]
    );
}

#[test]
fn premultiply_round_trip_error_is_bounded_by_quantization() {
    for alpha in 1_u16..=255 {
        for channel in 0_u16..=255 {
            let stored = ((channel * alpha + 127) / 255) as u8;
            let read = one(
                [stored, 0, 0, alpha as u8],
                PixelFormat::Rgba8,
                AlphaMode::Premultiplied,
            )[0];
            let error = i32::from(read).abs_diff(i32::from(channel));
            // One premultiplied step is 255 / alpha straight units.
            assert!(
                error <= u32::from(255 / (2 * alpha) + 1),
                "channel {channel} alpha {alpha} read back as {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Row order, stride, and validation
// ---------------------------------------------------------------------------

#[test]
fn bottom_up_rows_are_addressed_top_down() {
    // Memory holds rows A, B, C; the image is C (top), B, A (bottom).
    let memory = [1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255];
    let layout = PixelLayout::new(1, 3, PixelFormat::Rgba8, AlphaMode::Straight)
        .with_row_order(RowOrder::BottomUp);
    let frame = PixelFrameView::from_layout(&memory, layout).unwrap();

    assert_eq!(frame.rgba(0, 0), Some([3, 0, 0, 255]));
    assert_eq!(frame.rgba(0, 2), Some([1, 0, 0, 255]));
    assert_eq!(frame.row(0), Some(&memory[8..12]));
    assert_eq!(frame.row(3), None);
    assert_eq!(frame.rgba(1, 0), None);
}

#[test]
fn layout_validation_matches_the_original_constructor_rules() {
    let bytes = [0_u8; 64];
    let layout =
        |width, height| PixelLayout::new(width, height, PixelFormat::Bgra8, AlphaMode::Ignore);

    assert_eq!(
        PixelFrameView::from_layout(&bytes, layout(0, 1)).unwrap_err(),
        FrameError::ZeroDimension
    );
    assert_eq!(
        PixelFrameView::from_layout(&bytes, layout(4, 1).with_stride(15)).unwrap_err(),
        FrameError::StrideTooSmall
    );
    // Three rows of stride 24 need 2 * 24 + 16 = 64 bytes: the final row
    // needs no padding, whatever the row order.
    for order in ROW_ORDERS {
        let padded = layout(4, 3).with_stride(24).with_row_order(order);
        assert!(PixelFrameView::from_layout(&bytes, padded).is_ok());
        assert_eq!(
            PixelFrameView::from_layout(&bytes[..63], padded).unwrap_err(),
            FrameError::BufferTooSmall
        );
    }
    assert_eq!(
        PixelFrameView::from_layout(&bytes, layout(1, 3).with_stride(usize::MAX)).unwrap_err(),
        FrameError::SizeOverflow
    );
}

#[test]
fn original_constructor_means_top_down_straight_alpha() {
    let bytes = [1, 2, 3, 0, 5, 6, 7, 8];
    let legacy = PixelFrameView::new(
        &bytes,
        2,
        1,
        8,
        PixelFormat::Rgba8,
        Duration::from_millis(40),
    )
    .unwrap();
    let layout = legacy.layout();

    assert_eq!(
        layout,
        PixelLayout::new(2, 1, PixelFormat::Rgba8, AlphaMode::Straight)
    );
    assert_eq!(legacy.row_order(), RowOrder::TopDown);
    assert_eq!(legacy.alpha_mode(), AlphaMode::Straight);
    assert_eq!(legacy.timestamp(), Duration::from_millis(40));
    assert_eq!(legacy.rgba(0, 0), Some([1, 2, 3, 0]));
    assert_eq!(
        PixelFrameView::from_layout(&bytes, layout)
            .unwrap()
            .with_timestamp(Duration::from_millis(40))
            .timestamp(),
        legacy.timestamp()
    );
}

#[test]
fn sub_views_of_bottom_up_frames_keep_logical_coordinates() {
    let (width, height) = (5_u32, 4_u32);
    let stride = 5 * 4 + 8;
    let mut memory = vec![0_u8; stride * height as usize];
    for memory_row in 0..height {
        for x in 0..width {
            let offset = memory_row as usize * stride + x as usize * 4;
            memory[offset..offset + 4].copy_from_slice(&[x as u8, memory_row as u8, 9, 255]);
        }
    }
    let layout = PixelLayout::new(width, height, PixelFormat::Rgba8, AlphaMode::Straight)
        .with_stride(stride)
        .with_row_order(RowOrder::BottomUp);
    let frame = PixelFrameView::from_layout(&memory, layout).unwrap();
    let sub = frame.sub(PixelRect::new(1, 1, 3, 2)).unwrap();
    let nested = sub.sub(PixelRect::new(2, 1, 1, 1)).unwrap();

    assert_eq!((sub.width(), sub.height(), sub.stride()), (3, 2, stride));
    assert_eq!(sub.row_order(), RowOrder::BottomUp);
    for y in 0..2 {
        for x in 0..3 {
            assert_eq!(sub.rgba(x, y), frame.rgba(x + 1, y + 1));
        }
    }
    // Logical (3, 2) of the frame is memory row 4 - 1 - 2 = 1.
    assert_eq!(nested.rgba(0, 0), Some([3, 1, 9, 255]));
    // Sub-views borrow the same memory.
    let range = memory.as_ptr_range();
    assert!(range.contains(&sub.pixels().as_ptr()));

    assert_eq!(
        frame.sub(PixelRect::new(0, 0, 0, 1)).unwrap_err(),
        RegionError::Empty
    );
    assert_eq!(
        frame.sub(PixelRect::new(3, 0, 3, 1)).unwrap_err(),
        RegionError::OutOfBounds
    );
    assert_eq!(
        frame.sub(PixelRect::new(u32::MAX, 0, 2, 1)).unwrap_err(),
        RegionError::OutOfBounds
    );
}

// ---------------------------------------------------------------------------
// Every consumer reads every layout the same way
// ---------------------------------------------------------------------------

/// Encodes `canonical` (straight, top-down RGBA) in another layout, with
/// junk alpha for `Ignore` and a padded stride.
fn encode_layout(
    canonical: &common::Pattern,
    format: PixelFormat,
    row_order: RowOrder,
    alpha: AlphaMode,
) -> (Vec<u8>, PixelLayout) {
    let (width, height) = (canonical.width, canonical.height);
    let stride = width as usize * 4 + 12;
    let mut bytes = vec![0xEE; stride * height as usize];
    let source = canonical.view();
    for y in 0..height {
        let memory_row = match row_order {
            RowOrder::BottomUp => height - 1 - y,
            _ => y,
        };
        for x in 0..width {
            let [red, green, blue, straight_alpha] = source.rgba(x, y).unwrap();
            let [red, green, blue, stored_alpha] = match alpha {
                AlphaMode::Premultiplied => {
                    let scale =
                        |c: u8| ((u16::from(c) * u16::from(straight_alpha) + 127) / 255) as u8;
                    [scale(red), scale(green), scale(blue), straight_alpha]
                }
                AlphaMode::Ignore => [red, green, blue, (x ^ y) as u8],
                _ => [red, green, blue, straight_alpha],
            };
            let ordered = match format {
                PixelFormat::Bgra8 => [blue, green, red, stored_alpha],
                _ => [red, green, blue, stored_alpha],
            };
            let offset = memory_row as usize * stride + x as usize * 4;
            bytes[offset..offset + 4].copy_from_slice(&ordered);
        }
    }
    let layout = PixelLayout::new(width, height, format, alpha)
        .with_stride(stride)
        .with_row_order(row_order);
    (bytes, layout)
}

/// The straight, top-down RGBA image a view reads as.
fn normalized(frame: &PixelFrameView<'_>) -> PixelBuffer {
    let mut buffer = PixelBuffer::new(frame.width(), frame.height()).unwrap();
    let width = frame.width();
    for (index, pixel) in buffer.pixels_mut().chunks_exact_mut(4).enumerate() {
        let (x, y) = (index as u32 % width, index as u32 / width);
        pixel.copy_from_slice(&frame.rgba(x, y).unwrap());
    }
    buffer
}

#[test]
fn every_layout_reads_back_as_the_same_straight_image() {
    for pattern in common::patterns() {
        let canonical = pattern.view();
        for format in FORMATS {
            for row_order in ROW_ORDERS {
                for alpha in [AlphaMode::Straight, AlphaMode::Ignore] {
                    let (bytes, layout) = encode_layout(&pattern, format, row_order, alpha);
                    let frame = PixelFrameView::from_layout(&bytes, layout).unwrap();
                    for y in 0..pattern.height {
                        for x in 0..pattern.width {
                            let mut expected = canonical.rgba(x, y).unwrap();
                            if alpha == AlphaMode::Ignore {
                                expected[3] = 255;
                            }
                            assert_eq!(frame.rgba(x, y), Some(expected), "{layout:?} ({x},{y})");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn image_source_reads_every_layout_through_the_same_accessor() {
    let viewport = Viewport {
        width: 17,
        height: 9,
    };
    for pattern in common::patterns() {
        for format in FORMATS {
            for row_order in ROW_ORDERS {
                for alpha in ALPHA_MODES {
                    let (bytes, layout) = encode_layout(&pattern, format, row_order, alpha);
                    let frame = PixelFrameView::from_layout(&bytes, layout).unwrap();
                    let reference = normalized(&frame);
                    for filter in common::FILTERS {
                        for fit in common::FITS {
                            let spec =
                                RasterSpec::new(viewport, Quadrant.grid(), CellAspect::default())
                                    .unwrap();
                            let render = |view| {
                                let mut out = PixelBuffer::new(1, 1).unwrap();
                                ImageSource::new(view)
                                    .with_fit(fit)
                                    .with_sampling(filter)
                                    .render_into(&spec, &mut out)
                                    .unwrap();
                                out
                            };
                            assert_eq!(
                                render(frame),
                                render(reference.view()),
                                "{} {layout:?} {filter:?} {fit:?}",
                                pattern.name
                            );
                        }
                    }
                }
            }
        }
    }
}

/// A deterministic image of exactly `width` × `height` with all alpha classes.
fn exact_image(width: u32, height: u32) -> common::Pattern {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let alpha = match (x * 7 + y * 3) % 5 {
                0 => 0,
                1 => 90,
                _ => 255,
            };
            pixels.extend_from_slice(&[
                (x * 29) as u8,
                (y * 53) as u8,
                ((x + y) * 17) as u8,
                alpha,
            ]);
        }
    }
    common::Pattern {
        name: "exact",
        pixels,
        width,
        height,
        stride: width as usize * 4,
    }
}

/// A surface showing part of a larger foreign framebuffer (an editor
/// viewport, a split screen, a cropped capture) reads the window in place.
#[test]
fn a_window_into_a_larger_framebuffer_encodes_in_place() {
    let viewport = Viewport {
        width: 17,
        height: 9,
    };
    let encoders: [Box<dyn CellEncoder>; 3] = [
        Box::new(HalfBlock),
        Box::new(trend::Sextant),
        Box::new(trend::Braille),
    ];
    for encoder in encoders {
        let mut surface = trend::Surface::new(
            encoder,
            viewport,
            trend::TerminalCapabilities {
                color: trend::TerminalColorMode::TrueColor,
                unicode: true,
            },
        )
        .unwrap();
        let (width, height) = (surface.spec().width(), surface.spec().height());
        let whole = exact_image(width + 21, height + 13);
        for row_order in ROW_ORDERS {
            let (bytes, layout) = encode_layout(
                &whole,
                PixelFormat::Bgra8,
                row_order,
                AlphaMode::Premultiplied,
            );
            let framebuffer = PixelFrameView::from_layout(&bytes, layout).unwrap();
            let window = framebuffer
                .sub(PixelRect::new(13, 5, width, height))
                .unwrap();
            // Too large as a whole, exact as a window.
            assert!(surface.encode_from(framebuffer).is_err());
            surface.encode_from(window).unwrap();

            let reference = normalized(&window);
            let mut expected = CellFrame::default();
            expected.resize(viewport.width, viewport.height).unwrap();
            surface
                .encoder()
                .encode(reference.view(), expected.view_mut())
                .unwrap();
            assert_eq!(surface.cells(), &expected, "{row_order:?}");
        }
    }
}

#[test]
fn exact_size_frames_encode_directly_in_any_layout() {
    let viewport = Viewport {
        width: 23,
        height: 11,
    };
    for encoder in [
        &HalfBlock as &dyn CellEncoder,
        &Quadrant,
        &trend::Sextant,
        &trend::Braille,
        &trend::Density::ASCII,
        &trend::Density::new(trend::DensityRamp::Shades, true),
    ] {
        let spec = RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
        let image = exact_image(spec.width(), spec.height());
        for format in FORMATS {
            for row_order in ROW_ORDERS {
                for alpha in ALPHA_MODES {
                    let (bytes, layout) = encode_layout(&image, format, row_order, alpha);
                    let frame = PixelFrameView::from_layout(&bytes, layout).unwrap();
                    assert!(spec.matches(&frame));

                    // Zero-copy path: the borrowed frame goes straight to the encoder.
                    let mut direct = CellFrame::default();
                    direct.resize(viewport.width, viewport.height).unwrap();
                    encoder.encode(frame, direct.view_mut()).unwrap();

                    // Reference: the normalized straight RGBA image.
                    let reference = normalized(&frame);
                    let mut expected = CellFrame::default();
                    expected.resize(viewport.width, viewport.height).unwrap();
                    encoder
                        .encode(reference.view(), expected.view_mut())
                        .unwrap();

                    // Resampling path at 1:1 must agree with the direct path.
                    let mut resampled_pixels = PixelBuffer::new(1, 1).unwrap();
                    ImageSource::new(frame)
                        .with_fit(FitMode::Stretch)
                        .with_sampling(SamplingFilter::Nearest)
                        .render_into(&spec, &mut resampled_pixels)
                        .unwrap();
                    let mut resampled = CellFrame::default();
                    resampled.resize(viewport.width, viewport.height).unwrap();
                    encoder
                        .encode(resampled_pixels.view(), resampled.view_mut())
                        .unwrap();

                    assert_eq!(direct, expected, "{layout:?}");
                    assert_eq!(resampled, direct, "{layout:?}");

                    // A dirty region, encoded from a sub-view of the same
                    // borrowed frame, matches those cells of the whole.
                    let region = trend::CellRect::new(3, 2, 11, 6);
                    let mut partial = CellFrame::default();
                    partial.resize(viewport.width, viewport.height).unwrap();
                    encoder
                        .encode(
                            frame.sub(spec.pixel_rect(region).unwrap()).unwrap(),
                            partial.view_mut().sub(region).unwrap(),
                        )
                        .unwrap();
                    for row in region.y..region.y + region.height {
                        let columns = usize::from(region.x)..usize::from(region.x + region.width);
                        assert_eq!(
                            partial.view().row(row)[columns.clone()],
                            direct.view().row(row)[columns],
                            "{layout:?} row {row}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn junk_alpha_renders_opaque_only_when_declared_ignored() {
    // A BGRX capture whose fourth byte is always 0.
    let bytes = [0, 0, 255, 0, 0, 255, 0, 0];
    let spec = RasterSpec::new(
        Viewport {
            width: 1,
            height: 1,
        },
        HalfBlock.grid(),
        CellAspect::default(),
    )
    .unwrap();
    let encode = |alpha| {
        let layout = PixelLayout::new(1, 2, PixelFormat::Bgra8, alpha);
        let frame = PixelFrameView::from_layout(&bytes, layout).unwrap();
        assert!(spec.matches(&frame));
        let mut cells = CellFrame::default();
        cells.resize(1, 1).unwrap();
        HalfBlock.encode(frame, cells.view_mut()).unwrap();
        cells.cells()[0]
    };

    // Declared straight, the frame is honestly transparent...
    assert_eq!(encode(AlphaMode::Straight), trend::Cell::default());
    // ...declared as padding, it is the red-over-green image it really is.
    assert_eq!(
        encode(AlphaMode::Ignore),
        trend::Cell::half_block(trend::Color::rgb(255, 0, 0), trend::Color::rgb(0, 255, 0))
    );
}
