//! M4: `Surface` produces exactly what the manual pipeline produces, and the
//! mapping helpers agree with the sampler.

mod common;

use core::time::Duration;

use common::screen::Screen;
use trend::{
    AnsiPresenter, CellAspect, CellEncoder, CellFrame, CellRect, Density, DensityRamp, EncodeError,
    HalfBlock, ImageSource, OutputOptions, PixelBuffer, PixelFormat, PixelFrameView, PixelRect,
    Quadrant, RasterSpec, RenderError, SamplingFilter, Surface, TerminalCapabilities,
    TerminalColorMode, Viewport,
};

const TRUECOLOR: TerminalCapabilities = TerminalCapabilities {
    color: TerminalColorMode::TrueColor,
    unicode: true,
};
const VIEWPORT: Viewport = Viewport {
    width: 19,
    height: 7,
};

fn encoders() -> Vec<Box<dyn CellEncoder>> {
    vec![
        Box::new(HalfBlock),
        Box::new(Quadrant),
        Box::new(trend::Sextant),
        Box::new(trend::Braille),
        Box::new(Density::ASCII),
        Box::new(Density::new(DensityRamp::Shades, true)),
    ]
}

/// A deterministic scene drawn at any size: frame `t` of a moving pattern.
fn paint(pixels: &mut trend::PixelViewMut<'_>, t: u32) {
    for y in 0..pixels.height() {
        for (x, pixel) in pixels.row_mut(y).chunks_exact_mut(4).enumerate() {
            let x = x as u32;
            let alpha = if (x + y + t).is_multiple_of(11) {
                0
            } else {
                255
            };
            pixel.copy_from_slice(&[
                (x * 13 + t * 7) as u8,
                (y * 29) as u8,
                ((x ^ y) * 5 + t) as u8,
                alpha,
            ]);
        }
    }
}

fn manual_encode(
    encoder: &dyn CellEncoder,
    pixels: PixelFrameView<'_>,
    viewport: Viewport,
) -> CellFrame {
    let mut cells = CellFrame::default();
    cells.resize(viewport.width, viewport.height).unwrap();
    encoder.encode(pixels, cells.view_mut()).unwrap();
    cells
}

#[test]
fn drawing_matches_the_manual_pipeline_byte_for_byte() {
    for encoder in encoders() {
        let spec = RasterSpec::new(VIEWPORT, encoder.grid(), CellAspect::default()).unwrap();
        let mut surface = Surface::new(&*encoder, VIEWPORT, TRUECOLOR).unwrap();
        assert_eq!(surface.spec(), &spec);

        let mut pixels = PixelBuffer::new(spec.width(), spec.height()).unwrap();
        let mut presenter = AnsiPresenter::with_capabilities(TRUECOLOR);
        for t in 0..4 {
            let seen = surface
                .draw(|mut view, drawn_spec| {
                    assert_eq!(drawn_spec, &spec);
                    paint(&mut view, t);
                    t * 2
                })
                .unwrap();
            assert_eq!(seen, t * 2, "draw returns the closure's value");
            paint(&mut pixels.view_mut(), t);
            let cells = manual_encode(&*encoder, pixels.view(), VIEWPORT);
            assert_eq!(surface.cells(), &cells);
            assert_eq!(surface.pixels(), &pixels);

            let mut written = Vec::new();
            let stats = surface.present(&mut written).unwrap();
            assert_eq!(written, presenter.present_at(&cells, 1, 1));
            assert_eq!(stats, presenter.last_stats());
        }
    }
}

#[test]
fn images_match_render_into_then_encode() {
    let patterns = common::patterns();
    for encoder in encoders() {
        let spec = RasterSpec::new(VIEWPORT, encoder.grid(), CellAspect::default()).unwrap();
        let mut surface = Surface::new(&*encoder, VIEWPORT, TRUECOLOR).unwrap();
        for pattern in &patterns {
            for filter in common::FILTERS {
                for fit in common::FITS {
                    for transform in common::transforms() {
                        let source = ImageSource::new(pattern.view())
                            .with_sampling(filter)
                            .with_fit(fit)
                            .with_transform(transform);
                        surface.encode_image(&source).unwrap();
                        let mut pixels = PixelBuffer::new(1, 1).unwrap();
                        source.render_into(&spec, &mut pixels).unwrap();
                        assert_eq!(surface.pixels(), &pixels);
                        assert_eq!(
                            surface.cells(),
                            &manual_encode(&*encoder, pixels.view(), VIEWPORT),
                            "{} {filter:?} {fit:?}",
                            pattern.name
                        );
                        assert_eq!(
                            surface.source_mapping(),
                            Some(&source.mapping(&spec).unwrap())
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn exact_size_frames_are_encoded_in_place_and_others_rejected() {
    let spec = RasterSpec::new(VIEWPORT, Quadrant.grid(), CellAspect::default()).unwrap();
    // A BGRA bottom-up readback at exactly the spec's size.
    let width = spec.width() as usize;
    let stride = width * 4 + 12;
    let mut bgra = vec![0_u8; stride * spec.height() as usize];
    let mut rgba = PixelBuffer::new(spec.width(), spec.height()).unwrap();
    paint(&mut rgba.view_mut(), 3);
    for y in 0..spec.height() {
        let target = (spec.height() - 1 - y) as usize * stride;
        for (x, pixel) in rgba.view().row(y).unwrap().chunks_exact(4).enumerate() {
            bgra[target + x * 4..target + x * 4 + 4]
                .copy_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    let layout = trend::PixelLayout::new(
        spec.width(),
        spec.height(),
        PixelFormat::Bgra8,
        trend::AlphaMode::Straight,
    )
    .with_stride(stride)
    .with_row_order(trend::RowOrder::BottomUp);
    let frame = PixelFrameView::from_layout(&bgra, layout).unwrap();

    let mut surface = Surface::new(Quadrant, VIEWPORT, TRUECOLOR).unwrap();
    surface.encode_from(frame).unwrap();
    assert_eq!(
        surface.cells(),
        &manual_encode(&Quadrant, rgba.view(), VIEWPORT)
    );
    assert_eq!(surface.source_mapping(), None);
    // The frame was read in place: the surface's own buffer is untouched.
    assert!(surface.pixels().pixels().iter().all(|&byte| byte == 0));

    let small = PixelBuffer::new(spec.width() - 1, spec.height()).unwrap();
    assert_eq!(
        surface.encode_from(small.view()),
        Err(RenderError::Encode(EncodeError::SizeMismatch {
            expected: (spec.width(), spec.height()),
            actual: (spec.width() - 1, spec.height()),
        }))
    );
}

#[test]
fn damaged_regions_re_encode_to_the_same_cells_as_everything() {
    let damages = [
        PixelRect::new(0, 0, 1, 1),
        PixelRect::new(5, 3, 7, 4),
        PixelRect::new(30, 10, 100, 100), // clipped
        PixelRect::new(17, 0, 3, 28),
    ];
    for encoder in encoders() {
        let mut surface = Surface::new(&*encoder, VIEWPORT, TRUECOLOR).unwrap();
        surface.draw(|mut pixels, _| paint(&mut pixels, 0)).unwrap();
        let mut presented = Vec::new();
        surface.present(&mut presented).unwrap();

        for (t, damage) in damages.into_iter().enumerate() {
            // Change only the damaged pixels.
            let mut next =
                PixelBuffer::new(surface.spec().width(), surface.spec().height()).unwrap();
            paint(&mut next.view_mut(), t as u32 + 1);
            let Some(clipped) =
                damage.intersection(&PixelRect::new(0, 0, next.width(), next.height()))
            else {
                assert_eq!(surface.encode_region(damage).unwrap(), None);
                continue;
            };
            let mut view = surface.pixels_mut();
            for y in clipped.y..clipped.y + clipped.height {
                let range = clipped.x as usize * 4..(clipped.x + clipped.width) as usize * 4;
                view.row_mut(y)[range.clone()].copy_from_slice(&next.view().row(y).unwrap()[range]);
            }

            let cells = surface.encode_region(damage).unwrap().unwrap();
            assert_eq!(cells, surface.spec().covering_cells(clipped).unwrap());
            let full = manual_encode(&*encoder, surface.pixels().view(), VIEWPORT);
            assert_eq!(surface.cells(), &full, "{damage:?}");

            // Presenting just those cells brings the screen fully up to date.
            let mut output = Vec::new();
            surface.present_region(cells, &mut output).unwrap();
            presented.extend_from_slice(&output);
            assert!(surface.present_bytes().is_empty(), "{damage:?}");
        }
        let mut expected = Screen::new(40, 12);
        expected.apply(AnsiPresenter::with_capabilities(TRUECOLOR).present_at(
            surface.cells(),
            1,
            1,
        ));
        let mut actual = Screen::new(40, 12);
        actual.apply(&presented);
        assert_eq!(actual, expected);

        assert_eq!(
            surface.encode_region(PixelRect::new(500, 0, 5, 5)).unwrap(),
            None
        );
    }
}

/// Changing part of an image and re-rendering only `source_damage` gives
/// exactly the surface a full re-render gives, for every filter and fit.
#[test]
fn source_damage_covers_every_changed_surface_pixel() {
    let patterns = common::patterns();
    let damages = [
        PixelRect::new(0, 0, 1, 1),
        PixelRect::new(3, 2, 5, 4),
        PixelRect::new(10, 7, 40, 1),
        PixelRect::new(0, 0, 1000, 1000),
    ];
    for viewport in [
        VIEWPORT,
        Viewport {
            width: 3,
            height: 2,
        },
        Viewport {
            width: 60,
            height: 30,
        },
    ] {
        for encoder in [&HalfBlock as &dyn CellEncoder, &Density::ASCII] {
            let spec = RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
            for pattern in &patterns {
                for damage in damages {
                    let mut changed = pattern.pixels.clone();
                    for y in damage.y..(damage.y + damage.height).min(pattern.height) {
                        for x in damage.x..(damage.x + damage.width).min(pattern.width) {
                            let offset = y as usize * pattern.stride + x as usize * 4;
                            for channel in &mut changed[offset..offset + 4] {
                                *channel = !*channel;
                            }
                        }
                    }
                    let changed_view = PixelFrameView::new(
                        &changed,
                        pattern.width,
                        pattern.height,
                        pattern.stride,
                        PixelFormat::Rgba8,
                        Duration::ZERO,
                    )
                    .unwrap();
                    for filter in common::FILTERS {
                        for fit in common::FITS {
                            for transform in common::transforms() {
                                let before = ImageSource::new(pattern.view())
                                    .with_sampling(filter)
                                    .with_fit(fit)
                                    .with_transform(transform);
                                let after = before.clone_with(changed_view);
                                let mut surface =
                                    Surface::new(encoder, viewport, TRUECOLOR).unwrap();
                                surface.encode_image(&before).unwrap();
                                let mapping = before.mapping(&spec).unwrap();
                                if let Some(region) = mapping.source_damage(damage) {
                                    surface.encode_image_region(&after, region).unwrap();
                                }
                                let mut full = PixelBuffer::new(1, 1).unwrap();
                                after.render_into(&spec, &mut full).unwrap();
                                assert_eq!(
                                    surface.pixels(),
                                    &full,
                                    "{} {damage:?} {filter:?} {fit:?} {transform:?} {viewport:?}",
                                    pattern.name
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

trait CloneWith<'a> {
    fn clone_with(&self, pixels: PixelFrameView<'a>) -> ImageSource<'a>;
}

impl<'a> CloneWith<'a> for ImageSource<'a> {
    /// The same settings over different pixels of the same size.
    fn clone_with(&self, pixels: PixelFrameView<'a>) -> ImageSource<'a> {
        let debug = format!("{self:?}");
        let sampling = [
            SamplingFilter::Nearest,
            SamplingFilter::Bilinear,
            SamplingFilter::Area,
        ]
        .into_iter()
        .find(|filter| debug.contains(&format!("sampling: {filter:?}")))
        .unwrap();
        let fit = common::FITS
            .into_iter()
            .find(|fit| debug.contains(&format!("fit: {fit:?}")))
            .unwrap();
        let transform = common::transforms()
            .into_iter()
            .find(|transform| debug.contains(&format!("transform: {transform:?}")))
            .unwrap();
        ImageSource::new(pixels)
            .with_sampling(sampling)
            .with_fit(fit)
            .with_transform(transform)
    }
}

#[test]
fn nearest_mapping_names_the_pixel_each_surface_pixel_shows() {
    // Each source pixel's color encodes its own coordinates.
    let (width, height) = (53_u32, 31_u32);
    let source: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).flat_map(move |x| [x as u8, y as u8, 7, 255]))
        .collect();
    let view = PixelFrameView::new(
        &source,
        width,
        height,
        width as usize * 4,
        PixelFormat::Rgba8,
        Duration::ZERO,
    )
    .unwrap();
    for encoder in [&HalfBlock as &dyn CellEncoder, &Quadrant, &Density::ASCII] {
        for viewport in [
            VIEWPORT,
            Viewport {
                width: 80,
                height: 24,
            },
        ] {
            let spec = RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
            for fit in common::FITS {
                for transform in common::transforms() {
                    let image = ImageSource::new(view)
                        .with_fit(fit)
                        .with_transform(transform);
                    let mut pixels = PixelBuffer::new(1, 1).unwrap();
                    image.render_into(&spec, &mut pixels).unwrap();
                    let mapping = image.mapping(&spec).unwrap();
                    let mut bounds: Option<PixelRect> = None;
                    for y in 0..spec.height() {
                        for x in 0..spec.width() {
                            let [r, g, _, a] = pixels.view().rgba(x, y).unwrap();
                            let shown = (a != 0).then_some((u32::from(r), u32::from(g)));
                            assert_eq!(mapping.to_source(x, y), shown, "({x}, {y}) {fit:?}");
                            if shown.is_some() {
                                let point = PixelRect::new(x, y, 1, 1);
                                bounds = Some(match bounds {
                                    None => point,
                                    Some(b) => {
                                        let right = (b.x + b.width).max(x + 1);
                                        let bottom = (b.y + b.height).max(y + 1);
                                        let left = b.x.min(x);
                                        let top = b.y.min(y);
                                        PixelRect::new(left, top, right - left, bottom - top)
                                    }
                                });
                            }
                        }
                    }
                    assert_eq!(mapping.content(), bounds, "{fit:?} {transform:?}");
                    assert_eq!(mapping.to_source(spec.width(), 0), None);

                    // A cell maps through its center pixel.
                    let cell = spec.cell_pixels(2, 1).unwrap();
                    assert_eq!(
                        mapping.cell_to_source(2, 1),
                        mapping.to_source(cell.x + cell.width / 2, cell.y + cell.height / 2)
                    );
                    assert_eq!(mapping.spec(), &spec);
                }
            }
        }
    }
}

#[test]
fn cells_and_pixels_map_both_ways() {
    for encoder in [&HalfBlock as &dyn CellEncoder, &Quadrant, &Density::ASCII] {
        let spec = RasterSpec::new(VIEWPORT, encoder.grid(), CellAspect::default()).unwrap();
        for row in 0..VIEWPORT.height {
            for column in 0..VIEWPORT.width {
                let pixels = spec.cell_pixels(column, row).unwrap();
                assert_eq!(
                    pixels,
                    spec.pixel_rect(CellRect::new(column, row, 1, 1)).unwrap()
                );
                for y in pixels.y..pixels.y + pixels.height {
                    for x in pixels.x..pixels.x + pixels.width {
                        assert_eq!(spec.cell_at(x, y), Some((column, row)));
                    }
                }
            }
        }
        assert_eq!(spec.cell_at(spec.width(), 0), None);
        assert_eq!(spec.cell_at(0, spec.height()), None);
        assert_eq!(spec.cell_pixels(VIEWPORT.width, 0), None);
    }
}

#[test]
fn terminal_positions_map_to_cells_through_the_origin() {
    let mut surface = Surface::new(HalfBlock, VIEWPORT, TRUECOLOR).unwrap();
    assert_eq!(surface.cell_at_terminal(1, 1), Some((0, 0)));
    surface.set_origin(5, 10);
    assert_eq!(surface.origin(), (5, 10));
    assert_eq!(surface.cell_at_terminal(5, 10), Some((0, 0)));
    assert_eq!(surface.cell_at_terminal(11, 28), Some((18, 6)));
    assert_eq!(surface.cell_at_terminal(12, 28), None);
    assert_eq!(surface.cell_at_terminal(11, 29), None);
    assert_eq!(surface.cell_at_terminal(4, 10), None);
    assert_eq!(surface.cell_at_terminal(5, 9), None);
    surface.set_origin(0, 0);
    assert_eq!(surface.origin(), (1, 1));
}

#[test]
fn changing_settings_keeps_the_surface_consistent() {
    let mut surface: Surface<Box<dyn CellEncoder>> = Surface::new(
        Box::new(HalfBlock) as Box<dyn CellEncoder>,
        VIEWPORT,
        TRUECOLOR,
    )
    .unwrap();
    surface.draw(|mut pixels, _| paint(&mut pixels, 1)).unwrap();
    let mut sink = Vec::new();
    surface.present(&mut sink).unwrap();
    assert!(surface.present_bytes().is_empty());

    // Moving redraws everything at the new place.
    surface.set_origin(3, 4);
    surface.present(&mut sink).unwrap();
    assert_eq!(surface.last_stats().cells, 19 * 7);

    // A new encoder with another grid resizes the pixels, not the cells.
    surface.set_encoder(Box::new(Quadrant)).unwrap();
    assert_eq!((surface.spec().width(), surface.spec().height()), (38, 14));
    assert_eq!(
        (surface.pixels().width(), surface.pixels().height()),
        (38, 14)
    );
    surface.draw(|mut pixels, _| paint(&mut pixels, 2)).unwrap();
    assert_eq!(
        surface.cells(),
        &manual_encode(&Quadrant, surface.pixels().view(), VIEWPORT)
    );

    // Resizing.
    let larger = Viewport {
        width: 25,
        height: 9,
    };
    surface.resize(larger).unwrap();
    assert_eq!(surface.viewport(), larger);
    assert_eq!((surface.cells().width(), surface.cells().height()), (25, 9));
    surface.draw(|mut pixels, _| paint(&mut pixels, 3)).unwrap();
    surface.present(&mut sink).unwrap();
    assert_eq!(
        surface.last_stats().cells,
        25 * 9,
        "a new size redraws everything"
    );
    assert!(
        surface
            .resize(Viewport {
                width: 0,
                height: 9
            })
            .is_err()
    );
    assert_eq!(
        surface.viewport(),
        larger,
        "a failed resize changes nothing"
    );

    // Cell aspect changes how images fit, not the buffer size.
    surface
        .set_cell_aspect(CellAspect {
            width: 1,
            height: 1,
        })
        .unwrap();
    assert_eq!(
        surface.cell_aspect(),
        CellAspect {
            width: 1,
            height: 1
        }
    );
    assert_eq!(
        (surface.pixels().width(), surface.pixels().height()),
        (50, 18)
    );

    // New capabilities redraw in the new color mode.
    let ansi = TerminalCapabilities {
        color: TerminalColorMode::Ansi256,
        unicode: true,
    };
    surface.set_capabilities(ansi);
    assert_eq!(surface.capabilities(), ansi);
    let mut output = Vec::new();
    surface.present(&mut output).unwrap();
    assert_eq!(
        output,
        AnsiPresenter::with_capabilities(ansi).present_at(surface.cells(), 3, 4)
    );

    // Options survive a capability change and apply to the next frame.
    let options = OutputOptions::new().with_synchronized_updates(true);
    surface.set_output_options(options);
    surface.set_capabilities(TRUECOLOR);
    assert_eq!(surface.output_options(), options);
    let output = surface.present_bytes();
    assert!(output.starts_with(b"\x1b[?2026h") && output.ends_with(b"\x1b[?2026l"));

    // Invalidation redraws everything.
    surface.invalidate();
    surface.present(&mut sink).unwrap();
    assert_eq!(surface.last_stats().cells, 25 * 9);

    // Regions outside the surface present nothing.
    assert!(
        surface
            .present_region_bytes(CellRect::new(30, 0, 4, 4))
            .is_empty()
    );
    let stats = surface
        .present_region(CellRect::new(30, 0, 4, 4), &mut sink)
        .unwrap();
    assert_eq!(stats.bytes, 0);
}

#[test]
fn surfaces_accept_owned_borrowed_and_boxed_encoders() {
    let owned = Surface::new(Density::ASCII, VIEWPORT, TRUECOLOR).unwrap();
    let borrowed = Surface::new(&Density::ASCII, VIEWPORT, TRUECOLOR).unwrap();
    let boxed: Surface<Box<dyn CellEncoder>> = Surface::new(
        Box::new(Density::ASCII) as Box<dyn CellEncoder>,
        VIEWPORT,
        TRUECOLOR,
    )
    .unwrap();
    assert_eq!(owned.spec(), borrowed.spec());
    assert_eq!(owned.spec(), boxed.spec());
    assert_eq!(
        boxed.encoder().requirements(),
        Density::ASCII.requirements()
    );
    let borrowed: &dyn CellEncoder = &&Quadrant;
    assert_eq!(borrowed.requirements(), Quadrant.requirements());
}

#[test]
fn source_damage_stays_local() {
    // One changed pixel of a 64×64 image stretched over 120×60 pixels
    // touches a few surface pixels, not the surface.
    let patterns = common::patterns();
    let noise = patterns
        .iter()
        .find(|pattern| pattern.name == "noise")
        .unwrap();
    let viewport = Viewport {
        width: 120,
        height: 30,
    };
    let spec = RasterSpec::new(viewport, HalfBlock.grid(), CellAspect::default()).unwrap();
    let mapping = ImageSource::new(noise.view())
        .with_fit(trend::FitMode::Stretch)
        .mapping(&spec)
        .unwrap();
    let region = mapping.source_damage(PixelRect::new(20, 20, 1, 1)).unwrap();
    // About 1.9 × 0.9 surface pixels per source pixel, plus filter margins.
    assert!(region.width <= 12 && region.height <= 8, "{region:?}");
    assert_eq!(mapping.source_damage(PixelRect::new(64, 0, 5, 5)), None);
}

/// Random geometries: every surface pixel that changes when a source
/// rectangle changes lies inside `source_damage`.
#[test]
fn source_damage_holds_for_random_geometries() {
    let mut state = 0x1234_5678_9ABC_DEF1_u64;
    let mut random = |bound: u32| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % u64::from(bound)) as u32
    };
    let grids = [HalfBlock.grid(), Quadrant.grid(), Density::ASCII.grid()];
    for _ in 0..1500 {
        let (width, height) = (1 + random(40), 1 + random(40));
        let before: Vec<u8> = (0..width * height * 4).map(|_| random(256) as u8).collect();
        let viewport = Viewport {
            width: 1 + random(24) as u16,
            height: 1 + random(12) as u16,
        };
        let grid = grids[random(3) as usize];
        let spec = RasterSpec::new(viewport, grid, CellAspect::default()).unwrap();
        let transform = trend::RenderTransform {
            zoom_percent: [100, 100, 37, 250, 400][random(5) as usize],
            pan_x_cells: random(9) as i32 - 4,
            pan_y_cells: random(9) as i32 - 4,
            ..trend::RenderTransform::default()
        };
        let rect = PixelRect::new(
            random(width),
            random(height),
            1 + random(width),
            1 + random(height),
        );
        let mut after = before.clone();
        for y in rect.y..(rect.y + rect.height).min(height) {
            for x in rect.x..(rect.x + rect.width).min(width) {
                let offset = ((y * width + x) * 4) as usize;
                for channel in &mut after[offset..offset + 4] {
                    *channel = !*channel;
                }
            }
        }
        let view = |pixels| {
            PixelFrameView::new(
                pixels,
                width,
                height,
                width as usize * 4,
                PixelFormat::Rgba8,
                Duration::ZERO,
            )
            .unwrap()
        };
        let fit = common::FITS[random(3) as usize];
        let filter = common::FILTERS[random(3) as usize];
        let source = |pixels| {
            ImageSource::new(view(pixels))
                .with_fit(fit)
                .with_sampling(filter)
                .with_transform(transform)
        };
        let (mut old, mut new) = (
            PixelBuffer::new(1, 1).unwrap(),
            PixelBuffer::new(1, 1).unwrap(),
        );
        source(&before).render_into(&spec, &mut old).unwrap();
        source(&after).render_into(&spec, &mut new).unwrap();
        let damage = source(&before).mapping(&spec).unwrap().source_damage(rect);
        for y in 0..spec.height() {
            for x in 0..spec.width() {
                if old.view().rgba(x, y) != new.view().rgba(x, y) {
                    let inside = damage.is_some_and(|d| {
                        (d.x..d.x + d.width).contains(&x) && (d.y..d.y + d.height).contains(&y)
                    });
                    assert!(
                        inside,
                        "({x}, {y}) {filter:?} {fit:?} {transform:?} source {width}x{height} \
                         spec {}x{} {rect:?} -> {damage:?}",
                        spec.width(),
                        spec.height()
                    );
                }
            }
        }
    }
}

#[test]
fn dirty_rectangles_of_borrowed_frames_re_encode_in_place() {
    for encoder in encoders() {
        let spec = RasterSpec::new(VIEWPORT, encoder.grid(), CellAspect::default()).unwrap();
        let mut frame = PixelBuffer::new(spec.width(), spec.height()).unwrap();
        paint(&mut frame.view_mut(), 0);
        let mut surface = Surface::new(&*encoder, VIEWPORT, TRUECOLOR).unwrap();
        surface.encode_from(frame.view()).unwrap();

        // The "browser" repaints a dirty rectangle of its own frame.
        let dirty = PixelRect::new(5, 3, 9, 4);
        let mut next = PixelBuffer::new(spec.width(), spec.height()).unwrap();
        paint(&mut next.view_mut(), 7);
        for y in dirty.y..dirty.y + dirty.height {
            let range = dirty.x as usize * 4..(dirty.x + dirty.width) as usize * 4;
            frame.view_mut().row_mut(y)[range.clone()]
                .copy_from_slice(&next.view().row(y).unwrap()[range]);
        }
        let cells = surface
            .encode_from_region(frame.view(), dirty)
            .unwrap()
            .unwrap();
        assert_eq!(cells, spec.covering_cells(dirty).unwrap());
        assert_eq!(
            surface.cells(),
            &manual_encode(&*encoder, frame.view(), VIEWPORT)
        );
        assert_eq!(
            surface.encode_from_region(frame.view(), PixelRect::new(9_999, 0, 1, 1)),
            Ok(None)
        );
        let small = PixelBuffer::new(1, 1).unwrap();
        assert!(surface.encode_from_region(small.view(), dirty).is_err());
    }
}
