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
