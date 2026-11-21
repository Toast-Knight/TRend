//! Anti-aliased outline filling: signed area accumulation (the method of `font-rs`). Each line
//! segment adds its exact area to the cells it crosses; a running sum along the buffer then gives
//! every pixel's coverage, with the non-zero winding rule.

/// A glyph image: coverage 0 to 255, `width × height`, and where its top-left pixel sits relative
/// to the pen (x) and the baseline (y, down).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GlyphBitmap {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) coverage: Vec<u8>,
}

pub(crate) struct Accumulator {
    width: usize,
    height: usize,
    area: Vec<f32>,
}

impl Accumulator {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            // Lines may write one cell past a row's end; the running sum carries it on.
            area: vec![0.0; width * height + width + 4],
        }
    }

    /// Adds the line from `(x0, y0)` to `(x1, y1)`, in pixels with y down.
    pub(crate) fn line(&mut self, (x0, y0): (f32, f32), (x1, y1): (f32, f32)) {
        if (y0 - y1).abs() <= f32::EPSILON {
            return;
        }
        let (direction, (x0, y0), (x1, y1)) = if y0 < y1 {
            (1.0, (x0, y0), (x1, y1))
        } else {
            (-1.0, (x1, y1), (x0, y0))
        };
        let x0 = x0.clamp(0.0, self.width as f32);
        let x1 = x1.clamp(0.0, self.width as f32);
        let slope = (x1 - x0) / (y1 - y0);
        let mut x = x0;
        if y0 < 0.0 {
            x -= y0 * slope;
        }
        let first_row = y0.max(0.0) as usize;
        let last_row = (y1.ceil().max(0.0) as usize).min(self.height);
        for row in first_row..last_row {
            let start = row * self.width;
            let dy = (row as f32 + 1.0).min(y1) - (row as f32).max(y0);
            let x_next = x + slope * dy;
            let d = dy * direction;
            let (left, right) = if x < x_next { (x, x_next) } else { (x_next, x) };
            let left_floor = left.floor();
            let left_cell = left_floor as usize;
            let right_ceil = right.ceil();
            let right_cell = right_ceil as usize;
            if right_cell <= left_cell + 1 {
                let middle = 0.5 * (x + x_next) - left_floor;
                self.area[start + left_cell] += d - d * middle;
                self.area[start + left_cell + 1] += d * middle;
            } else {
                let inverse = (right - left).recip();
                let left_fraction = left - left_floor;
                let first = 0.5 * inverse * (1.0 - left_fraction) * (1.0 - left_fraction);
                let right_fraction = right - right_ceil + 1.0;
                let last = 0.5 * inverse * right_fraction * right_fraction;
                self.area[start + left_cell] += d * first;
                if right_cell == left_cell + 2 {
                    self.area[start + left_cell + 1] += d * (1.0 - first - last);
                } else {
                    let second = inverse * (1.5 - left_fraction);
                    self.area[start + left_cell + 1] += d * (second - first);
                    for cell in left_cell + 2..right_cell - 1 {
                        self.area[start + cell] += d * inverse;
                    }
                    let before_last = second + (right_cell - left_cell - 3) as f32 * inverse;
                    self.area[start + right_cell - 1] += d * (1.0 - before_last - last);
                }
                self.area[start + right_cell] += d * last;
            }
            x = x_next;
        }
    }

    /// Adds the quadratic curve from `from` through control `control` to `to`, as short lines.
    pub(crate) fn quad(&mut self, from: (f32, f32), control: (f32, f32), to: (f32, f32)) {
        let deviation = ((from.0 - 2.0 * control.0 + to.0).powi(2)
            + (from.1 - 2.0 * control.1 + to.1).powi(2))
        .sqrt();
        let steps = ((deviation * 2.0).sqrt().ceil() as usize).clamp(1, 32);
        let mut previous = from;
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            let u = 1.0 - t;
            let point = (
                u * u * from.0 + 2.0 * u * t * control.0 + t * t * to.0,
                u * u * from.1 + 2.0 * u * t * control.1 + t * t * to.1,
            );
            self.line(previous, point);
            previous = point;
        }
    }

    /// The coverage of every pixel, 0 to 255.
    pub(crate) fn finish(self) -> Vec<u8> {
        let mut sum = 0.0_f32;
        self.area[..self.width * self.height]
            .iter()
            .map(|&area| {
                sum += area;
                (sum.abs().min(1.0) * 255.0 + 0.5) as u8
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squares_fill_exactly_and_edges_are_partial() {
        // A 2x2 square from (1, 1) to (3, 3) in a 4x4 grid.
        let mut accumulator = Accumulator::new(4, 4);
        let square = [(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0), (1.0, 1.0)];
        for pair in square.windows(2) {
            accumulator.line(pair[0], pair[1]);
        }
        let coverage = accumulator.finish();
        assert_eq!(coverage[5], 255);
        assert_eq!(coverage[10], 255);
        assert_eq!(coverage[0], 0);
        assert_eq!(coverage[3], 0);
        // Half a pixel wide: half covered.
        let mut accumulator = Accumulator::new(4, 2);
        let strip = [(1.5, 0.0), (2.0, 0.0), (2.0, 2.0), (1.5, 2.0), (1.5, 0.0)];
        for pair in strip.windows(2) {
            accumulator.line(pair[0], pair[1]);
        }
        let coverage = accumulator.finish();
        assert!((126..=129).contains(&coverage[1]), "{coverage:?}");
        // Both windings fill.
        let mut reversed = Accumulator::new(4, 4);
        for pair in square.windows(2) {
            reversed.line(pair[1], pair[0]);
        }
        assert_eq!(reversed.finish()[5], 255);
    }

    #[test]
    fn curves_and_out_of_range_points_are_safe() {
        let mut accumulator = Accumulator::new(8, 8);
        accumulator.quad((-5.0, -5.0), (40.0, 4.0), (3.0, 30.0));
        accumulator.line((100.0, 4.0), (-100.0, 6.0));
        let coverage = accumulator.finish();
        assert_eq!(coverage.len(), 64);
    }
}
