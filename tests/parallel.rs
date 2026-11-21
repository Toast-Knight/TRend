//! `Surface::encode_image_parallel` and `encode_from_parallel` produce exactly
//! the cells and bytes of the single-threaded methods, for every encoder,
//! filter, fit, size, and thread count.

use trend::{
    AlphaMode, Braille, CellEncoder, Color, Density, DensityRamp, FitMode, HalfBlock, ImageSource,
    PixelFormat, PixelFrameView, PixelLayout, Quadrant, RenderTransform, RowOrder, SamplingFilter,
    Sextant, Surface, TerminalCapabilities, TerminalColorMode, Viewport,
};

const CAPABILITIES: TerminalCapabilities = TerminalCapabilities {
    color: TerminalColorMode::TrueColor,
    unicode: true,
};
const SIZES: [(u16, u16); 5] = [(1, 1), (7, 3), (40, 13), (83, 29), (31, 64)];
const THREADS: [usize; 7] = [0, 1, 2, 3, 4, 7, 100];

/// A BGRA, bottom-up, premultiplied image with translucent and transparent
/// pixels, so every reader and the partial-visibility paths are exercised.
fn image(width: u32, height: u32) -> (Vec<u8>, PixelLayout) {
    let mut state = 0x1234_5678_u32 ^ width.wrapping_mul(31) ^ height;
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let alpha = match state % 7 {
                0 => 0,
                1 => 90,
                _ => 255,
            } as u16;
            let scale = |c: u32| ((c as u16 * alpha + 127) / 255) as u8;
            bytes.extend_from_slice(&[
                scale((x * 7 + y * 3) % 256),
                scale((x * 2 + y * 11 + state % 32) % 256),
                scale((x * 13) % 256),
                alpha as u8,
            ]);
        }
    }
    let layout = PixelLayout::new(width, height, PixelFormat::Bgra8, AlphaMode::Premultiplied)
        .with_row_order(RowOrder::BottomUp);
    (bytes, layout)
}

fn check_encoder<E: CellEncoder + Sync>(make: impl Fn() -> E, name: &str) {
    let (bytes, layout) = image(157, 101);
    let source = PixelFrameView::from_layout(&bytes, layout).unwrap();
    for (width, height) in SIZES {
        let viewport = Viewport { width, height };
        for (fit, sampling, transform, matte) in [
            (
                FitMode::Contain,
                SamplingFilter::Area,
                RenderTransform::default(),
                None,
            ),
            (
                FitMode::Cover,
                SamplingFilter::Bilinear,
                RenderTransform::default(),
                None,
            ),
            (
                FitMode::Stretch,
                SamplingFilter::Nearest,
                RenderTransform::default(),
                Some(Color::rgb(3, 40, 90)),
            ),
            (
                FitMode::Contain,
                SamplingFilter::Bilinear,
                RenderTransform {
                    zoom_percent: 160,
                    pan_x_cells: 2,
                    pan_y_cells: -1,
                    ..RenderTransform::default()
                },
                None,
            ),
        ] {
            let image = ImageSource::new(source)
                .with_fit(fit)
                .with_sampling(sampling)
                .with_transform(transform)
                .with_alpha_background(matte);
            let mut expected = Surface::new(make(), viewport, CAPABILITIES).unwrap();
            expected.encode_image(&image).unwrap();
            let expected_bytes = expected.present_bytes().to_vec();
            for threads in THREADS {
                let mut parallel = Surface::new(make(), viewport, CAPABILITIES).unwrap();
                parallel.encode_image_parallel(&image, threads).unwrap();
                let context =
                    format!("{name} {width}x{height} {fit:?} {sampling:?} threads {threads}");
                assert_eq!(parallel.cells(), expected.cells(), "cells: {context}");
                assert_eq!(
                    parallel.pixels().view().pixels(),
                    expected.pixels().view().pixels(),
                    "pixels: {context}"
                );
                assert_eq!(
                    parallel.source_mapping(),
                    expected.source_mapping(),
                    "mapping: {context}"
                );
                assert_eq!(parallel.present_bytes(), expected_bytes, "bytes: {context}");
            }
        }

        // Exact-size frames, read in place.
        let spec = *Surface::new(make(), viewport, CAPABILITIES).unwrap().spec();
        let (bytes, layout) = image(spec.width(), spec.height());
        let frame = PixelFrameView::from_layout(&bytes, layout).unwrap();
        let mut expected = Surface::new(make(), viewport, CAPABILITIES).unwrap();
        expected.encode_from(frame).unwrap();
        for threads in THREADS {
            let mut parallel = Surface::new(make(), viewport, CAPABILITIES).unwrap();
            parallel.encode_from_parallel(frame, threads).unwrap();
            assert_eq!(
                parallel.cells(),
                expected.cells(),
                "{name} {width}x{height} exact, threads {threads}"
            );
        }
    }
}

#[test]
fn every_encoder_matches_single_threaded_encoding() {
    check_encoder(|| HalfBlock, "HalfBlock");
    check_encoder(|| Quadrant, "Quadrant");
    check_encoder(|| Sextant, "Sextant");
    check_encoder(|| Braille, "Braille");
    check_encoder(|| Density::new(DensityRamp::Shades, true), "Density");
}

#[test]
fn boxed_encoders_work_and_errors_match() {
    let viewport = Viewport {
        width: 20,
        height: 9,
    };
    let encoder: Box<dyn CellEncoder + Sync> = Box::new(Sextant);
    let mut surface = Surface::new(encoder, viewport, CAPABILITIES).unwrap();
    let (bytes, layout) = image(10, 10);
    let wrong_size = PixelFrameView::from_layout(&bytes, layout).unwrap();
    let parallel = surface.encode_from_parallel(wrong_size, 4).unwrap_err();
    let single = surface.encode_from(wrong_size).unwrap_err();
    assert_eq!(format!("{parallel:?}"), format!("{single:?}"));
    surface
        .encode_image_parallel(&ImageSource::new(wrong_size), 4)
        .unwrap();
}
