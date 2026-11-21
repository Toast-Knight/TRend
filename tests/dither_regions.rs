//! M3 regression tests: dithering regions and row bands with global
//! coordinates matches dithering the whole surface, so banded or regional
//! rendering never shows seams where bands meet.

mod common;

use trend::dither::{ordered_4x4_in_place, ordered_4x4_region};
use trend::{
    CellAspect, CellEncoder, CellFrame, FitMode, HalfBlock, ImageSource, PixelBuffer, PixelRect,
    Quadrant, RasterSpec, RenderOptions, RenderRequest, Renderer, SamplingFilter, Viewport,
};

const VIEWPORT: Viewport = Viewport {
    width: 19,
    height: 7,
};

fn whole_dithered(source: &ImageSource<'_>, spec: &RasterSpec, strength: u8) -> PixelBuffer {
    let mut buffer = PixelBuffer::new(1, 1).unwrap();
    source.render_into(spec, &mut buffer).unwrap();
    ordered_4x4_in_place(&mut buffer, strength);
    buffer
}

#[test]
fn banded_dithering_matches_whole_surface_dithering() {
    let spec = RasterSpec::new(VIEWPORT, Quadrant.grid(), CellAspect::default()).unwrap();
    for pattern in common::patterns() {
        for matte in common::MATTES {
            let source = ImageSource::new(pattern.view())
                .with_sampling(SamplingFilter::Bilinear)
                .with_alpha_background(matte);
            for strength in [1, 16, 64] {
                let expected = whole_dithered(&source, &spec, strength);
                // Band heights that do and don't align with the 4×4 pattern.
                for rows_per_band in [1, 3, 4, 5, 9] {
                    let mut banded = PixelBuffer::new(spec.width(), spec.height()).unwrap();
                    for (first_row, mut band) in banded.view_mut().into_row_bands(rows_per_band) {
                        source
                            .render_region(&spec, 0, first_row, band.reborrow())
                            .unwrap();
                        ordered_4x4_region(&mut band, 0, first_row, strength);
                    }
                    assert_eq!(
                        banded, expected,
                        "{} strength {strength} bands of {rows_per_band}",
                        pattern.name
                    );
                }
            }
        }
    }
}

#[test]
fn regional_dithering_uses_the_regions_global_origin() {
    let spec = RasterSpec::new(VIEWPORT, HalfBlock.grid(), CellAspect::default()).unwrap();
    let patterns = common::patterns();
    let source = ImageSource::new(patterns[0].view()).with_fit(FitMode::Stretch);
    let expected = whole_dithered(&source, &spec, 24);

    for rect in [
        PixelRect::new(1, 1, 5, 3),
        PixelRect::new(7, 2, 11, 9),
        PixelRect::new(spec.width() - 3, spec.height() - 2, 3, 2),
    ] {
        let mut region = PixelBuffer::new(rect.width, rect.height).unwrap();
        let mut view = region.view_mut();
        source
            .render_region(&spec, rect.x, rect.y, view.reborrow())
            .unwrap();
        ordered_4x4_region(&mut view, rect.x, rect.y, 24);
        let reference = expected.view().sub(rect).unwrap();
        for y in 0..rect.height {
            assert_eq!(region.view().row(y), reference.row(y), "{rect:?}");
        }

        // Dithering the region at origin (0, 0) would use the wrong pattern
        // phase whenever the origin isn't a multiple of 4.
        if rect.x % 4 != 0 || rect.y % 4 != 0 {
            let mut wrong = PixelBuffer::new(rect.width, rect.height).unwrap();
            source
                .render_region(&spec, rect.x, rect.y, wrong.view_mut())
                .unwrap();
            ordered_4x4_region(&mut wrong.view_mut(), 0, 0, 24);
            assert_ne!(wrong, region, "{rect:?}");
        }
    }
}

#[test]
fn dithering_leaves_transparent_pixels_and_alpha_untouched() {
    let mut buffer = PixelBuffer::new(3, 1).unwrap();
    buffer
        .pixels_mut()
        .copy_from_slice(&[100, 100, 100, 0, 100, 100, 100, 7, 100, 100, 100, 255]);
    let mut view = buffer.view_mut();
    ordered_4x4_region(&mut view, 1, 0, 0);
    assert_eq!(buffer.view().rgba(1, 0), Some([100, 100, 100, 7]));

    ordered_4x4_region(&mut buffer.view_mut(), 1, 0, 40);
    let view = buffer.view();
    assert_eq!(view.rgba(0, 0), Some([100, 100, 100, 0]));
    assert_eq!(view.rgba(1, 0).unwrap()[3], 7);
    assert_eq!(view.rgba(2, 0).unwrap()[3], 255);
    assert_ne!(view.rgba(1, 0), Some([100, 100, 100, 7]));
}

/// The banded pipeline (render and dither band by band, then encode) matches
/// the legacy renderer cell for cell, including dither.
#[test]
fn banded_pipeline_matches_the_legacy_renderer_with_dither() {
    let spec = RasterSpec::new(VIEWPORT, HalfBlock.grid(), CellAspect::default()).unwrap();
    for pattern in common::patterns() {
        for filter in common::FILTERS {
            let options = RenderOptions {
                sampling: filter,
                dither_strength: 16,
                ..RenderOptions::default()
            };
            let mut legacy = CellFrame::default();
            Renderer::new()
                .render_with_options_into(
                    pattern.view(),
                    RenderRequest {
                        viewport: VIEWPORT,
                        fit: FitMode::Contain,
                        generation: 0,
                    },
                    options,
                    &mut legacy,
                )
                .unwrap();

            let source = ImageSource::new(pattern.view()).with_sampling(filter);
            let mut pixels = PixelBuffer::new(spec.width(), spec.height()).unwrap();
            for (first_row, mut band) in pixels.view_mut().into_row_bands(3) {
                source
                    .render_region(&spec, 0, first_row, band.reborrow())
                    .unwrap();
                ordered_4x4_region(&mut band, 0, first_row, options.dither_strength);
            }
            let mut cells = CellFrame::default();
            cells.resize(VIEWPORT.width, VIEWPORT.height).unwrap();
            HalfBlock.encode(pixels.view(), cells.view_mut()).unwrap();

            assert_eq!(cells, legacy, "{} {filter:?}", pattern.name);
        }
    }
}
