//! Experimental rendering paths that do not affect the production renderer.

use crate::cell::{Cell, CellFrame, Color};
use crate::framebuffer::PixelFrameView;
use crate::geometry::{GeometryError, RenderGeometry, RenderTransform};
use crate::renderer::{RenderError, RenderRequest, RenderStatus};
use crate::scale::{SamplingFilter, sample};

const WORK_WIDTH: u32 = 8;
const WORK_HEIGHT: u32 = 8;
const WORK_PIXELS: usize = (WORK_WIDTH * WORK_HEIGHT) as usize;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExperimentalGlyphSet {
    #[default]
    Small,
    Expanded,
    Reliable,
    UniqueExpanded,
    Pruned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExperimentalGlyphOptions {
    pub sampling: SamplingFilter,
    pub glyph_set: ExperimentalGlyphSet,
    pub transform: RenderTransform,
}

impl Default for ExperimentalGlyphOptions {
    fn default() -> Self {
        Self {
            sampling: SamplingFilter::Area,
            glyph_set: ExperimentalGlyphSet::Small,
            transform: RenderTransform::default(),
        }
    }
}

#[derive(Clone, Copy)]
struct GlyphMask {
    glyph: char,
    coverage: u64,
}

const EMPTY: u64 = 0;
const FULL: u64 = u64::MAX;
const UPPER: u64 = 0x0000_0000_FFFF_FFFF;
const LOWER: u64 = 0xFFFF_FFFF_0000_0000;
const LEFT: u64 = 0x0F0F_0F0F_0F0F_0F0F;
const RIGHT: u64 = 0xF0F0_F0F0_F0F0_F0F0;
const HORIZONTAL: u64 = 0x0000_00FF_FF00_0000;
const VERTICAL: u64 = 0x1818_1818_1818_1818;
const TOP_LEFT: u64 = 0x0000_0000_0F0F_0F0F;
const TOP_RIGHT: u64 = 0x0000_0000_F0F0_F0F0;
const BOTTOM_LEFT: u64 = 0x0F0F_0F0F_0000_0000;
const BOTTOM_RIGHT: u64 = 0xF0F0_F0F0_0000_0000;

const SMALL_MASKS: &[GlyphMask] = &[
    GlyphMask {
        glyph: ' ',
        coverage: EMPTY,
    },
    GlyphMask {
        glyph: '▀',
        coverage: UPPER,
    },
    GlyphMask {
        glyph: '▄',
        coverage: LOWER,
    },
    GlyphMask {
        glyph: '█',
        coverage: FULL,
    },
    GlyphMask {
        glyph: '▌',
        coverage: LEFT,
    },
    GlyphMask {
        glyph: '▐',
        coverage: RIGHT,
    },
    GlyphMask {
        glyph: '─',
        coverage: HORIZONTAL,
    },
    GlyphMask {
        glyph: '│',
        coverage: VERTICAL,
    },
];

const EXPANDED_MASKS: &[GlyphMask] = &[
    SMALL_MASKS[0],
    SMALL_MASKS[1],
    SMALL_MASKS[2],
    SMALL_MASKS[3],
    SMALL_MASKS[4],
    SMALL_MASKS[5],
    SMALL_MASKS[6],
    SMALL_MASKS[7],
    GlyphMask {
        glyph: '▘',
        coverage: TOP_LEFT,
    },
    GlyphMask {
        glyph: '▝',
        coverage: TOP_RIGHT,
    },
    GlyphMask {
        glyph: '▖',
        coverage: BOTTOM_LEFT,
    },
    GlyphMask {
        glyph: '▗',
        coverage: BOTTOM_RIGHT,
    },
    GlyphMask {
        glyph: '▛',
        coverage: FULL ^ BOTTOM_RIGHT,
    },
    GlyphMask {
        glyph: '▜',
        coverage: FULL ^ BOTTOM_LEFT,
    },
    GlyphMask {
        glyph: '▙',
        coverage: FULL ^ TOP_RIGHT,
    },
    GlyphMask {
        glyph: '▟',
        coverage: FULL ^ TOP_LEFT,
    },
    GlyphMask {
        glyph: '▚',
        coverage: TOP_LEFT | BOTTOM_RIGHT,
    },
    GlyphMask {
        glyph: '▞',
        coverage: TOP_RIGHT | BOTTOM_LEFT,
    },
    GlyphMask {
        glyph: '┌',
        coverage: 0x0101_0101_0101_01FF,
    },
    GlyphMask {
        glyph: '┐',
        coverage: 0x8080_8080_8080_80FF,
    },
    GlyphMask {
        glyph: '└',
        coverage: 0xFF01_0101_0101_0101,
    },
    GlyphMask {
        glyph: '┘',
        coverage: 0xFF80_8080_8080_8080,
    },
];

const RELIABLE_MASKS: &[GlyphMask] = &[
    SMALL_MASKS[0],
    SMALL_MASKS[1],
    SMALL_MASKS[4],
    EXPANDED_MASKS[8],
    EXPANDED_MASKS[9],
    EXPANDED_MASKS[10],
    EXPANDED_MASKS[11],
];

const UNIQUE_EXPANDED_MASKS: &[GlyphMask] = &[
    RELIABLE_MASKS[0],
    RELIABLE_MASKS[1],
    RELIABLE_MASKS[2],
    RELIABLE_MASKS[3],
    RELIABLE_MASKS[4],
    RELIABLE_MASKS[5],
    RELIABLE_MASKS[6],
    SMALL_MASKS[6],
    SMALL_MASKS[7],
    EXPANDED_MASKS[16],
    EXPANDED_MASKS[18],
    EXPANDED_MASKS[19],
    EXPANDED_MASKS[20],
    EXPANDED_MASKS[21],
];

const PRUNED_CANDIDATES: usize = 6;

#[derive(Clone, Copy, Default)]
struct CellStatistics {
    valid_mask: u64,
    light_mask: u64,
    count: u32,
    sums: [u32; 3],
    squared_sums: [u64; 3],
    luma_sum: u32,
}

struct CellScratch {
    work: [Option<Color>; WORK_PIXELS],
    statistics: CellStatistics,
    candidates: [u8; UNIQUE_EXPANDED_MASKS.len()],
}

impl CellScratch {
    const fn new() -> Self {
        Self {
            work: [None; WORK_PIXELS],
            statistics: CellStatistics {
                valid_mask: 0,
                light_mask: 0,
                count: 0,
                sums: [0; 3],
                squared_sums: [0; 3],
                luma_sum: 0,
            },
            candidates: [0; UNIQUE_EXPANDED_MASKS.len()],
        }
    }
}

#[derive(Debug, Default)]
pub struct ExperimentalGlyphRenderer;

impl ExperimentalGlyphRenderer {
    pub const PHASE_6A_SCRATCH_BYTES_PER_CELL: usize = size_of::<[Option<Color>; WORK_PIXELS]>();
    pub const SCRATCH_BYTES_PER_CELL: usize = size_of::<CellScratch>();

    pub const fn scratch_bytes_for(glyph_set: ExperimentalGlyphSet) -> usize {
        match glyph_set {
            ExperimentalGlyphSet::Small | ExperimentalGlyphSet::Expanded => {
                Self::PHASE_6A_SCRATCH_BYTES_PER_CELL
            }
            ExperimentalGlyphSet::Reliable
            | ExperimentalGlyphSet::UniqueExpanded
            | ExperimentalGlyphSet::Pruned => Self::SCRATCH_BYTES_PER_CELL,
        }
    }

    pub const fn new() -> Self {
        Self
    }

    pub fn render_into(
        &self,
        source: PixelFrameView<'_>,
        request: RenderRequest,
        options: ExperimentalGlyphOptions,
        output: &mut CellFrame,
    ) -> Result<(), RenderError> {
        output
            .resize(request.viewport.width, request.viewport.height)
            .map_err(RenderError::CellBuffer)?;
        self.render_to_slice(source, request, options, output.cells_mut())?;
        output.set_generation(request.generation);
        Ok(())
    }

    pub fn render_to_slice(
        &self,
        source: PixelFrameView<'_>,
        request: RenderRequest,
        options: ExperimentalGlyphOptions,
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
        options: ExperimentalGlyphOptions,
        output: &mut [Cell],
        mut should_cancel: F,
    ) -> Result<RenderStatus, RenderError>
    where
        F: FnMut() -> bool,
    {
        let geometry = RenderGeometry::calculate_transformed(
            source.width(),
            source.height(),
            request.viewport,
            request.fit,
            options.transform,
        )?;
        let work_geometry = work_geometry(geometry)?;
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

        let masks = masks(options.glyph_set);
        for cell_y in 0..request.viewport.height {
            if should_cancel() {
                return Ok(RenderStatus::Cancelled);
            }
            for cell_x in 0..request.viewport.width {
                let index =
                    usize::from(cell_y) * usize::from(request.viewport.width) + usize::from(cell_x);
                output[index] = match options.glyph_set {
                    ExperimentalGlyphSet::Small | ExperimentalGlyphSet::Expanded => {
                        let mut work = [None; WORK_PIXELS];
                        fill_work(
                            &mut work,
                            &source,
                            &work_geometry,
                            cell_x,
                            cell_y,
                            options.sampling,
                        );
                        select_cell(&work, masks)
                    }
                    ExperimentalGlyphSet::Reliable
                    | ExperimentalGlyphSet::UniqueExpanded
                    | ExperimentalGlyphSet::Pruned => {
                        let mut scratch = CellScratch::new();
                        fill_work(
                            &mut scratch.work,
                            &source,
                            &work_geometry,
                            cell_x,
                            cell_y,
                            options.sampling,
                        );
                        select_cell_for_set(&mut scratch, options.glyph_set, masks)
                    }
                };
            }
        }

        Ok(RenderStatus::Complete)
    }
}

fn fill_work(
    work: &mut [Option<Color>; WORK_PIXELS],
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    cell_x: u16,
    cell_y: u16,
    sampling: SamplingFilter,
) {
    for work_y in 0..WORK_HEIGHT {
        for work_x in 0..WORK_WIDTH {
            let index = (work_y * WORK_WIDTH + work_x) as usize;
            work[index] = sample(
                source,
                geometry,
                u32::from(cell_x) * WORK_WIDTH + work_x,
                u32::from(cell_y) * WORK_HEIGHT + work_y,
                sampling,
            );
        }
    }
}

pub const fn glyph_coverage(glyph: char) -> Option<u64> {
    let mut index = 0;
    while index < EXPANDED_MASKS.len() {
        if EXPANDED_MASKS[index].glyph == glyph {
            return Some(EXPANDED_MASKS[index].coverage);
        }
        index += 1;
    }
    None
}

fn work_geometry(geometry: RenderGeometry) -> Result<RenderGeometry, GeometryError> {
    Ok(RenderGeometry {
        viewport: geometry.viewport,
        sample_width: geometry
            .sample_width
            .checked_mul(WORK_WIDTH)
            .ok_or(GeometryError::SizeOverflow)?,
        sample_height: geometry
            .sample_height
            .checked_mul(WORK_HEIGHT / 2)
            .ok_or(GeometryError::SizeOverflow)?,
        offset_x: geometry
            .offset_x
            .checked_mul(i64::from(WORK_WIDTH))
            .ok_or(GeometryError::SizeOverflow)?,
        offset_y: geometry
            .offset_y
            .checked_mul(i64::from(WORK_HEIGHT / 2))
            .ok_or(GeometryError::SizeOverflow)?,
    })
}

fn masks(set: ExperimentalGlyphSet) -> &'static [GlyphMask] {
    match set {
        ExperimentalGlyphSet::Small => SMALL_MASKS,
        ExperimentalGlyphSet::Expanded => EXPANDED_MASKS,
        ExperimentalGlyphSet::Reliable => RELIABLE_MASKS,
        ExperimentalGlyphSet::UniqueExpanded | ExperimentalGlyphSet::Pruned => {
            UNIQUE_EXPANDED_MASKS
        }
    }
}

fn select_cell_for_set(
    scratch: &mut CellScratch,
    set: ExperimentalGlyphSet,
    masks: &[GlyphMask],
) -> Cell {
    if scratch.work.iter().all(Option::is_none) {
        return Cell::default();
    }
    match set {
        ExperimentalGlyphSet::Small | ExperimentalGlyphSet::Expanded => {
            select_cell(&scratch.work, masks)
        }
        ExperimentalGlyphSet::Reliable | ExperimentalGlyphSet::UniqueExpanded => {
            scratch.statistics = build_statistics(&scratch.work, false);
            select_optimized(&scratch.work, &scratch.statistics, masks)
        }
        ExperimentalGlyphSet::Pruned => {
            scratch.statistics = build_statistics(&scratch.work, true);
            let count = prune_candidates(
                &scratch.statistics,
                masks,
                &mut scratch.candidates[..masks.len()],
            );
            select_optimized_indices(
                &scratch.work,
                &scratch.statistics,
                masks,
                &scratch.candidates[..count],
            )
        }
    }
}

fn select_cell(work: &[Option<Color>; WORK_PIXELS], masks: &[GlyphMask]) -> Cell {
    if work.iter().all(Option::is_none) {
        return Cell::default();
    }
    let mut best = fit_mask(work, masks[0]);
    for &mask in &masks[1..] {
        let candidate = fit_mask(work, mask);
        if candidate.error < best.error {
            best = candidate;
        }
    }
    best.cell
}

#[derive(Clone, Copy)]
struct FittedMask {
    cell: Cell,
    error: u64,
}

fn fit_mask(work: &[Option<Color>; WORK_PIXELS], mask: GlyphMask) -> FittedMask {
    let mut sums = [[0_u32; 3]; 2];
    let mut counts = [0_u32; 2];
    for (index, sample) in work.iter().enumerate() {
        if let Some(color) = sample {
            let pen = usize::from(mask.coverage & (1_u64 << index) != 0);
            sums[pen][0] += u32::from(color.red);
            sums[pen][1] += u32::from(color.green);
            sums[pen][2] += u32::from(color.blue);
            counts[pen] += 1;
        }
    }

    let mut colors = [Color::DEFAULT; 2];
    for pen in 0..2 {
        if counts[pen] != 0 {
            colors[pen] = Color::rgb(
                rounded_average(sums[pen][0], counts[pen]),
                rounded_average(sums[pen][1], counts[pen]),
                rounded_average(sums[pen][2], counts[pen]),
            );
        }
    }
    if counts[0] == 0 {
        colors[0] = colors[1];
    }
    if counts[1] == 0 {
        colors[1] = colors[0];
    }

    let mut error = 0_u64;
    for (index, sample) in work.iter().enumerate() {
        if let Some(actual) = sample {
            let pen = usize::from(mask.coverage & (1_u64 << index) != 0);
            let predicted = colors[pen];
            let red = i32::from(actual.red) - i32::from(predicted.red);
            let green = i32::from(actual.green) - i32::from(predicted.green);
            let blue = i32::from(actual.blue) - i32::from(predicted.blue);
            error += (red * red + green * green + blue * blue) as u64;
        }
    }

    FittedMask {
        cell: Cell::with_glyph(mask.glyph, colors[1], colors[0]),
        error,
    }
}

fn build_statistics(
    work: &[Option<Color>; WORK_PIXELS],
    include_light_mask: bool,
) -> CellStatistics {
    let mut statistics = CellStatistics::default();
    for (index, sample) in work.iter().enumerate() {
        if let Some(color) = sample {
            statistics.valid_mask |= 1_u64 << index;
            statistics.count += 1;
            for (channel, value) in [color.red, color.green, color.blue].into_iter().enumerate() {
                let value = u32::from(value);
                statistics.sums[channel] += value;
                statistics.squared_sums[channel] += u64::from(value * value);
            }
            statistics.luma_sum += luma(*color);
        }
    }
    if include_light_mask && statistics.count != 0 {
        let mean_luma = statistics.luma_sum / statistics.count;
        for (index, sample) in work.iter().enumerate() {
            if sample.is_some_and(|color| luma(color) >= mean_luma) {
                statistics.light_mask |= 1_u64 << index;
            }
        }
    }
    statistics
}

fn luma(color: Color) -> u32 {
    (77 * u32::from(color.red) + 150 * u32::from(color.green) + 29 * u32::from(color.blue)) >> 8
}

fn select_optimized(
    work: &[Option<Color>; WORK_PIXELS],
    statistics: &CellStatistics,
    masks: &[GlyphMask],
) -> Cell {
    let mut best = fit_mask_from_statistics(work, statistics, masks[0]);
    for &mask in &masks[1..] {
        let candidate = fit_mask_from_statistics(work, statistics, mask);
        if candidate.error < best.error {
            best = candidate;
        }
    }
    best.cell
}

fn select_optimized_indices(
    work: &[Option<Color>; WORK_PIXELS],
    statistics: &CellStatistics,
    masks: &[GlyphMask],
    indices: &[u8],
) -> Cell {
    let mut best = fit_mask_from_statistics(work, statistics, masks[usize::from(indices[0])]);
    for &index in &indices[1..] {
        let candidate = fit_mask_from_statistics(work, statistics, masks[usize::from(index)]);
        if candidate.error < best.error {
            best = candidate;
        }
    }
    best.cell
}

fn fit_mask_from_statistics(
    work: &[Option<Color>; WORK_PIXELS],
    statistics: &CellStatistics,
    mask: GlyphMask,
) -> FittedMask {
    let covered_mask = mask.coverage & statistics.valid_mask;
    let covered_count = covered_mask.count_ones();
    let mut covered_sums = [0_u32; 3];
    let mut remaining = covered_mask;
    while remaining != 0 {
        let index = remaining.trailing_zeros() as usize;
        let color = work[index].expect("covered mask contains only valid samples");
        covered_sums[0] += u32::from(color.red);
        covered_sums[1] += u32::from(color.green);
        covered_sums[2] += u32::from(color.blue);
        remaining &= remaining - 1;
    }

    let background_count = statistics.count - covered_count;
    let background_sums = [
        statistics.sums[0] - covered_sums[0],
        statistics.sums[1] - covered_sums[1],
        statistics.sums[2] - covered_sums[2],
    ];
    let mut foreground = mean_color(covered_sums, covered_count);
    let mut background = mean_color(background_sums, background_count);
    if covered_count == 0 {
        foreground = background;
    }
    if background_count == 0 {
        background = foreground;
    }

    FittedMask {
        cell: Cell::with_glyph(mask.glyph, foreground, background),
        error: fitted_error(
            statistics,
            covered_sums,
            covered_count,
            foreground,
            background,
        ),
    }
}

fn mean_color(sums: [u32; 3], count: u32) -> Color {
    if count == 0 {
        return Color::DEFAULT;
    }
    Color::rgb(
        rounded_average(sums[0], count),
        rounded_average(sums[1], count),
        rounded_average(sums[2], count),
    )
}

fn fitted_error(
    statistics: &CellStatistics,
    covered_sums: [u32; 3],
    covered_count: u32,
    foreground: Color,
    background: Color,
) -> u64 {
    let foreground_values = [foreground.red, foreground.green, foreground.blue];
    let background_values = [background.red, background.green, background.blue];
    let background_count = statistics.count - covered_count;
    let mut error = 0_i64;
    for channel in 0..3 {
        let foreground_value = i64::from(foreground_values[channel]);
        let background_value = i64::from(background_values[channel]);
        let foreground_sum = i64::from(covered_sums[channel]);
        let background_sum = i64::from(statistics.sums[channel] - covered_sums[channel]);
        error += statistics.squared_sums[channel] as i64
            - 2 * foreground_value * foreground_sum
            - 2 * background_value * background_sum
            + i64::from(covered_count) * foreground_value * foreground_value
            + i64::from(background_count) * background_value * background_value;
    }
    error as u64
}

fn prune_candidates(statistics: &CellStatistics, masks: &[GlyphMask], output: &mut [u8]) -> usize {
    let count = PRUNED_CANDIDATES.min(masks.len());
    let mut selected = 0_u64;
    for slot in output.iter_mut().take(count) {
        let mut best_index = 0;
        let mut best_distance = u32::MAX;
        for (index, mask) in masks.iter().enumerate() {
            if selected & (1_u64 << index) != 0 {
                continue;
            }
            let difference =
                ((mask.coverage ^ statistics.light_mask) & statistics.valid_mask).count_ones();
            let distance = difference.min(statistics.count - difference);
            if distance < best_distance {
                best_index = index;
                best_distance = distance;
            }
        }
        *slot = best_index as u8;
        selected |= 1_u64 << best_index;
    }
    count
}

fn rounded_average(sum: u32, count: u32) -> u8 {
    ((sum + count / 2) / count) as u8
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::*;
    use crate::framebuffer::PixelFormat;
    use crate::geometry::{FitMode, Viewport};

    fn solid_work(top: Color, bottom: Color) -> [Option<Color>; WORK_PIXELS] {
        let mut work = [None; WORK_PIXELS];
        for (index, pixel) in work.iter_mut().enumerate() {
            *pixel = Some(if index < 32 { top } else { bottom });
        }
        work
    }

    fn mask_work(mask: u64, foreground: Color, background: Color) -> [Option<Color>; WORK_PIXELS] {
        let mut work = [None; WORK_PIXELS];
        for (index, pixel) in work.iter_mut().enumerate() {
            *pixel = Some(if mask & (1_u64 << index) != 0 {
                foreground
            } else {
                background
            });
        }
        work
    }

    #[test]
    fn empty_mask_uses_background_mean_without_dividing_by_zero() {
        let color = Color::rgb(10, 20, 30);
        let fitted = fit_mask(
            &solid_work(color, color),
            GlyphMask {
                glyph: ' ',
                coverage: EMPTY,
            },
        );

        assert_eq!(fitted.error, 0);
        assert_eq!(fitted.cell.foreground, color);
        assert_eq!(fitted.cell.background, color);
    }

    #[test]
    fn full_mask_uses_foreground_mean_without_dividing_by_zero() {
        let color = Color::rgb(30, 20, 10);
        let fitted = fit_mask(
            &solid_work(color, color),
            GlyphMask {
                glyph: '█',
                coverage: FULL,
            },
        );

        assert_eq!(fitted.error, 0);
        assert_eq!(fitted.cell.foreground, color);
        assert_eq!(fitted.cell.background, color);
    }

    #[test]
    fn half_block_mask_uses_separate_averages() {
        let top = Color::rgb(240, 20, 10);
        let bottom = Color::rgb(10, 30, 220);
        let fitted = fit_mask(
            &solid_work(top, bottom),
            GlyphMask {
                glyph: '▀',
                coverage: UPPER,
            },
        );

        assert_eq!(fitted.error, 0);
        assert_eq!(fitted.cell.foreground, top);
        assert_eq!(fitted.cell.background, bottom);
    }

    #[test]
    fn foreground_and_background_use_rounded_rgb_averages() {
        let mut work = solid_work(Color::rgb(10, 20, 30), Color::rgb(100, 110, 120));
        work[0] = Some(Color::rgb(12, 22, 32));
        work[32] = Some(Color::rgb(102, 112, 122));
        let fitted = fit_mask(
            &work,
            GlyphMask {
                glyph: '▀',
                coverage: UPPER,
            },
        );

        assert_eq!(fitted.cell.foreground, Color::rgb(10, 20, 30));
        assert_eq!(fitted.cell.background, Color::rgb(100, 110, 120));
    }

    #[test]
    fn known_two_color_region_selects_half_block() {
        let top = Color::rgb(255, 0, 0);
        let bottom = Color::rgb(0, 0, 255);
        let cell = select_cell(&solid_work(top, bottom), SMALL_MASKS);

        assert_eq!(cell.glyph, '▀' as u32);
        assert_eq!(cell.foreground, top);
        assert_eq!(cell.background, bottom);
    }

    #[test]
    fn repeated_rendering_is_deterministic() {
        let pixels = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let source =
            PixelFrameView::new(&pixels, 2, 2, 8, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let request = RenderRequest {
            viewport: Viewport {
                width: 2,
                height: 1,
            },
            fit: FitMode::Stretch,
            generation: 7,
        };
        let mut first = CellFrame::default();
        let mut second = CellFrame::default();
        let renderer = ExperimentalGlyphRenderer::new();
        for glyph_set in [
            ExperimentalGlyphSet::Small,
            ExperimentalGlyphSet::Expanded,
            ExperimentalGlyphSet::Reliable,
            ExperimentalGlyphSet::UniqueExpanded,
            ExperimentalGlyphSet::Pruned,
        ] {
            let options = ExperimentalGlyphOptions {
                glyph_set,
                ..ExperimentalGlyphOptions::default()
            };
            renderer
                .render_into(source, request, options, &mut first)
                .unwrap();
            renderer
                .render_into(source, request, options, &mut second)
                .unwrap();
            assert_eq!(first, second);
        }
    }

    #[test]
    fn no_per_cell_allocation_uses_caller_storage_and_fixed_stack_scratch() {
        let pixels = [80_u8, 90, 100, 255];
        let source =
            PixelFrameView::new(&pixels, 1, 1, 4, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let mut output = [Cell::default(); 4];

        ExperimentalGlyphRenderer::new()
            .render_to_slice(
                source,
                RenderRequest {
                    viewport: Viewport {
                        width: 2,
                        height: 2,
                    },
                    fit: FitMode::Stretch,
                    generation: 1,
                },
                ExperimentalGlyphOptions::default(),
                &mut output,
            )
            .unwrap();

        assert_eq!(size_of::<ExperimentalGlyphRenderer>(), 0);
        assert_eq!(
            ExperimentalGlyphRenderer::PHASE_6A_SCRATCH_BYTES_PER_CELL,
            320
        );
        assert_eq!(ExperimentalGlyphRenderer::SCRATCH_BYTES_PER_CELL, 400);
        assert!(output.iter().all(|cell| *cell != Cell::default()));
    }

    #[test]
    fn scratch_storage_is_fixed_and_bounded() {
        assert_eq!(WORK_PIXELS, 64);
        assert_eq!(SMALL_MASKS.len(), 8);
        assert_eq!(EXPANDED_MASKS.len(), 22);
        assert_eq!(RELIABLE_MASKS.len(), 7);
        assert_eq!(UNIQUE_EXPANDED_MASKS.len(), 14);
    }

    #[test]
    fn phase_6b_masks_are_unique_without_complements() {
        for masks in [RELIABLE_MASKS, UNIQUE_EXPANDED_MASKS] {
            for (index, left) in masks.iter().enumerate() {
                for right in &masks[index + 1..] {
                    assert_ne!(left.coverage, right.coverage);
                    assert_ne!(left.coverage, !right.coverage);
                }
            }
        }
    }

    #[test]
    fn reliable_masks_exclude_font_dependent_lines_and_diagonals() {
        let glyphs: Vec<_> = RELIABLE_MASKS.iter().map(|mask| mask.glyph).collect();
        assert_eq!(glyphs, [' ', '▀', '▌', '▘', '▝', '▖', '▗']);
    }

    #[test]
    fn reused_statistics_match_phase_6a_color_fitting_and_error() {
        let mut work = [None; WORK_PIXELS];
        for (index, pixel) in work.iter_mut().enumerate() {
            *pixel = Some(Color::rgb(
                (index * 3) as u8,
                (255 - index * 2) as u8,
                (index * 4) as u8,
            ));
        }
        let statistics = build_statistics(&work, false);
        for &mask in UNIQUE_EXPANDED_MASKS {
            let phase_6a = fit_mask(&work, mask);
            let phase_6b = fit_mask_from_statistics(&work, &statistics, mask);
            assert_eq!(phase_6b.cell, phase_6a.cell);
            assert_eq!(phase_6b.error, phase_6a.error);
        }
    }

    #[test]
    fn pruning_keeps_exact_high_contrast_boundaries() {
        for (expected_index, mask) in UNIQUE_EXPANDED_MASKS.iter().enumerate() {
            let work = mask_work(
                mask.coverage,
                Color::rgb(255, 255, 255),
                Color::rgb(0, 0, 0),
            );
            let statistics = build_statistics(&work, true);
            let mut candidates = [0_u8; UNIQUE_EXPANDED_MASKS.len()];
            let count = prune_candidates(&statistics, UNIQUE_EXPANDED_MASKS, &mut candidates);
            assert!(
                candidates[..count].contains(&(expected_index as u8)),
                "pruned exact mask {}",
                mask.glyph
            );
        }
    }

    #[test]
    fn optimized_uniform_cell_selects_space() {
        let color = Color::rgb(90, 100, 110);
        let work = solid_work(color, color);
        let statistics = build_statistics(&work, false);
        let cell = select_optimized(&work, &statistics, RELIABLE_MASKS);

        assert_eq!(cell.glyph, ' ' as u32);
        assert_eq!(cell.foreground, color);
        assert_eq!(cell.background, color);
    }

    #[test]
    fn optimized_high_contrast_boundary_selects_half_block() {
        let top = Color::rgb(255, 255, 255);
        let bottom = Color::rgb(0, 0, 0);
        let work = solid_work(top, bottom);
        let statistics = build_statistics(&work, false);
        let cell = select_optimized(&work, &statistics, RELIABLE_MASKS);

        assert_eq!(cell.glyph, '▀' as u32);
        assert_eq!(cell.foreground, top);
        assert_eq!(cell.background, bottom);
    }

    #[test]
    fn work_raster_preserves_half_block_aspect_domain() {
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
        let work = work_geometry(geometry).unwrap();

        assert_eq!((geometry.sample_width, geometry.sample_height), (80, 45));
        assert_eq!((work.sample_width, work.sample_height), (640, 180));
        assert_eq!((work.offset_x, work.offset_y), (0, 4));
    }
}
