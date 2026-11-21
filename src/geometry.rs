use core::fmt;

use crate::framebuffer::PixelFrameView;

/// Controls how source pixels are mapped into the terminal viewport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FitMode {
    /// Preserve aspect ratio and show the entire source, leaving unused cells.
    Contain,
    /// Preserve aspect ratio and fill the viewport, cropping overflow.
    Cover,
    /// Fill the viewport without preserving aspect ratio.
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Alignment {
    Start,
    #[default]
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderTransform {
    pub zoom_percent: u16,
    pub pan_x_cells: i32,
    pub pan_y_cells: i32,
    pub horizontal_alignment: Alignment,
    pub vertical_alignment: Alignment,
}

impl Default for RenderTransform {
    fn default() -> Self {
        Self {
            zoom_percent: 100,
            pan_x_cells: 0,
            pan_y_cells: 0,
            horizontal_alignment: Alignment::Center,
            vertical_alignment: Alignment::Center,
        }
    }
}

/// Terminal output dimensions measured in character cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderGeometry {
    pub viewport: Viewport,
    pub sample_width: u32,
    pub sample_height: u32,
    pub offset_x: i64,
    pub offset_y: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GeometryError {
    ZeroDimension,
    ZeroZoom,
    SizeOverflow,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ZeroDimension => "source and viewport dimensions must be non-zero",
            Self::ZeroZoom => "zoom percentage must be non-zero",
            Self::SizeOverflow => "geometry calculation overflowed",
        })
    }
}

impl std::error::Error for GeometryError {}

impl RenderGeometry {
    pub fn calculate(
        source_width: u32,
        source_height: u32,
        viewport: Viewport,
        fit: FitMode,
    ) -> Result<Self, GeometryError> {
        Self::calculate_transformed(
            source_width,
            source_height,
            viewport,
            fit,
            RenderTransform::default(),
        )
    }

    /// Fits a source into a half-block viewport (two square pixels per cell).
    ///
    /// Equivalent to [`Self::calculate_for_spec`] with a half-block
    /// [`RasterSpec`] on 1:2 cells, which is how it's implemented.
    pub fn calculate_transformed(
        source_width: u32,
        source_height: u32,
        viewport: Viewport,
        fit: FitMode,
        transform: RenderTransform,
    ) -> Result<Self, GeometryError> {
        if source_width == 0 || source_height == 0 {
            return Err(GeometryError::ZeroDimension);
        }
        let spec = RasterSpec::half_block(viewport)?;
        Self::calculate_for_spec(source_width, source_height, &spec, fit, transform)
    }
}

/// Pixels resolved per terminal cell by a [`crate::CellEncoder`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubcellGrid {
    pub columns: u8,
    pub rows: u8,
}

impl SubcellGrid {
    /// One column, two rows: `▀`/`▄` half-blocks.
    pub const HALF_BLOCK: Self = Self {
        columns: 1,
        rows: 2,
    };
    /// Two columns, two rows: quadrant block elements.
    pub const QUADRANT: Self = Self {
        columns: 2,
        rows: 2,
    };
    /// Two columns, three rows: sextant mosaics.
    pub const SEXTANT: Self = Self {
        columns: 2,
        rows: 3,
    };
    /// Two columns, four rows: Braille dots.
    pub const BRAILLE: Self = Self {
        columns: 2,
        rows: 4,
    };
}

/// Physical width:height of one terminal cell as drawn by the terminal font.
///
/// TRend never detects this; the host supplies it. The default of 1:2 is the
/// common monospace cell shape and the value today's half-block output assumes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellAspect {
    pub width: u16,
    pub height: u16,
}

impl Default for CellAspect {
    fn default() -> Self {
        Self {
            width: 1,
            height: 2,
        }
    }
}

/// Physical width:height of one encoder pixel, in lowest terms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelAspect {
    pub width: u32,
    pub height: u32,
}

/// The pixel surface an encoder needs for a viewport: the handshake between
/// what the terminal output requires and what a source must render.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RasterSpec {
    viewport: Viewport,
    grid: SubcellGrid,
    width: u32,
    height: u32,
    pixel_aspect: PixelAspect,
}

impl RasterSpec {
    pub fn new(
        viewport: Viewport,
        grid: SubcellGrid,
        cell: CellAspect,
    ) -> Result<Self, GeometryError> {
        if viewport.width == 0
            || viewport.height == 0
            || grid.columns == 0
            || grid.rows == 0
            || cell.width == 0
            || cell.height == 0
        {
            return Err(GeometryError::ZeroDimension);
        }
        let spec = Self::from_parts(viewport, grid, cell);
        // A spec must describe a surface that can exist in memory.
        usize::try_from(spec.width)
            .ok()
            .and_then(|width| width.checked_mul(usize::try_from(spec.height).ok()?))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(GeometryError::SizeOverflow)?;
        Ok(spec)
    }

    /// The half-block spec today's renderer assumes: 1×2 pixels per cell on a
    /// 1:2 cell, so pixels are square. Unlike [`Self::new`] it doesn't require
    /// the surface to fit in memory, because geometry alone never allocates.
    pub(crate) fn half_block(viewport: Viewport) -> Result<Self, GeometryError> {
        if viewport.width == 0 || viewport.height == 0 {
            return Err(GeometryError::ZeroDimension);
        }
        Ok(Self::from_parts(
            viewport,
            SubcellGrid::HALF_BLOCK,
            CellAspect::default(),
        ))
    }

    /// Builds a spec from non-zero parts.
    fn from_parts(viewport: Viewport, grid: SubcellGrid, cell: CellAspect) -> Self {
        let width = u32::from(viewport.width) * u32::from(grid.columns);
        let height = u32::from(viewport.height) * u32::from(grid.rows);

        // One pixel spans cell.width / columns horizontally and
        // cell.height / rows vertically.
        let aspect_width = u32::from(cell.width) * u32::from(grid.rows);
        let aspect_height = u32::from(cell.height) * u32::from(grid.columns);
        let divisor = greatest_common_divisor(aspect_width, aspect_height);
        Self {
            viewport,
            grid,
            width,
            height,
            pixel_aspect: PixelAspect {
                width: aspect_width / divisor,
                height: aspect_height / divisor,
            },
        }
    }

    /// Output size in cells.
    pub const fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub const fn grid(&self) -> SubcellGrid {
        self.grid
    }

    /// Pixel width: `viewport.width * grid.columns`.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Pixel height: `viewport.height * grid.rows`.
    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn pixel_aspect(&self) -> PixelAspect {
        self.pixel_aspect
    }

    /// True when `frame` is exactly this spec's pixel size, so an encoder can
    /// read it directly with no resampling or copy.
    pub fn matches(&self, frame: &PixelFrameView<'_>) -> bool {
        frame.width() == self.width && frame.height() == self.height
    }

    /// The pixels that encode `cells`: each cell spans `grid.columns` by
    /// `grid.rows` pixels. Use it to pair a cell region with its pixel region.
    pub fn pixel_rect(&self, cells: CellRect) -> Result<PixelRect, RegionError> {
        cells.check_within(self.viewport.width, self.viewport.height)?;
        let columns = u32::from(self.grid.columns);
        let rows = u32::from(self.grid.rows);
        Ok(PixelRect::new(
            u32::from(cells.x) * columns,
            u32::from(cells.y) * rows,
            u32::from(cells.width) * columns,
            u32::from(cells.height) * rows,
        ))
    }

    /// The cell showing pixel `(x, y)`, as `(column, row)`, or `None`
    /// outside the surface. Use it to find the cell under a point of an
    /// application's scene.
    pub fn cell_at(&self, x: u32, y: u32) -> Option<(u16, u16)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        // Bounded by the viewport, which is u16.
        Some((
            (x / u32::from(self.grid.columns)) as u16,
            (y / u32::from(self.grid.rows)) as u16,
        ))
    }

    /// The pixels cell `(column, row)` encodes, or `None` outside the
    /// viewport. Use it to find the scene point under a cell, such as the
    /// one a mouse event reports.
    pub fn cell_pixels(&self, column: u16, row: u16) -> Option<PixelRect> {
        self.pixel_rect(CellRect::new(column, row, 1, 1)).ok()
    }

    /// The smallest cell region whose pixels cover `pixels`, rounding outward.
    /// Use it to turn a damaged pixel rectangle into the cells to re-encode.
    pub fn covering_cells(&self, pixels: PixelRect) -> Result<CellRect, RegionError> {
        pixels.check_within(self.width, self.height)?;
        let columns = u32::from(self.grid.columns);
        let rows = u32::from(self.grid.rows);
        let left = pixels.x / columns;
        let top = pixels.y / rows;
        let right = (pixels.x + pixels.width).div_ceil(columns);
        let bottom = (pixels.y + pixels.height).div_ceil(rows);
        // Bounded by the viewport, which is u16.
        Ok(CellRect::new(
            left as u16,
            top as u16,
            (right - left) as u16,
            (bottom - top) as u16,
        ))
    }
}

/// A rectangle in pixel coordinates (top-left origin, rows downward).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The overlapping area, or `None` if the rectangles don't overlap.
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self
            .x
            .saturating_add(self.width)
            .min(other.x.saturating_add(other.width));
        let bottom = self
            .y
            .saturating_add(self.height)
            .min(other.y.saturating_add(other.height));
        (left < right && top < bottom).then(|| Self::new(left, top, right - left, bottom - top))
    }

    /// Checks that the rectangle is non-empty and lies inside `width` × `height`.
    pub(crate) fn check_within(&self, width: u32, height: u32) -> Result<(), RegionError> {
        if self.width == 0 || self.height == 0 {
            return Err(RegionError::Empty);
        }
        match (
            self.x.checked_add(self.width),
            self.y.checked_add(self.height),
        ) {
            (Some(right), Some(bottom)) if right <= width && bottom <= height => Ok(()),
            _ => Err(RegionError::OutOfBounds),
        }
    }
}

/// A rectangle in terminal cell coordinates (top-left origin, rows downward).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CellRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl CellRect {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The overlapping area, or `None` if the rectangles don't overlap.
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self
            .x
            .saturating_add(self.width)
            .min(other.x.saturating_add(other.width));
        let bottom = self
            .y
            .saturating_add(self.height)
            .min(other.y.saturating_add(other.height));
        (left < right && top < bottom).then(|| Self::new(left, top, right - left, bottom - top))
    }

    /// Checks that the rectangle is non-empty and lies inside `width` × `height`.
    pub(crate) fn check_within(&self, width: u16, height: u16) -> Result<(), RegionError> {
        if self.width == 0 || self.height == 0 {
            return Err(RegionError::Empty);
        }
        match (
            self.x.checked_add(self.width),
            self.y.checked_add(self.height),
        ) {
            (Some(right), Some(bottom)) if right <= width && bottom <= height => Ok(()),
            _ => Err(RegionError::OutOfBounds),
        }
    }
}

/// Why a sub-view or region could not be created.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RegionError {
    /// The rectangle has zero width or height.
    Empty,
    /// The rectangle extends past the view it was taken from.
    OutOfBounds,
}

impl fmt::Display for RegionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "region has zero width or height",
            Self::OutOfBounds => "region extends outside the view",
        })
    }
}

impl std::error::Error for RegionError {}

impl RenderGeometry {
    /// Fits a source into a [`RasterSpec`] pixel surface.
    ///
    /// Generalizes [`Self::calculate_transformed`] to any subcell grid and
    /// pixel aspect. Containment compares physical sizes, and pan offsets are
    /// converted from cells to pixels with the spec's grid. For a half-block
    /// spec with square pixels the results are identical.
    pub fn calculate_for_spec(
        source_width: u32,
        source_height: u32,
        spec: &RasterSpec,
        fit: FitMode,
        transform: RenderTransform,
    ) -> Result<Self, GeometryError> {
        if source_width == 0 || source_height == 0 {
            return Err(GeometryError::ZeroDimension);
        }
        if transform.zoom_percent == 0 {
            return Err(GeometryError::ZeroZoom);
        }

        let available_width = u64::from(spec.width);
        let available_height = u64::from(spec.height);
        let aspect_width = u128::from(spec.pixel_aspect.width);
        let aspect_height = u128::from(spec.pixel_aspect.height);
        let source_width_wide = u128::from(source_width);
        let source_height_wide = u128::from(source_height);

        let (base_width, base_height) = match fit {
            FitMode::Stretch => (available_width, available_height),
            FitMode::Contain | FitMode::Cover => {
                // Physical target width / height versus source width / height.
                let target_by_source =
                    u128::from(available_width) * aspect_width * source_height_wide;
                let source_by_target =
                    u128::from(available_height) * aspect_height * source_width_wide;
                let width_limited = target_by_source <= source_by_target;
                let use_width = match fit {
                    FitMode::Contain => width_limited,
                    FitMode::Cover => !width_limited,
                    FitMode::Stretch => unreachable!(),
                };

                if use_width {
                    let height = target_by_source / (source_width_wide * aspect_height);
                    let height = u64::try_from(height).map_err(|_| GeometryError::SizeOverflow)?;
                    (available_width, height.max(1))
                } else {
                    let width = source_by_target / (source_height_wide * aspect_width);
                    let width = u64::try_from(width).map_err(|_| GeometryError::SizeOverflow)?;
                    (width.max(1), available_height)
                }
            }
        };

        let sample_width = base_width
            .checked_mul(u64::from(transform.zoom_percent))
            .ok_or(GeometryError::SizeOverflow)?
            .div_ceil(100);
        let sample_height = base_height
            .checked_mul(u64::from(transform.zoom_percent))
            .ok_or(GeometryError::SizeOverflow)?
            .div_ceil(100);
        let offset_x = aligned_offset(
            available_width,
            sample_width,
            transform.horizontal_alignment,
            i64::from(transform.pan_x_cells)
                .checked_mul(i64::from(spec.grid.columns))
                .ok_or(GeometryError::SizeOverflow)?,
        )?;
        let offset_y = aligned_offset(
            available_height,
            sample_height,
            transform.vertical_alignment,
            i64::from(transform.pan_y_cells)
                .checked_mul(i64::from(spec.grid.rows))
                .ok_or(GeometryError::SizeOverflow)?,
        )?;

        Ok(Self {
            viewport: spec.viewport,
            sample_width: u32::try_from(sample_width).map_err(|_| GeometryError::SizeOverflow)?,
            sample_height: u32::try_from(sample_height).map_err(|_| GeometryError::SizeOverflow)?,
            offset_x,
            offset_y,
        })
    }
}

const fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn aligned_offset(
    available: u64,
    rendered: u64,
    alignment: Alignment,
    pan: i64,
) -> Result<i64, GeometryError> {
    let available = i64::try_from(available).map_err(|_| GeometryError::SizeOverflow)?;
    let rendered = i64::try_from(rendered).map_err(|_| GeometryError::SizeOverflow)?;
    let difference = available
        .checked_sub(rendered)
        .ok_or(GeometryError::SizeOverflow)?;
    let base = match alignment {
        Alignment::Start => 0,
        Alignment::Center => difference / 2,
        Alignment::End => difference,
    };
    if rendered <= available {
        return Ok(base);
    }

    Ok(base.saturating_sub(pan).clamp(difference, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_preserves_wide_aspect_in_half_block_cells() {
        let geometry = RenderGeometry::calculate(
            1920,
            1080,
            Viewport {
                width: 80,
                height: 24,
            },
            FitMode::Contain,
        )
        .unwrap();

        assert_eq!(geometry.sample_width, 80);
        assert_eq!(geometry.sample_height, 45);
        assert_eq!(
            geometry.viewport,
            Viewport {
                width: 80,
                height: 24
            }
        );
        assert_eq!(geometry.offset_y, 1);
    }

    #[test]
    fn cover_fills_viewport_and_reports_crop_samples() {
        let geometry = RenderGeometry::calculate(
            100,
            100,
            Viewport {
                width: 80,
                height: 24,
            },
            FitMode::Cover,
        )
        .unwrap();

        assert_eq!(geometry.sample_width, 80);
        assert_eq!(geometry.sample_height, 80);
        assert_eq!(
            geometry.viewport,
            Viewport {
                width: 80,
                height: 24
            }
        );
        assert_eq!(geometry.offset_y, -16);
    }

    #[test]
    fn alignment_positions_contained_content() {
        let transform = RenderTransform {
            horizontal_alignment: Alignment::End,
            vertical_alignment: Alignment::Start,
            ..RenderTransform::default()
        };
        let geometry = RenderGeometry::calculate_transformed(
            1,
            4,
            Viewport {
                width: 8,
                height: 2,
            },
            FitMode::Contain,
            transform,
        )
        .unwrap();

        assert_eq!(geometry.sample_width, 1);
        assert_eq!(geometry.offset_x, 7);
        assert_eq!(geometry.offset_y, 0);
    }

    #[test]
    fn zoom_and_pan_are_clamped_to_image_bounds() {
        let transform = RenderTransform {
            zoom_percent: 200,
            pan_x_cells: 100,
            pan_y_cells: -100,
            ..RenderTransform::default()
        };
        let geometry = RenderGeometry::calculate_transformed(
            4,
            4,
            Viewport {
                width: 4,
                height: 2,
            },
            FitMode::Contain,
            transform,
        )
        .unwrap();

        assert_eq!((geometry.sample_width, geometry.sample_height), (8, 8));
        assert_eq!(geometry.offset_x, -4);
        assert_eq!(geometry.offset_y, 0);
    }

    #[test]
    fn raster_spec_derives_pixel_size_and_aspect_from_the_grid() {
        let viewport = Viewport {
            width: 40,
            height: 20,
        };
        let half =
            RasterSpec::new(viewport, SubcellGrid::HALF_BLOCK, CellAspect::default()).unwrap();
        let quadrant =
            RasterSpec::new(viewport, SubcellGrid::QUADRANT, CellAspect::default()).unwrap();

        assert_eq!((half.width(), half.height()), (40, 40));
        assert_eq!(
            half.pixel_aspect(),
            PixelAspect {
                width: 1,
                height: 1
            }
        );
        assert_eq!((quadrant.width(), quadrant.height()), (80, 40));
        assert_eq!(
            quadrant.pixel_aspect(),
            PixelAspect {
                width: 1,
                height: 2
            }
        );
        assert_eq!(
            RasterSpec::new(
                viewport,
                SubcellGrid {
                    columns: 0,
                    rows: 2
                },
                CellAspect::default()
            ),
            Err(GeometryError::ZeroDimension)
        );
    }

    /// The pre-M2b `calculate_transformed`, kept verbatim as an independent
    /// reference now that the public function delegates to
    /// `calculate_for_spec`.
    fn reference_calculate_transformed(
        source_width: u32,
        source_height: u32,
        viewport: Viewport,
        fit: FitMode,
        transform: RenderTransform,
    ) -> Result<RenderGeometry, GeometryError> {
        if source_width == 0 || source_height == 0 || viewport.width == 0 || viewport.height == 0 {
            return Err(GeometryError::ZeroDimension);
        }
        if transform.zoom_percent == 0 {
            return Err(GeometryError::ZeroZoom);
        }

        let available_width = u64::from(viewport.width);
        let available_height = u64::from(viewport.height)
            .checked_mul(2)
            .ok_or(GeometryError::SizeOverflow)?;

        let (base_width, base_height) = match fit {
            FitMode::Stretch => (available_width, available_height),
            FitMode::Contain | FitMode::Cover => {
                let width_limited = available_width
                    .checked_mul(u64::from(source_height))
                    .ok_or(GeometryError::SizeOverflow)?
                    <= available_height
                        .checked_mul(u64::from(source_width))
                        .ok_or(GeometryError::SizeOverflow)?;
                let use_width = match fit {
                    FitMode::Contain => width_limited,
                    FitMode::Cover => !width_limited,
                    FitMode::Stretch => unreachable!(),
                };

                if use_width {
                    let height = available_width
                        .checked_mul(u64::from(source_height))
                        .ok_or(GeometryError::SizeOverflow)?
                        / u64::from(source_width);
                    (available_width, height.max(1))
                } else {
                    let width = available_height
                        .checked_mul(u64::from(source_width))
                        .ok_or(GeometryError::SizeOverflow)?
                        / u64::from(source_height);
                    (width.max(1), available_height)
                }
            }
        };

        let sample_width = base_width
            .checked_mul(u64::from(transform.zoom_percent))
            .ok_or(GeometryError::SizeOverflow)?
            .div_ceil(100);
        let sample_height = base_height
            .checked_mul(u64::from(transform.zoom_percent))
            .ok_or(GeometryError::SizeOverflow)?
            .div_ceil(100);
        let offset_x = aligned_offset(
            available_width,
            sample_width,
            transform.horizontal_alignment,
            i64::from(transform.pan_x_cells),
        )?;
        let offset_y = aligned_offset(
            available_height,
            sample_height,
            transform.vertical_alignment,
            i64::from(transform.pan_y_cells)
                .checked_mul(2)
                .ok_or(GeometryError::SizeOverflow)?,
        )?;

        Ok(RenderGeometry {
            viewport,
            sample_width: u32::try_from(sample_width).map_err(|_| GeometryError::SizeOverflow)?,
            sample_height: u32::try_from(sample_height).map_err(|_| GeometryError::SizeOverflow)?,
            offset_x,
            offset_y,
        })
    }

    /// The spec-based calculation (and so the public `calculate_transformed`)
    /// must reproduce the original half-block calculation exactly.
    #[test]
    fn half_block_spec_agrees_with_existing_geometry() {
        let sources = [1_u32, 2, 3, 7, 37, 64, 300, 1920];
        let source_heights = [1_u32, 4, 23, 1080];
        let viewports = [(1_u16, 1_u16), (3, 2), (17, 9), (80, 24), (1, 40), (200, 3)];
        let fits = [FitMode::Contain, FitMode::Cover, FitMode::Stretch];
        let alignments = [Alignment::Start, Alignment::Center, Alignment::End];
        let mut compared = 0;

        for &source_width in &sources {
            for &source_height in &source_heights {
                for &(width, height) in &viewports {
                    let viewport = Viewport { width, height };
                    let spec =
                        RasterSpec::new(viewport, SubcellGrid::HALF_BLOCK, CellAspect::default())
                            .unwrap();
                    for fit in fits {
                        for zoom_percent in [1_u16, 50, 100, 250, 1000] {
                            for pan in [(-7, 3), (0, 0), (5, -9)] {
                                for horizontal_alignment in alignments {
                                    for vertical_alignment in alignments {
                                        let transform = RenderTransform {
                                            zoom_percent,
                                            pan_x_cells: pan.0,
                                            pan_y_cells: pan.1,
                                            horizontal_alignment,
                                            vertical_alignment,
                                        };
                                        let reference = reference_calculate_transformed(
                                            source_width,
                                            source_height,
                                            viewport,
                                            fit,
                                            transform,
                                        );
                                        assert_eq!(
                                            RenderGeometry::calculate_for_spec(
                                                source_width,
                                                source_height,
                                                &spec,
                                                fit,
                                                transform,
                                            ),
                                            reference,
                                            "{source_width}x{source_height} into {viewport:?} {fit:?} {transform:?}"
                                        );
                                        assert_eq!(
                                            RenderGeometry::calculate_transformed(
                                                source_width,
                                                source_height,
                                                viewport,
                                                fit,
                                                transform,
                                            ),
                                            reference
                                        );
                                        compared += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(compared, 8 * 4 * 6 * 3 * 5 * 3 * 3 * 3);
    }

    #[test]
    fn delegated_geometry_reports_the_same_errors() {
        let cases = [
            (0, 5, 3, 2, 100),
            (5, 0, 3, 2, 100),
            (5, 5, 0, 2, 100),
            (5, 5, 3, 0, 100),
            (0, 5, 0, 2, 0),
            (5, 5, 3, 2, 0),
            (5, 5, 0, 0, 0),
            (u32::MAX, 1, u16::MAX, u16::MAX, u16::MAX),
            (1, u32::MAX, u16::MAX, u16::MAX, u16::MAX),
        ];
        for (source_width, source_height, width, height, zoom_percent) in cases {
            let viewport = Viewport { width, height };
            let transform = RenderTransform {
                zoom_percent,
                ..RenderTransform::default()
            };
            for fit in [FitMode::Contain, FitMode::Cover, FitMode::Stretch] {
                assert_eq!(
                    RenderGeometry::calculate_transformed(
                        source_width,
                        source_height,
                        viewport,
                        fit,
                        transform
                    ),
                    reference_calculate_transformed(
                        source_width,
                        source_height,
                        viewport,
                        fit,
                        transform
                    ),
                    "{source_width}x{source_height} {viewport:?} zoom {zoom_percent} {fit:?}"
                );
            }
        }
    }

    #[test]
    fn quadrant_spec_fits_by_physical_size_not_pixel_count() {
        let spec = RasterSpec::new(
            Viewport {
                width: 40,
                height: 20,
            },
            SubcellGrid::QUADRANT,
            CellAspect::default(),
        )
        .unwrap();
        let square = RenderGeometry::calculate_for_spec(
            100,
            100,
            &spec,
            FitMode::Contain,
            RenderTransform::default(),
        )
        .unwrap();
        // 80 pixels of width 1 and 40 pixels of height 2 are physically square.
        assert_eq!((square.sample_width, square.sample_height), (80, 40));

        let wide = RenderGeometry::calculate_for_spec(
            200,
            100,
            &spec,
            FitMode::Contain,
            RenderTransform {
                pan_x_cells: 3,
                ..RenderTransform::default()
            },
        )
        .unwrap();
        assert_eq!((wide.sample_width, wide.sample_height), (80, 20));
        assert_eq!((wide.offset_x, wide.offset_y), (0, 10));
    }

    #[test]
    fn zero_zoom_is_rejected() {
        let transform = RenderTransform {
            zoom_percent: 0,
            ..RenderTransform::default()
        };
        assert_eq!(
            RenderGeometry::calculate_transformed(
                1,
                1,
                Viewport {
                    width: 1,
                    height: 1
                },
                FitMode::Contain,
                transform,
            ),
            Err(GeometryError::ZeroZoom)
        );
    }
}
