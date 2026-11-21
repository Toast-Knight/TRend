//! A convenience owner for one on-screen render target.

use std::io::{self, Write};

use crate::cell::{CellFrame, CellViewMut};
use crate::encode::{CellEncoder, EncodeError};
use crate::framebuffer::{PixelFrameView, PixelViewMut};
use crate::geometry::{CellAspect, CellRect, PixelRect, RasterSpec, SubcellGrid, Viewport};
use crate::pixel_buffer::PixelBuffer;
use crate::raster::{ImageSource, SourceMapping};
use crate::renderer::RenderError;
use crate::terminal::{AnsiPresenter, OutputOptions, PresentStats, TerminalCapabilities};

/// Everything one terminal render target needs, kept consistent: the
/// encoder, the [`RasterSpec`], a pixel buffer, a cell frame, and a presenter
/// that remembers what the terminal shows.
///
/// A `Surface` is the pipeline's convenience layer, nothing more. It holds
/// no scene, never composites, never reads input, starts threads only in
/// the opt-in `*_parallel` methods, and never touches the terminal except
/// through the writer passed to [`Self::present`]. Every step it performs is
/// available separately, and it produces the same cells and bytes as doing
/// them by hand.
///
/// ```
/// # use trend::*;
/// # fn demo() -> Result<(), Box<dyn std::error::Error>> {
/// let viewport = Viewport { width: 80, height: 24 };
/// let capabilities = TerminalCapabilities { color: TerminalColorMode::TrueColor, unicode: true };
/// let mut surface = Surface::new(HalfBlock, viewport, capabilities)?;
/// let mut stdout = Vec::new(); // std::io::stdout().lock() in a real program
///
/// surface.draw(|mut pixels, spec| {
///     for y in 0..spec.height() {
///         for pixel in pixels.row_mut(y).chunks_exact_mut(4) {
///             pixel.copy_from_slice(&[0, 0, (y * 255 / spec.height()) as u8, 255]);
///         }
///     }
/// })?;
/// surface.present(&mut stdout)?;
/// # Ok(()) }
/// ```
///
/// Output is positioned at [`Self::origin`] and restores default attributes
/// afterwards, so it can share the screen with the host's own text. It never
/// clears anything outside its own cells; a host that shrinks the surface
/// clears the screen itself if it wants the old area blank.
#[derive(Debug)]
pub struct Surface<E> {
    encoder: E,
    cell_aspect: CellAspect,
    spec: RasterSpec,
    pixels: PixelBuffer,
    cells: CellFrame,
    presenter: AnsiPresenter,
    origin: (u16, u16),
    mapping: Option<SourceMapping>,
}

impl<E: CellEncoder> Surface<E> {
    /// A surface of `viewport` cells at the top-left of the screen, with the
    /// default cell aspect and output options.
    pub fn new(
        encoder: E,
        viewport: Viewport,
        capabilities: TerminalCapabilities,
    ) -> Result<Self, RenderError> {
        let cell_aspect = CellAspect::default();
        let spec = RasterSpec::new(viewport, encoder.grid(), cell_aspect)?;
        let pixels = PixelBuffer::new(spec.width(), spec.height())
            .map_err(|_| RenderError::CellBuffer("pixel surface does not fit in memory"))?;
        let mut cells = CellFrame::default();
        cells
            .resize(viewport.width, viewport.height)
            .map_err(RenderError::CellBuffer)?;
        Ok(Self {
            encoder,
            cell_aspect,
            spec,
            pixels,
            cells,
            presenter: AnsiPresenter::with_capabilities(capabilities),
            origin: (1, 1),
            mapping: None,
        })
    }

    /// Changes the size in cells. The pixel buffer is cleared to
    /// transparent; the next presentation redraws every cell. Does nothing
    /// if the size is unchanged.
    pub fn resize(&mut self, viewport: Viewport) -> Result<(), RenderError> {
        self.rebuild(viewport, self.encoder.grid(), self.cell_aspect)
    }

    /// Replaces the encoder. If its grid differs, the pixel buffer is
    /// resized and cleared. Cells are re-encoded on the next encode.
    pub fn set_encoder(&mut self, encoder: E) -> Result<(), RenderError> {
        self.rebuild(self.spec.viewport(), encoder.grid(), self.cell_aspect)?;
        self.encoder = encoder;
        Ok(())
    }

    /// Changes the terminal's cell shape, which changes the pixel aspect
    /// images are fitted with. It doesn't change the pixel buffer's size.
    pub fn set_cell_aspect(&mut self, cell_aspect: CellAspect) -> Result<(), RenderError> {
        self.rebuild(self.spec.viewport(), self.encoder.grid(), cell_aspect)?;
        self.cell_aspect = cell_aspect;
        Ok(())
    }

    /// Changes the color and glyph support used when presenting. The next
    /// presentation redraws every cell.
    pub fn set_capabilities(&mut self, capabilities: TerminalCapabilities) {
        self.presenter = AnsiPresenter::with_options(capabilities, self.presenter.options());
    }

    /// Changes output options from the next presentation on.
    pub fn set_output_options(&mut self, options: OutputOptions) {
        self.presenter.set_options(options);
    }

    /// Moves the surface to a 1-based terminal position (row, column). The
    /// next presentation redraws every cell there; the old position isn't
    /// cleared.
    pub fn set_origin(&mut self, row: u16, column: u16) {
        self.origin = (row.max(1), column.max(1));
    }

    pub const fn encoder(&self) -> &E {
        &self.encoder
    }

    pub const fn spec(&self) -> &RasterSpec {
        &self.spec
    }

    pub const fn viewport(&self) -> Viewport {
        self.spec.viewport()
    }

    pub const fn cell_aspect(&self) -> CellAspect {
        self.cell_aspect
    }

    pub const fn capabilities(&self) -> TerminalCapabilities {
        self.presenter.capabilities()
    }

    pub const fn output_options(&self) -> OutputOptions {
        self.presenter.options()
    }

    /// The 1-based terminal position (row, column) of the top-left cell.
    pub const fn origin(&self) -> (u16, u16) {
        self.origin
    }

    /// The pixel buffer: what was last drawn or placed with
    /// [`Self::encode_image`]. [`Self::encode_from`] reads the caller's frame
    /// instead and leaves it as it was.
    pub const fn pixels(&self) -> &PixelBuffer {
        &self.pixels
    }

    /// The pixel buffer for drawing. Call [`Self::encode`] or
    /// [`Self::encode_region`] afterwards to update the cells.
    pub fn pixels_mut(&mut self) -> PixelViewMut<'_> {
        self.pixels.view_mut()
    }

    /// The cells the next presentation shows.
    pub const fn cells(&self) -> &CellFrame {
        &self.cells
    }

    /// The cells for drawing over the encoded output, such as text with
    /// [`crate::text::put_str`]. The next encode overwrites the cells it
    /// covers, so draw overlays after encoding and before presenting.
    pub fn cells_mut(&mut self) -> CellViewMut<'_> {
        self.cells.view_mut()
    }

    /// Where the last image placed with [`Self::encode_image`] or
    /// [`Self::encode_image_region`] lies on the surface, until the spec
    /// changes or [`Self::draw`] or [`Self::encode_from`] replaces it.
    pub const fn source_mapping(&self) -> Option<&SourceMapping> {
        self.mapping.as_ref()
    }

    /// Statistics for the most recent presentation.
    pub const fn last_stats(&self) -> PresentStats {
        self.presenter.last_stats()
    }

    /// Lets `draw` write the pixel buffer, then encodes every cell. The
    /// buffer keeps its contents between frames, so a drawing can update
    /// only what changed. Returns what `draw` returns.
    pub fn draw<R>(
        &mut self,
        draw: impl FnOnce(PixelViewMut<'_>, &RasterSpec) -> R,
    ) -> Result<R, RenderError> {
        let result = draw(self.pixels.view_mut(), &self.spec);
        self.mapping = None;
        self.encode()?;
        Ok(result)
    }

    /// Encodes the whole pixel buffer into the cells.
    pub fn encode(&mut self) -> Result<(), RenderError> {
        self.encoder
            .encode(self.pixels.view(), self.cells.view_mut())?;
        Ok(())
    }

    /// Re-encodes only the cells covering `damage` (in surface pixels,
    /// clipped to the surface) and returns them, ready for
    /// [`Self::present_region`]. Returns `None` if nothing is covered.
    pub fn encode_region(&mut self, damage: PixelRect) -> Result<Option<CellRect>, RenderError> {
        let Some((cells, pixels)) = self.damaged(damage)? else {
            return Ok(None);
        };
        self.encoder.encode(
            self.pixels.view().sub(pixels)?,
            self.cells.view_mut().sub(cells)?,
        )?;
        Ok(Some(cells))
    }

    /// Encodes a frame that is already the spec's exact pixel size, in
    /// place, with no copy: the path for applications that render at
    /// [`Self::spec`] size into their own memory, such as a GPU readback in
    /// any [`crate::PixelLayout`].
    ///
    /// Any other size is an [`EncodeError::SizeMismatch`]; use
    /// [`Self::encode_image`] to fit and resample it instead. The two are
    /// kept apart on purpose: when pixels aren't square, a frame that merely
    /// happens to have the spec's size would otherwise be stretched instead
    /// of fitted.
    pub fn encode_from(&mut self, frame: PixelFrameView<'_>) -> Result<(), RenderError> {
        if !self.spec.matches(&frame) {
            return Err(RenderError::Encode(EncodeError::SizeMismatch {
                expected: (self.spec.width(), self.spec.height()),
                actual: (frame.width(), frame.height()),
            }));
        }
        self.mapping = None;
        self.encoder.encode(frame, self.cells.view_mut())?;
        Ok(())
    }

    /// Re-encodes only the cells covering `damage` (in pixels, clipped to the
    /// surface) from an exact-size frame, in place like [`Self::encode_from`],
    /// and returns them. Use it with the dirty rectangles a browser engine,
    /// compositor, or emulator reports for its frames.
    pub fn encode_from_region(
        &mut self,
        frame: PixelFrameView<'_>,
        damage: PixelRect,
    ) -> Result<Option<CellRect>, RenderError> {
        if !self.spec.matches(&frame) {
            return Err(RenderError::Encode(EncodeError::SizeMismatch {
                expected: (self.spec.width(), self.spec.height()),
                actual: (frame.width(), frame.height()),
            }));
        }
        let Some((cells, pixels)) = self.damaged(damage)? else {
            return Ok(None);
        };
        self.mapping = None;
        self.encoder
            .encode(frame.sub(pixels)?, self.cells.view_mut().sub(cells)?)?;
        Ok(Some(cells))
    }

    /// Fits and resamples an image into the pixel buffer, then encodes every
    /// cell. The result is identical to [`ImageSource::render_into`]
    /// followed by the encoder, without reallocating the buffer.
    pub fn encode_image(&mut self, source: &ImageSource<'_>) -> Result<(), RenderError> {
        source.render_region(&self.spec, 0, 0, self.pixels.view_mut())?;
        self.mapping = Some(source.mapping(&self.spec)?);
        self.encode()
    }

    /// Re-renders and re-encodes only the cells covering `damage` (surface
    /// pixels, clipped to the surface) and returns them. Pair it with
    /// [`SourceMapping::source_damage`] to turn a changed part of the image
    /// into the cells to update.
    pub fn encode_image_region(
        &mut self,
        source: &ImageSource<'_>,
        damage: PixelRect,
    ) -> Result<Option<CellRect>, RenderError> {
        let Some((cells, pixels)) = self.damaged(damage)? else {
            return Ok(None);
        };
        source.render_region(
            &self.spec,
            pixels.x,
            pixels.y,
            self.pixels.view_mut().sub(pixels)?,
        )?;
        self.mapping = Some(source.mapping(&self.spec)?);
        self.encoder.encode(
            self.pixels.view().sub(pixels)?,
            self.cells.view_mut().sub(cells)?,
        )?;
        Ok(Some(cells))
    }

    /// Writes the cells that changed since the last presentation to
    /// `output`, positioned at [`Self::origin`]. Nothing is written if
    /// nothing changed.
    pub fn present<W: Write + ?Sized>(&mut self, output: &mut W) -> io::Result<PresentStats> {
        output.write_all(self.present_bytes())?;
        Ok(self.last_stats())
    }

    /// Like [`Self::present`], but only for the cells in `region` (clipped
    /// to the surface). Cells outside it keep their pending changes for a
    /// later presentation.
    pub fn present_region<W: Write + ?Sized>(
        &mut self,
        region: CellRect,
        output: &mut W,
    ) -> io::Result<PresentStats> {
        if self.clip(region).is_none() {
            return Ok(PresentStats::default());
        }
        output.write_all(self.present_region_bytes(region))?;
        Ok(self.last_stats())
    }

    /// The bytes [`Self::present`] writes, for hosts with their own output.
    pub fn present_bytes(&mut self) -> &[u8] {
        self.presenter
            .present_view_at(self.cells.view(), self.origin.0, self.origin.1)
    }

    /// The bytes [`Self::present_region`] writes.
    pub fn present_region_bytes(&mut self, region: CellRect) -> &[u8] {
        match self.clip(region) {
            // A clipped region is non-empty and inside the frame.
            Some(region) => self
                .presenter
                .present_region_at(self.cells.view(), region, self.origin.0, self.origin.1)
                .unwrap_or(&[]),
            None => &[],
        }
    }

    /// Forgets what the terminal shows, so the next presentation redraws
    /// every cell. Use it after other output overwrote the surface's area.
    pub fn invalidate(&mut self) {
        self.presenter.invalidate();
    }

    /// The cell `(column, row)` at a 1-based terminal position, such as one
    /// a mouse event reports, or `None` outside the surface.
    pub fn cell_at_terminal(&self, row: u16, column: u16) -> Option<(u16, u16)> {
        let row = row.checked_sub(self.origin.0)?;
        let column = column.checked_sub(self.origin.1)?;
        let viewport = self.spec.viewport();
        (column < viewport.width && row < viewport.height).then_some((column, row))
    }

    fn clip(&self, region: CellRect) -> Option<CellRect> {
        let viewport = self.spec.viewport();
        region.intersection(&CellRect::new(0, 0, viewport.width, viewport.height))
    }

    /// Clips damage to the surface and pairs it with its covering cells.
    fn damaged(&self, damage: PixelRect) -> Result<Option<(CellRect, PixelRect)>, RenderError> {
        let surface = PixelRect::new(0, 0, self.spec.width(), self.spec.height());
        let Some(damage) = damage.intersection(&surface) else {
            return Ok(None);
        };
        let cells = self.spec.covering_cells(damage)?;
        Ok(Some((cells, self.spec.pixel_rect(cells)?)))
    }

    fn rebuild(
        &mut self,
        viewport: Viewport,
        grid: SubcellGrid,
        cell_aspect: CellAspect,
    ) -> Result<(), RenderError> {
        let spec = RasterSpec::new(viewport, grid, cell_aspect)?;
        if spec == self.spec {
            return Ok(());
        }
        if (spec.width(), spec.height()) != (self.spec.width(), self.spec.height()) {
            self.pixels
                .resize(spec.width(), spec.height())
                .map_err(|_| RenderError::CellBuffer("pixel surface does not fit in memory"))?;
        }
        self.cells
            .resize(viewport.width, viewport.height)
            .map_err(RenderError::CellBuffer)?;
        self.spec = spec;
        self.mapping = None;
        Ok(())
    }
}

/// Multi-threaded encoding, for large terminals. The surface is split into
/// horizontal bands of whole cell rows, and each band is resampled and
/// encoded on its own scoped thread. Cells are identical to the
/// single-threaded methods', since every encoder and filter works per cell
/// and per row (checked by `tests/parallel.rs`).
///
/// These spawn up to `threads - 1` threads per call (the calling thread
/// takes a band too), so unlike the rest of `Surface` they allocate: thread
/// stacks and handles. That costs tens of microseconds, worth it when a
/// frame takes milliseconds. Pass `std::thread::available_parallelism()`,
/// or fewer to leave cores for the application. `threads <= 1` runs the
/// single-threaded method.
impl<E: CellEncoder + Sync> Surface<E> {
    /// [`Self::encode_image`], split across up to `threads` threads.
    pub fn encode_image_parallel(
        &mut self,
        source: &ImageSource<'_>,
        threads: usize,
    ) -> Result<(), RenderError> {
        let bands = self.bands(threads);
        if bands.0 <= 1 {
            return self.encode_image(source);
        }
        let mapping = source.mapping(&self.spec)?;
        let (spec, encoder) = (&self.spec, &self.encoder);
        let pixel_bands = self.pixels.view_mut().into_row_bands(bands.2);
        let cell_bands = self.cells.view_mut().into_row_bands(bands.1);
        run_bands(
            pixel_bands.zip(cell_bands),
            |((y, mut pixels), (_, cells))| {
                source.render_region(spec, 0, y, pixels.reborrow())?;
                encoder.encode(pixels.as_view(), cells)?;
                Ok(())
            },
        )?;
        self.mapping = Some(mapping);
        Ok(())
    }

    /// [`Self::encode_from`], split across up to `threads` threads. The
    /// frame is still read in place.
    pub fn encode_from_parallel(
        &mut self,
        frame: PixelFrameView<'_>,
        threads: usize,
    ) -> Result<(), RenderError> {
        let bands = self.bands(threads);
        if bands.0 <= 1 || !self.spec.matches(&frame) {
            return self.encode_from(frame); // (also reports a size mismatch)
        }
        self.mapping = None;
        let (encoder, width) = (&self.encoder, frame.width());
        let subcell_rows = u32::from(self.spec.grid().rows);
        let cell_bands = self.cells.view_mut().into_row_bands(bands.1);
        run_bands(cell_bands, |(row, cells)| {
            let band = PixelRect::new(
                0,
                u32::from(row) * subcell_rows,
                width,
                u32::from(cells.height()) * subcell_rows,
            );
            encoder.encode(frame.sub(band)?, cells)?;
            Ok(())
        })
    }

    /// (bands, cell rows per band, pixel rows per band) for `threads`.
    fn bands(&self, threads: usize) -> (usize, u16, u32) {
        let rows = self.spec.viewport().height.max(1);
        let per_band = rows.div_ceil(threads.clamp(1, usize::from(rows)) as u16);
        let bands = usize::from(rows.div_ceil(per_band));
        (
            bands,
            per_band,
            u32::from(per_band) * u32::from(self.spec.grid().rows),
        )
    }
}

/// Runs `work` on every band: all but the last on scoped threads, the last
/// on the calling thread. Returns the first error, after every band ends.
fn run_bands<B: Send>(
    bands: impl Iterator<Item = B>,
    work: impl Fn(B) -> Result<(), RenderError> + Sync,
) -> Result<(), RenderError> {
    std::thread::scope(|scope| {
        let mut bands = bands.peekable();
        let mut handles = Vec::new();
        let mut last = Ok(());
        while let Some(band) = bands.next() {
            if bands.peek().is_some() {
                let work = &work;
                handles.push(scope.spawn(move || work(band)));
            } else {
                last = work(band);
            }
        }
        let mut result = Ok(());
        for handle in handles {
            let outcome = handle
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            if result.is_ok() {
                result = outcome;
            }
        }
        result.and(last)
    })
}
