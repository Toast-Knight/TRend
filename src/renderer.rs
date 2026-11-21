use core::fmt;

use crate::cell::{Cell, CellFrame, CellViewMut, Color};
use crate::dither::ordered_4x4;
use crate::encode::{EncodeError, half_block_cell};
use crate::framebuffer::PixelFrameView;
use crate::geometry::{FitMode, GeometryError, RasterSpec, RegionError, RenderTransform, Viewport};
use crate::raster::ImageSource;
use crate::scale::SamplingFilter;

/// Describes the output geometry and identity of one render operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderRequest {
    /// Output dimensions in terminal cells.
    pub viewport: Viewport,
    /// Source-to-viewport fitting policy.
    pub fit: FitMode,
    /// Caller-defined identifier copied to the resulting [`CellFrame`].
    pub generation: u64,
}

/// Optional quality, transform, dithering, and alpha-compositing controls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderOptions {
    pub sampling: SamplingFilter,
    pub dither_strength: u8,
    pub transform: RenderTransform,
    /// Composites RGBA input over this color. When absent, zero-alpha samples
    /// use the terminal default and nonzero samples preserve straight RGB.
    pub alpha_background: Option<Color>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RenderError {
    Geometry(GeometryError),
    CellBuffer(&'static str),
    /// A requested region lies outside the render surface.
    Region(RegionError),
    /// An encoder rejected its input.
    Encode(EncodeError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderStatus {
    Complete,
    Cancelled,
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Geometry(error) => error.fmt(formatter),
            Self::CellBuffer(message) => formatter.write_str(message),
            Self::Region(error) => error.fmt(formatter),
            Self::Encode(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<GeometryError> for RenderError {
    fn from(value: GeometryError) -> Self {
        Self::Geometry(value)
    }
}

impl From<EncodeError> for RenderError {
    fn from(value: EncodeError) -> Self {
        Self::Encode(value)
    }
}

impl From<RegionError> for RenderError {
    fn from(value: RegionError) -> Self {
        Self::Region(value)
    }
}

/// Stateless pixel-to-cell renderer.
///
/// A renderer can be reused across frames. Render into a reusable [`CellFrame`]
/// for the simplest allocation-conscious path, or into a caller-owned cell
/// slice when integrating with another buffer abstraction.
#[derive(Debug, Default)]
pub struct Renderer;

impl Renderer {
    pub const fn new() -> Self {
        Self
    }

    pub fn render_into(
        &self,
        source: PixelFrameView<'_>,
        request: RenderRequest,
        output: &mut CellFrame,
    ) -> Result<(), RenderError> {
        self.render_with_options_into(source, request, RenderOptions::default(), output)
    }

    pub fn render_with_options_into(
        &self,
        source: PixelFrameView<'_>,
        request: RenderRequest,
        options: RenderOptions,
        output: &mut CellFrame,
    ) -> Result<(), RenderError> {
        output
            .resize(request.viewport.width, request.viewport.height)
            .map_err(RenderError::CellBuffer)?;
        self.render_to_slice_with_options(source, request, options, output.cells_mut())?;
        output.set_generation(request.generation);
        Ok(())
    }

    pub fn render_to_slice(
        &self,
        source: PixelFrameView<'_>,
        request: RenderRequest,
        output: &mut [Cell],
    ) -> Result<(), RenderError> {
        self.render_to_slice_with_options(source, request, RenderOptions::default(), output)
    }

    pub fn render_to_slice_with_options(
        &self,
        source: PixelFrameView<'_>,
        request: RenderRequest,
        options: RenderOptions,
        output: &mut [Cell],
    ) -> Result<(), RenderError> {
        let status =
            self.render_to_slice_cancellable(source, request, options, output, || false)?;
        debug_assert_eq!(status, RenderStatus::Complete);
        Ok(())
    }

    pub fn render_to_slice_cancellable<F>(
        &self,
        source: PixelFrameView<'_>,
        request: RenderRequest,
        options: RenderOptions,
        output: &mut [Cell],
        mut should_cancel: F,
    ) -> Result<RenderStatus, RenderError>
    where
        F: FnMut() -> bool,
    {
        // The same stages as ImageSource -> ordered dither -> HalfBlock, streamed
        // one cell row at a time so no pixel buffer is allocated.
        let image = ImageSource::new(source)
            .with_fit(request.fit)
            .with_sampling(options.sampling)
            .with_transform(options.transform)
            .with_alpha_background(options.alpha_background);
        let spec = RasterSpec::half_block(request.viewport)?;
        let geometry = image.geometry(&spec)?;
        let required = usize::from(request.viewport.width)
            .checked_mul(usize::from(request.viewport.height))
            .ok_or(RenderError::CellBuffer(
                "cell dimensions overflow addressable memory",
            ))?;
        if output.len() < required {
            return Err(RenderError::CellBuffer("output cell buffer is too small"));
        }
        if should_cancel() {
            return Ok(RenderStatus::Cancelled);
        }
        output[..required].fill(Cell::default());
        let mut cells = CellViewMut::from_slice(
            &mut output[..required],
            request.viewport.width,
            request.viewport.height,
        )
        .expect("output length was checked");

        let pixel = |x: u32, y: u32| {
            let color = image.color_at(&geometry, x, y)?;
            Some(if options.dither_strength == 0 {
                color
            } else {
                ordered_4x4(color, x, y, options.dither_strength)
            })
        };
        for cell_y in 0..request.viewport.height {
            if should_cancel() {
                return Ok(RenderStatus::Cancelled);
            }
            let y = u32::from(cell_y) * 2;
            for (cell_x, cell) in cells.row_mut(cell_y).iter_mut().enumerate() {
                let x = cell_x as u32;
                *cell = half_block_cell(pixel(x, y), pixel(x, y + 1));
            }
        }

        Ok(RenderStatus::Complete)
    }
}

#[cfg(test)]
mod option_tests {
    use core::time::Duration;

    use super::*;
    use crate::framebuffer::PixelFormat;

    #[test]
    fn render_options_integrate_bilinear_sampling_and_dither() {
        let pixels = [
            64, 64, 64, 255, 192, 64, 64, 255, 64, 192, 64, 255, 192, 192, 192, 255,
        ];
        let source =
            PixelFrameView::new(&pixels, 2, 2, 8, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let request = RenderRequest {
            viewport: Viewport {
                width: 3,
                height: 2,
            },
            fit: FitMode::Stretch,
            generation: 1,
        };
        let mut plain = CellFrame::default();
        let mut processed = CellFrame::default();
        let renderer = Renderer::new();
        renderer.render_into(source, request, &mut plain).unwrap();
        renderer
            .render_with_options_into(
                source,
                request,
                RenderOptions {
                    sampling: SamplingFilter::Bilinear,
                    dither_strength: 16,
                    ..RenderOptions::default()
                },
                &mut processed,
            )
            .unwrap();

        assert_ne!(plain.cells(), processed.cells());
        assert_eq!(processed.generation(), 1);
    }

    #[test]
    fn stale_render_cancels_between_rows() {
        let pixels = [255_u8; 8 * 8 * 4];
        let source =
            PixelFrameView::new(&pixels, 8, 8, 32, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let request = RenderRequest {
            viewport: Viewport {
                width: 8,
                height: 4,
            },
            fit: FitMode::Stretch,
            generation: 1,
        };
        let mut output = [Cell::default(); 32];
        let mut checks = 0;

        let status = Renderer::new()
            .render_to_slice_cancellable(
                source,
                request,
                RenderOptions::default(),
                &mut output,
                || {
                    checks += 1;
                    checks > 3
                },
            )
            .unwrap();

        assert_eq!(status, RenderStatus::Cancelled);
        assert!(output[..16].iter().all(|cell| *cell != Cell::default()));
        assert!(output[16..].iter().all(|cell| *cell == Cell::default()));
    }

    #[test]
    fn immediate_cancellation_does_not_touch_staging_buffer() {
        let pixels = [255_u8; 8];
        let source =
            PixelFrameView::new(&pixels, 1, 2, 4, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let sentinel = Cell::half_block(Color::rgb(1, 2, 3), Color::rgb(4, 5, 6));
        let mut output = [sentinel; 1];

        let status = Renderer::new()
            .render_to_slice_cancellable(
                source,
                RenderRequest {
                    viewport: Viewport {
                        width: 1,
                        height: 1,
                    },
                    fit: FitMode::Contain,
                    generation: 1,
                },
                RenderOptions::default(),
                &mut output,
                || true,
            )
            .unwrap();

        assert_eq!(status, RenderStatus::Cancelled);
        assert_eq!(output, [sentinel]);
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::*;
    use crate::framebuffer::PixelFormat;

    #[test]
    fn renders_two_rgba_rows_as_one_half_block_cell() {
        let pixels = [255, 0, 0, 255, 0, 0, 255, 255];
        let source =
            PixelFrameView::new(&pixels, 1, 2, 4, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let mut output = CellFrame::default();

        Renderer::new()
            .render_into(
                source,
                RenderRequest {
                    viewport: Viewport {
                        width: 1,
                        height: 1,
                    },
                    fit: FitMode::Contain,
                    generation: 7,
                },
                &mut output,
            )
            .unwrap();

        assert_eq!(output.generation(), 7);
        assert_eq!(output.cells()[0].glyph, '▀' as u32);
        assert_eq!(output.cells()[0].foreground, Color::rgb(255, 0, 0));
        assert_eq!(output.cells()[0].background, Color::rgb(0, 0, 255));
    }

    #[test]
    fn contain_letterboxes_without_changing_output_dimensions() {
        let pixels = [255_u8; 4 * 4];
        let source =
            PixelFrameView::new(&pixels, 1, 4, 4, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let mut output = CellFrame::default();

        Renderer::new()
            .render_into(
                source,
                RenderRequest {
                    viewport: Viewport {
                        width: 4,
                        height: 2,
                    },
                    fit: FitMode::Contain,
                    generation: 1,
                },
                &mut output,
            )
            .unwrap();

        assert_eq!((output.width(), output.height()), (4, 2));
        assert_eq!(output.cells()[0], Cell::default());
        assert_ne!(output.cells()[1], Cell::default());
    }

    #[test]
    fn transparent_and_opaque_black_produce_distinct_cells() {
        let transparent = [0, 0, 0, 0, 255, 0, 0, 0];
        let opaque = [0, 0, 0, 255, 0, 0, 0, 255];
        let request = RenderRequest {
            viewport: Viewport {
                width: 1,
                height: 1,
            },
            fit: FitMode::Stretch,
            generation: 1,
        };
        let mut transparent_cells = CellFrame::default();
        let mut opaque_cells = CellFrame::default();
        let renderer = Renderer::new();
        renderer
            .render_into(
                PixelFrameView::new(&transparent, 1, 2, 4, PixelFormat::Rgba8, Duration::ZERO)
                    .unwrap(),
                request,
                &mut transparent_cells,
            )
            .unwrap();
        renderer
            .render_into(
                PixelFrameView::new(&opaque, 1, 2, 4, PixelFormat::Rgba8, Duration::ZERO).unwrap(),
                request,
                &mut opaque_cells,
            )
            .unwrap();

        assert_eq!(transparent_cells.cells()[0], Cell::default());
        assert_eq!(opaque_cells.cells()[0].foreground, Color::rgb(0, 0, 0));
        assert_eq!(opaque_cells.cells()[0].background, Color::rgb(0, 0, 0));
        assert_ne!(transparent_cells, opaque_cells);
    }

    #[test]
    fn one_transparent_half_uses_terminal_default_for_that_half() {
        let pixels = [0, 0, 0, 0, 3, 2, 1, 255];
        let source =
            PixelFrameView::new(&pixels, 1, 2, 4, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let mut output = CellFrame::default();
        Renderer::new()
            .render_into(
                source,
                RenderRequest {
                    viewport: Viewport {
                        width: 1,
                        height: 1,
                    },
                    fit: FitMode::Stretch,
                    generation: 1,
                },
                &mut output,
            )
            .unwrap();

        assert_eq!(output.cells()[0].glyph, '▄' as u32);
        assert_eq!(output.cells()[0].foreground, Color::rgb(3, 2, 1));
        assert_eq!(output.cells()[0].background, Color::DEFAULT);
    }
}
