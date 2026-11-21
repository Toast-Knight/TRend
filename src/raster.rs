use crate::cell::Color;
use crate::framebuffer::{PixelFrameView, PixelViewMut};
use crate::geometry::{
    FitMode, GeometryError, PixelRect, RasterSpec, RenderGeometry, RenderTransform,
};
use crate::pixel_buffer::PixelBuffer;
use crate::renderer::RenderError;
use crate::scale::{
    SamplingFilter, area_row, bilinear_row, sample_rgba, sample_with_alpha_background,
};

/// A whole-row resampler (`bilinear_row`, `area_row`): one call per output row.
type RowFilter = fn(&PixelFrameView<'_>, &RenderGeometry, u32, u32, &mut [u8], Option<Color>);

/// Resamples an existing RGBA8 frame (a decoded image, a video frame, or any
/// application framebuffer) into a [`PixelBuffer`] sized for an encoder.
///
/// This is the fit, transform, and sampling half of today's `Renderer`,
/// writing pixels instead of cells. It honors the spec's pixel aspect, so the
/// same source keeps its shape under any [`crate::CellEncoder`].
#[derive(Clone, Copy, Debug)]
pub struct ImageSource<'a> {
    pixels: PixelFrameView<'a>,
    fit: FitMode,
    sampling: SamplingFilter,
    transform: RenderTransform,
    alpha_background: Option<Color>,
}

impl<'a> ImageSource<'a> {
    /// Contain fit, nearest sampling, identity transform, no matte.
    pub fn new(pixels: PixelFrameView<'a>) -> Self {
        Self {
            pixels,
            fit: FitMode::Contain,
            sampling: SamplingFilter::Nearest,
            transform: RenderTransform::default(),
            alpha_background: None,
        }
    }

    pub fn with_fit(mut self, fit: FitMode) -> Self {
        self.fit = fit;
        self
    }

    pub fn with_sampling(mut self, sampling: SamplingFilter) -> Self {
        self.sampling = sampling;
        self
    }

    pub fn with_transform(mut self, transform: RenderTransform) -> Self {
        self.transform = transform;
        self
    }

    /// Composites over this color; the output is then opaque wherever the
    /// source is in bounds. Without a matte, straight RGB and coverage alpha
    /// are kept.
    pub fn with_alpha_background(mut self, alpha_background: Option<Color>) -> Self {
        self.alpha_background = alpha_background;
        self
    }

    /// Resizes `target` to the spec and writes every pixel. Pixels outside the
    /// fitted image are transparent.
    pub fn render_into(
        &self,
        spec: &RasterSpec,
        target: &mut PixelBuffer,
    ) -> Result<(), RenderError> {
        let geometry = self.geometry(spec)?;
        target
            .resize(spec.width(), spec.height())
            .map_err(|_| RenderError::Geometry(GeometryError::SizeOverflow))?;
        self.fill(&geometry, 0, 0, target.view_mut());
        Ok(())
    }

    /// Renders part of the spec's pixel surface into `target`: the rectangle
    /// whose top-left is `(x, y)` and whose size is `target`'s size.
    ///
    /// The output is identical to the same pixels of [`Self::render_into`].
    /// Use it to render straight into a region of a larger buffer (a spec
    /// sized to the region, at `(0, 0)`), or one row band at a time:
    ///
    /// ```
    /// # use trend::*;
    /// # fn demo(source: ImageSource<'_>, spec: &RasterSpec, buffer: &mut PixelBuffer,
    /// #         cancelled: impl Fn() -> bool) -> Result<(), RenderError> {
    /// for (first_row, band) in buffer.view_mut().into_row_bands(16) {
    ///     if cancelled() {
    ///         break;
    ///     }
    ///     source.render_region(spec, 0, first_row, band)?;
    /// }
    /// # Ok(()) }
    /// ```
    ///
    /// Returns [`RenderError::Region`] if the rectangle extends past the
    /// spec's surface.
    pub fn render_region(
        &self,
        spec: &RasterSpec,
        x: u32,
        y: u32,
        target: PixelViewMut<'_>,
    ) -> Result<(), RenderError> {
        let geometry = self.geometry(spec)?;
        PixelRect::new(x, y, target.width(), target.height())
            .check_within(spec.width(), spec.height())?;
        self.fill(&geometry, x, y, target);
        Ok(())
    }

    /// How this source lies on `spec`'s surface: which source pixel each
    /// surface pixel shows, and which surface pixels a source change affects.
    /// Use it to map input (a click on a cell) back to the image, or damage
    /// in the image forward to the cells to re-encode.
    pub fn mapping(&self, spec: &RasterSpec) -> Result<SourceMapping, RenderError> {
        Ok(SourceMapping {
            geometry: self.geometry(spec)?,
            spec: *spec,
            source_width: self.pixels.width(),
            source_height: self.pixels.height(),
        })
    }

    /// Fits this source into `spec`.
    pub(crate) fn geometry(&self, spec: &RasterSpec) -> Result<RenderGeometry, GeometryError> {
        RenderGeometry::calculate_for_spec(
            self.pixels.width(),
            self.pixels.height(),
            spec,
            self.fit,
            self.transform,
        )
    }

    /// The color of surface pixel `(x, y)`, or `None` for no content. The
    /// same sample [`Self::render_into`] writes, without a pixel buffer; the
    /// legacy [`crate::Renderer`] streams cells from it.
    #[inline]
    pub(crate) fn color_at(&self, geometry: &RenderGeometry, x: u32, y: u32) -> Option<Color> {
        sample_with_alpha_background(
            &self.pixels,
            geometry,
            x,
            y,
            self.sampling,
            self.alpha_background,
        )
    }

    /// Writes surface pixels starting at `(x, y)` into every pixel of `target`.
    fn fill(&self, geometry: &RenderGeometry, x: u32, y: u32, mut target: PixelViewMut<'_>) {
        let row_filter = match self.sampling {
            SamplingFilter::Bilinear => Some(bilinear_row as RowFilter),
            SamplingFilter::Area => Some(area_row as RowFilter),
            _ => None,
        };
        if let Some(filter) = row_filter {
            for row in 0..target.height() {
                filter(
                    &self.pixels,
                    geometry,
                    x,
                    y + row,
                    target.row_mut(row),
                    self.alpha_background,
                );
            }
            return;
        }
        for row in 0..target.height() {
            for (column, pixel) in target.row_mut(row).chunks_exact_mut(4).enumerate() {
                pixel.copy_from_slice(&sample_rgba(
                    &self.pixels,
                    geometry,
                    x + column as u32,
                    y + row,
                    self.sampling,
                    self.alpha_background,
                ));
            }
        }
    }
}

/// Where an [`ImageSource`] lands on a [`RasterSpec`] surface, from
/// [`ImageSource::mapping`]. Coordinates are pixels, `(x, y)` from the
/// top-left of the surface or the source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceMapping {
    geometry: RenderGeometry,
    spec: RasterSpec,
    source_width: u32,
    source_height: u32,
}

impl SourceMapping {
    /// The spec this mapping was made for.
    pub const fn spec(&self) -> &RasterSpec {
        &self.spec
    }

    /// The source pixel shown at surface pixel `(x, y)`, or `None` where the
    /// surface shows no image (letterboxing, or panned away). This is the
    /// pixel the Nearest filter samples; other filters blend its neighbors.
    pub fn to_source(&self, x: u32, y: u32) -> Option<(u32, u32)> {
        let render_x = i64::from(x) - self.geometry.offset_x;
        let render_y = i64::from(y) - self.geometry.offset_y;
        if x >= self.spec.width()
            || y >= self.spec.height()
            || render_x < 0
            || render_y < 0
            || render_x >= i64::from(self.geometry.sample_width)
            || render_y >= i64::from(self.geometry.sample_height)
        {
            return None;
        }
        // Same arithmetic as the sampler, so the answer is exact.
        let source_x =
            render_x as u64 * u64::from(self.source_width) / u64::from(self.geometry.sample_width);
        let source_y = render_y as u64 * u64::from(self.source_height)
            / u64::from(self.geometry.sample_height);
        Some((source_x as u32, source_y as u32))
    }

    /// The source pixel at the center of cell `(column, row)`, or `None`
    /// where the cell shows no image. Use it to map a mouse event to the
    /// image.
    pub fn cell_to_source(&self, column: u16, row: u16) -> Option<(u32, u32)> {
        let cell = self.spec.cell_pixels(column, row)?;
        self.to_source(cell.x + cell.width / 2, cell.y + cell.height / 2)
    }

    /// The surface pixels that show the image, or `None` if none do.
    pub fn content(&self) -> Option<PixelRect> {
        self.surface_span(
            0,
            i128::from(self.geometry.sample_width),
            0,
            i128::from(self.geometry.sample_height),
        )
    }

    /// The surface pixels whose samples can read any pixel of `source`, or
    /// `None` if the change isn't visible. Conservative for every filter:
    /// re-rendering this rectangle after `source` changed produces the same
    /// surface as re-rendering everything.
    pub fn source_damage(&self, source: PixelRect) -> Option<PixelRect> {
        let bounds = PixelRect::new(0, 0, self.source_width, self.source_height);
        let source = source.intersection(&bounds)?;
        // Bilinear and Area read neighbors of the pixel Nearest maps to, so
        // widen the source by one pixel on each side, then take every
        // surface pixel whose span overlaps it (floor the start, ceil the
        // end). Randomized tests over fits, filters, transforms, and grids
        // confirm no changed surface pixel falls outside.
        let map = |start: u32, end: u32, source_size: u32, sample: u32| {
            let (start, end) = (i128::from(start) - 1, i128::from(end) + 1);
            let (size, sample) = (i128::from(source_size), i128::from(sample));
            let first = (start * sample).div_euclid(size);
            let last = (end * sample + size - 1).div_euclid(size);
            (first, last)
        };
        let (left, right) = map(
            source.x,
            source.x + source.width,
            self.source_width,
            self.geometry.sample_width,
        );
        let (top, bottom) = map(
            source.y,
            source.y + source.height,
            self.source_height,
            self.geometry.sample_height,
        );
        self.surface_span(left, right, top, bottom)
    }

    /// Image-space spans `[left, right) × [top, bottom)`, offset onto the
    /// surface and clipped to it.
    fn surface_span(&self, left: i128, right: i128, top: i128, bottom: i128) -> Option<PixelRect> {
        let clip = |start: i128, end: i128, offset: i64, size: u32| {
            let start = (start + i128::from(offset)).clamp(0, i128::from(size));
            let end = (end + i128::from(offset)).clamp(0, i128::from(size));
            (start < end).then_some((start as u32, (end - start) as u32))
        };
        let (x, width) = clip(left, right, self.geometry.offset_x, self.spec.width())?;
        let (y, height) = clip(top, bottom, self.geometry.offset_y, self.spec.height())?;
        Some(PixelRect::new(x, y, width, height))
    }
}
