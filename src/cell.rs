use crate::geometry::{CellRect, RegionError};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub kind: u8,
}

impl Color {
    pub const DEFAULT: Self = Self {
        red: 0,
        green: 0,
        blue: 0,
        kind: 0,
    };

    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red,
            green,
            blue,
            kind: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Cell {
    pub glyph: u32,
    pub foreground: Color,
    pub background: Color,
    /// Text attributes: a combination of [`Cell::BOLD`], [`Cell::ITALIC`],
    /// and the other attribute bits. Encoders leave it 0; text overlays
    /// ([`crate::text::put_str`]) set it. The top bit is reserved.
    pub attributes: u8,
    reserved: [u8; 3],
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            glyph: ' ' as u32,
            foreground: Color::DEFAULT,
            background: Color::DEFAULT,
            attributes: 0,
            reserved: [0; 3],
        }
    }
}

impl Cell {
    /// Bits of [`Cell::attributes`], written by [`crate::AnsiPresenter`] as
    /// SGR attributes. Terminals vary in which ones they draw; bold,
    /// underline, and reverse are the most widely supported.
    pub const BOLD: u8 = 1 << 0;
    /// Faint or decreased intensity.
    pub const DIM: u8 = 1 << 1;
    pub const ITALIC: u8 = 1 << 2;
    pub const UNDERLINE: u8 = 1 << 3;
    pub const BLINK: u8 = 1 << 4;
    /// Swaps foreground and background.
    pub const REVERSE: u8 = 1 << 5;
    pub const STRIKETHROUGH: u8 = 1 << 6;

    /// A cell whose on-screen content is unknown. Its reserved bytes can't be
    /// set through the public API, so it never equals a real cell, and the
    /// presenter redraws any cell it remembers as unknown.
    pub(crate) const UNKNOWN: Self = Self {
        glyph: 0,
        foreground: Color::DEFAULT,
        background: Color::DEFAULT,
        attributes: 0,
        reserved: [0xFF; 3],
    };

    pub const fn with_glyph(glyph: char, foreground: Color, background: Color) -> Self {
        Self {
            glyph: glyph as u32,
            foreground,
            background,
            attributes: 0,
            reserved: [0; 3],
        }
    }

    pub const fn half_block(foreground: Color, background: Color) -> Self {
        Self::with_glyph('▀', foreground, background)
    }

    pub const fn upper_half(foreground: Color) -> Self {
        Self::half_block(foreground, Color::DEFAULT)
    }

    pub const fn lower_half(foreground: Color) -> Self {
        Self {
            glyph: '▄' as u32,
            foreground,
            background: Color::DEFAULT,
            attributes: 0,
            reserved: [0; 3],
        }
    }
}

/// A reusable rectangular buffer of terminal cells produced by [`crate::Renderer`].
///
/// Reuse the same value across frames to retain its allocation. `generation`
/// is copied from the corresponding [`crate::RenderRequest`] and can be used
/// by hosts to reject stale asynchronous results.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CellFrame {
    cells: Vec<Cell>,
    width: u16,
    height: u16,
    generation: u64,
}

impl CellFrame {
    pub fn resize(&mut self, width: u16, height: u16) -> Result<(), &'static str> {
        let length = usize::from(width)
            .checked_mul(usize::from(height))
            .ok_or("cell dimensions overflow addressable memory")?;
        self.cells.resize(length, Cell::default());
        self.width = width;
        self.height = height;
        Ok(())
    }

    pub fn clear(&mut self) {
        self.cells.fill(Cell::default());
    }

    pub const fn width(&self) -> u16 {
        self.width
    }

    pub const fn height(&self) -> u16 {
        self.height
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn set_generation(&mut self, generation: u64) {
        self.generation = generation;
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn cells_mut(&mut self) -> &mut [Cell] {
        &mut self.cells
    }

    pub fn capacity(&self) -> usize {
        self.cells.capacity()
    }

    pub fn allocated_bytes(&self) -> usize {
        self.cells.capacity().saturating_mul(size_of::<Cell>())
    }

    /// Borrows the frame as a read-only cell rectangle.
    pub fn view(&self) -> CellView<'_> {
        CellView {
            cells: &self.cells,
            width: self.width,
            height: self.height,
            stride: usize::from(self.width),
        }
    }

    /// Borrows the frame as a writable cell rectangle, e.g. for a
    /// [`crate::CellEncoder`]. Take regions with [`CellViewMut::sub`] and
    /// split it with [`CellViewMut::into_row_bands`].
    pub fn view_mut(&mut self) -> CellViewMut<'_> {
        CellViewMut {
            stride: usize::from(self.width),
            cells: &mut self.cells,
            width: self.width,
            height: self.height,
        }
    }
}

/// Start index and length of a `width` × `height` rectangle at `(x, y)` in
/// row-major storage with `stride` cells per row.
fn span(x: u16, y: u16, width: u16, height: u16, stride: usize) -> (usize, usize) {
    let start = usize::from(y) * stride + usize::from(x);
    let length = if height == 0 {
        0
    } else {
        (usize::from(height) - 1) * stride + usize::from(width)
    };
    (start, length)
}

/// A borrowed, read-only rectangle of terminal cells, possibly a region of a
/// larger frame.
#[derive(Clone, Copy, Debug)]
pub struct CellView<'a> {
    cells: &'a [Cell],
    width: u16,
    height: u16,
    stride: usize,
}

impl<'a> CellView<'a> {
    pub const fn width(&self) -> u16 {
        self.width
    }

    pub const fn height(&self) -> u16 {
        self.height
    }

    /// Cells of row `y`. Panics if `y >= height`.
    pub fn row(&self, y: u16) -> &'a [Cell] {
        assert!(y < self.height, "cell row out of range");
        let start = usize::from(y) * self.stride;
        &self.cells[start..start + usize::from(self.width)]
    }

    /// A view of `rect` within this one. No cells are copied.
    pub fn sub(&self, rect: CellRect) -> Result<Self, RegionError> {
        rect.check_within(self.width, self.height)?;
        let (start, length) = span(rect.x, rect.y, rect.width, rect.height, self.stride);
        Ok(Self {
            cells: &self.cells[start..start + length],
            width: rect.width,
            height: rect.height,
            stride: self.stride,
        })
    }
}

/// A borrowed, writable rectangle of terminal cells, possibly a region of a
/// larger frame. Encoders write into it; several regions of one frame can
/// hold different encoders' output.
#[derive(Debug)]
pub struct CellViewMut<'a> {
    cells: &'a mut [Cell],
    width: u16,
    height: u16,
    stride: usize,
}

impl<'a> CellViewMut<'a> {
    /// Views the first `width * height` cells of a caller-owned slice as a
    /// row-major rectangle, e.g. the output of `render_to_slice` or cells
    /// supplied through the C ABI. Longer slices are fine; shorter ones return
    /// [`RegionError::OutOfBounds`]. Zero dimensions give an empty view.
    pub fn from_slice(cells: &'a mut [Cell], width: u16, height: u16) -> Result<Self, RegionError> {
        let length = usize::from(width) * usize::from(height);
        let cells = cells.get_mut(..length).ok_or(RegionError::OutOfBounds)?;
        Ok(Self {
            cells,
            width,
            height,
            stride: usize::from(width),
        })
    }

    pub const fn width(&self) -> u16 {
        self.width
    }

    pub const fn height(&self) -> u16 {
        self.height
    }

    /// Cells of row `y`. Panics if `y >= height`.
    pub fn row_mut(&mut self, y: u16) -> &mut [Cell] {
        assert!(y < self.height, "cell row out of range");
        let start = usize::from(y) * self.stride;
        &mut self.cells[start..start + usize::from(self.width)]
    }

    /// Reads the region back.
    pub fn as_view(&self) -> CellView<'_> {
        CellView {
            cells: self.cells,
            width: self.width,
            height: self.height,
            stride: self.stride,
        }
    }

    /// A shorter-lived view of the same cells, so the view can be passed to
    /// a function that takes it by value (such as
    /// [`crate::CellEncoder::encode`]) and used again afterwards.
    pub fn reborrow(&mut self) -> CellViewMut<'_> {
        CellViewMut {
            cells: self.cells,
            width: self.width,
            height: self.height,
            stride: self.stride,
        }
    }

    /// Sets every cell of the view to `cell`, for example to clear a panel
    /// before drawing text into it.
    pub fn fill(&mut self, cell: Cell) {
        for row in 0..self.height {
            self.row_mut(row).fill(cell);
        }
    }

    /// A writable view of `rect` within this one. No cells are copied.
    pub fn sub(&mut self, rect: CellRect) -> Result<CellViewMut<'_>, RegionError> {
        rect.check_within(self.width, self.height)?;
        let (start, length) = span(rect.x, rect.y, rect.width, rect.height, self.stride);
        Ok(CellViewMut {
            cells: &mut self.cells[start..start + length],
            width: rect.width,
            height: rect.height,
            stride: self.stride,
        })
    }

    /// Splits the view into disjoint bands of `rows_per_band` rows (the last
    /// may be shorter), each yielded with its first row. Bands can be encoded
    /// independently, in sequence or on separate threads. An empty view
    /// yields nothing. Panics if `rows_per_band` is 0.
    pub fn into_row_bands(self, rows_per_band: u16) -> CellRowBands<'a> {
        assert!(rows_per_band > 0, "rows_per_band must be non-zero");
        CellRowBands {
            rest: (self.width > 0 && self.height > 0).then_some(self),
            next_row: 0,
            rows_per_band,
        }
    }
}

/// Iterator returned by [`CellViewMut::into_row_bands`].
#[derive(Debug)]
pub struct CellRowBands<'a> {
    rest: Option<CellViewMut<'a>>,
    next_row: u16,
    rows_per_band: u16,
}

impl<'a> Iterator for CellRowBands<'a> {
    /// The band's first row within the original view, and the band.
    type Item = (u16, CellViewMut<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        let view = self.rest.take()?;
        let first_row = self.next_row;
        if view.height <= self.rows_per_band {
            return Some((first_row, view));
        }
        let split = usize::from(self.rows_per_band) * view.stride;
        let (band, rest) = view.cells.split_at_mut(split);
        self.next_row += self.rows_per_band;
        self.rest = Some(CellViewMut {
            cells: rest,
            width: view.width,
            height: view.height - self.rows_per_band,
            stride: view.stride,
        });
        Some((
            first_row,
            CellViewMut {
                cells: band,
                width: view.width,
                height: self.rows_per_band,
                stride: view.stride,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_stays_within_footprint_budget() {
        assert_eq!(size_of::<Color>(), 4);
        assert_eq!(size_of::<Cell>(), 16);
    }

    #[test]
    fn resizing_reuses_capacity_when_dimensions_shrink() {
        let mut frame = CellFrame::default();
        frame.resize(80, 24).unwrap();
        let capacity = frame.cells.capacity();
        frame.resize(40, 12).unwrap();

        assert_eq!(frame.cells.len(), 480);
        assert_eq!(frame.cells.capacity(), capacity);
    }
}
