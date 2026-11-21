use crate::cell::Color;
use crate::framebuffer::{PixelFrameView, decode, with_decoder};
use crate::geometry::RenderGeometry;

/// Pixel resampling strategy used while mapping a source frame to terminal cells.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SamplingFilter {
    /// Select the closest source pixel. Fast and sharp for pixel art.
    #[default]
    Nearest,
    /// Blend the four nearest source pixels for smoother enlargement.
    Bilinear,
    /// Average the covered source area for higher-quality downscaling.
    Area,
}

#[cfg(test)]
pub(crate) fn sample(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    viewport_x: u32,
    viewport_y: u32,
    filter: SamplingFilter,
) -> Option<Color> {
    sample_with_alpha_background(source, geometry, viewport_x, viewport_y, filter, None)
}

/// Samples one output pixel as a color, or `None` for "no content".
///
/// This is the single sampling entry point: [`sample_rgba`] and the legacy
/// [`crate::Renderer`] both derive from it, so they cannot disagree.
pub(crate) fn sample_with_alpha_background(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    viewport_x: u32,
    viewport_y: u32,
    filter: SamplingFilter,
    alpha_background: Option<Color>,
) -> Option<Color> {
    if filter == SamplingFilter::Nearest {
        let pixel = nearest_point(source, geometry, viewport_x, viewport_y)?;
        return resolve_pixel(pixel, alpha_background);
    }
    sample_point(source, geometry, viewport_x, viewport_y, filter)?.resolve(alpha_background)
}

/// Samples one output pixel as straight RGBA8 for the raster stage.
///
/// Wherever [`sample_with_alpha_background`] returns `None` this returns
/// alpha 0, and wherever it returns a color this returns the same RGB with
/// nonzero alpha. With a matte the result is opaque. Without one, alpha is
/// the rounded coverage but never 0 for a sample with any coverage, so
/// visibility is preserved exactly.
pub(crate) fn sample_rgba(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    x: u32,
    y: u32,
    filter: SamplingFilter,
    alpha_background: Option<Color>,
) -> [u8; 4] {
    if filter == SamplingFilter::Nearest {
        let Some(pixel) = nearest_point(source, geometry, x, y) else {
            return [0; 4];
        };
        let Some(color) = resolve_pixel(pixel, alpha_background) else {
            return [0; 4];
        };
        // A single pixel's coverage is its own alpha.
        let alpha = if alpha_background.is_some() {
            255
        } else {
            pixel[3]
        };
        return [color.red, color.green, color.blue, alpha];
    }
    let Some(sample) = sample_point(source, geometry, x, y, filter) else {
        return [0; 4];
    };
    let Some(color) = sample.resolve(alpha_background) else {
        return [0; 4];
    };
    let alpha = if alpha_background.is_some() {
        255
    } else {
        sample.coverage()
    };
    [color.red, color.green, color.blue, alpha]
}

/// Fills one row of output pixels, starting at `(x, y)`, with the Bilinear
/// filter: the same values [`sample_rgba`] gives pixel by pixel.
///
/// The vertical interpolation is computed once for the row. When all four
/// source pixels are opaque, which is nearly always true of decoded video,
/// photos, and rendered scenes, the result is the rounded bilinear blend of
/// the channels. That is what the general alpha-weighted path reduces to for
/// alpha 255: it multiplies by 255, then divides by 255 in `resolve`, and the
/// coverage is 255. Other pixels take the general path.
pub(crate) fn bilinear_row(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    x: u32,
    y: u32,
    row: &mut [u8],
    alpha_background: Option<Color>,
) {
    let render_y = i64::from(y) - geometry.offset_y;
    if render_y < 0 || render_y >= i64::from(geometry.sample_height) {
        row.fill(0);
        return;
    }
    let vertical = interpolation_axis(render_y as u32, source.height(), geometry.sample_height);
    // The two source rows every pixel of this output row blends, borrowed
    // once and decoded by a reader specialized for the layout.
    let (Some(upper), Some(lower)) = (source.row(vertical.low), source.row(vertical.high)) else {
        row.fill(0);
        return;
    };
    let columns = image_columns(geometry, x, row.len() / 4);
    row[..columns.start * 4].fill(0);
    row[columns.end * 4..].fill(0);
    if columns.is_empty() {
        return;
    }
    let first = (i64::from(x) + columns.start as i64 - geometry.offset_x) as u32;
    let mut walk = interpolation_walk(first, source.width(), geometry.sample_width);
    with_decoder!(source, D => {
        for (index, pixel) in row[columns.start * 4..columns.end * 4].chunks_exact_mut(4).enumerate() {
            let output_x = x + (columns.start + index) as u32;
            let horizontal = match walk.as_mut() {
                Some(walk) => walk(),
                None => interpolation_axis(first + index as u32, source.width(), geometry.sample_width),
            };
            let (low, high) = (horizontal.low as usize * 4, horizontal.high as usize * 4);
            let top_left = decode::<D>(&upper[low..low + 4]);
            let top_right = decode::<D>(&upper[high..high + 4]);
            let bottom_left = decode::<D>(&lower[low..low + 4]);
            let bottom_right = decode::<D>(&lower[high..high + 4]);
            if top_left[3] & top_right[3] & bottom_left[3] & bottom_right[3] == 255 {
                let channel = |index: usize| {
                    bilinear_rounded_value(
                        u64::from(top_left[index]),
                        u64::from(top_right[index]),
                        u64::from(bottom_left[index]),
                        u64::from(bottom_right[index]),
                        horizontal.fraction,
                        vertical.fraction,
                    ) as u8
                };
                pixel.copy_from_slice(&[channel(0), channel(1), channel(2), 255]);
            } else {
                pixel.copy_from_slice(&sample_rgba(
                    source,
                    geometry,
                    output_x,
                    y,
                    SamplingFilter::Bilinear,
                    alpha_background,
                ));
            }
        }
    });
}

/// Fills one row of output pixels, starting at `(x, y)`, with the Area
/// filter: the same values [`sample_rgba`] gives pixel by pixel.
///
/// The box's rows are computed once for the output row, and source rows are
/// read through a reader specialized for the layout. The sums, and their
/// rounding in [`AlphaSample`], are exactly the per-pixel filter's.
pub(crate) fn area_row(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    x: u32,
    y: u32,
    row: &mut [u8],
    alpha_background: Option<Color>,
) {
    if geometry.sample_width >= source.width() || geometry.sample_height >= source.height() {
        // Enlarging on either axis: Area samples bilinearly (see `sample_area`).
        bilinear_row(source, geometry, x, y, row, alpha_background);
        return;
    }
    let render_y = i64::from(y) - geometry.offset_y;
    if render_y < 0 || render_y >= i64::from(geometry.sample_height) {
        row.fill(0);
        return;
    }
    let (source_width, source_height) = (u64::from(source.width()), u64::from(source.height()));
    let (destination_width, destination_height) = (
        u64::from(geometry.sample_width),
        u64::from(geometry.sample_height),
    );
    let render_y = render_y as u64;
    let start_y = render_y * source_height / destination_height;
    let end_y = ((render_y + 1) * source_height)
        .div_ceil(destination_height)
        .min(source_height);
    let columns = image_columns(geometry, x, row.len() / 4);
    row[..columns.start * 4].fill(0);
    row[columns.end * 4..].fill(0);
    if columns.is_empty() {
        return;
    }
    let first = (i64::from(x) + columns.start as i64 - geometry.offset_x) as u64;
    // Box edges: floor(render_x·S / D) and ceil((render_x + 1)·S / D).
    let mut starts = DivWalk::new(first * source_width, source_width, destination_width);
    let mut ends = DivWalk::new(
        (first + 1) * source_width + destination_width - 1,
        source_width,
        destination_width,
    );
    with_decoder!(source, D => {
        for pixel in row[columns.start * 4..columns.end * 4].chunks_exact_mut(4) {
            let start_x = starts.next() as usize * 4;
            let end_x = ends.next().min(source_width) as usize * 4;
            let mut sample = AlphaSample {
                red_alpha: 0,
                green_alpha: 0,
                blue_alpha: 0,
                alpha: 0,
                weight: 0,
            };
            for source_y in start_y..end_y {
                let Some(line) = source.row(source_y as u32) else {
                    continue;
                };
                for bytes in line[start_x..end_x].chunks_exact(4) {
                    let [red, green, blue, alpha] = decode::<D>(bytes);
                    let alpha = u64::from(alpha);
                    sample.red_alpha += u64::from(red) * alpha;
                    sample.green_alpha += u64::from(green) * alpha;
                    sample.blue_alpha += u64::from(blue) * alpha;
                    sample.alpha += alpha;
                    sample.weight += 1;
                }
            }
            pixel.copy_from_slice(&resolve_rgba(sample, alpha_background));
        }
    });
}

/// The output columns `x..x + width` that show the image; the others are
/// letterboxing (or panned away).
fn image_columns(geometry: &RenderGeometry, x: u32, width: usize) -> core::ops::Range<usize> {
    let clamp = |render_x: i64| {
        (render_x + geometry.offset_x - i64::from(x)).clamp(0, width as i64) as usize
    };
    clamp(0)..clamp(i64::from(geometry.sample_width))
}

/// A filtered sample as the straight RGBA [`sample_rgba`] writes.
#[inline]
fn resolve_rgba(sample: AlphaSample, alpha_background: Option<Color>) -> [u8; 4] {
    if sample.weight == 0 {
        return [0; 4];
    }
    let Some(color) = sample.resolve(alpha_background) else {
        return [0; 4];
    };
    let alpha = if alpha_background.is_some() {
        255
    } else {
        sample.coverage()
    };
    [color.red, color.green, color.blue, alpha]
}

/// The source pixel nearest to output pixel `(x, y)`, as straight RGBA, or
/// `None` outside the fitted image. Same mapping as the Nearest filter.
#[inline]
fn nearest_point(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    x: u32,
    y: u32,
) -> Option<[u8; 4]> {
    let render_x = i64::from(x) - geometry.offset_x;
    let render_y = i64::from(y) - geometry.offset_y;
    if render_x < 0
        || render_y < 0
        || render_x >= i64::from(geometry.sample_width)
        || render_y >= i64::from(geometry.sample_height)
    {
        return None;
    }
    let source_x = u64::try_from(render_x).ok()? * u64::from(source.width())
        / u64::from(geometry.sample_width);
    let source_y = u64::try_from(render_y).ok()? * u64::from(source.height())
        / u64::from(geometry.sample_height);
    let (x, y) = (u32::try_from(source_x).ok()?, u32::try_from(source_y).ok()?);
    if source.is_straight_rgba_top_down() {
        read_rgba::<false>(source, x, y)
    } else {
        read_rgba::<true>(source, x, y)
    }
}

/// Resolves one straight source pixel exactly as [`AlphaSample::resolve`]
/// does for a weight-1 sample of it, without its per-channel divisions:
/// `(c·a + a/2) / a == c`, and compositing divides by the constant 255.
#[inline]
fn resolve_pixel(pixel: [u8; 4], background: Option<Color>) -> Option<Color> {
    let [red, green, blue, alpha] = pixel;
    if alpha == 0 {
        return background;
    }
    let Some(background) = background else {
        return Some(Color::rgb(red, green, blue));
    };
    let alpha = u32::from(alpha);
    let inverse = 255 - alpha;
    let mix = |channel: u8, under: u8| {
        ((u32::from(channel) * alpha + u32::from(under) * inverse + 127) / 255) as u8
    };
    Some(Color::rgb(
        mix(red, background.red),
        mix(green, background.green),
        mix(blue, background.blue),
    ))
}

/// Maps an output pixel into the fitted image and filters the source there.
/// `None` outside the fitted image.
#[inline]
fn sample_point(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    x: u32,
    y: u32,
    filter: SamplingFilter,
) -> Option<AlphaSample> {
    let render_x = i64::from(x) - geometry.offset_x;
    let render_y = i64::from(y) - geometry.offset_y;
    if render_x < 0
        || render_y < 0
        || render_x >= i64::from(geometry.sample_width)
        || render_y >= i64::from(geometry.sample_height)
    {
        return None;
    }
    filtered(source, geometry, render_x, render_y, filter)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AlphaSample {
    red_alpha: u64,
    green_alpha: u64,
    blue_alpha: u64,
    alpha: u64,
    weight: u64,
}

impl AlphaSample {
    /// Average alpha rounded to 8 bits, clamped to at least 1 when any
    /// coverage exists so that "visible" survives the conversion.
    fn coverage(self) -> u8 {
        if self.alpha == 0 {
            return 0;
        }
        let average = (self.alpha + self.weight / 2) / self.weight;
        average.clamp(1, 255) as u8
    }

    fn resolve(self, background: Option<Color>) -> Option<Color> {
        if self.alpha == 0 {
            return background;
        }
        if let Some(background) = background {
            let maximum_alpha = 255 * self.weight;
            let inverse = maximum_alpha - self.alpha;
            return Some(Color::rgb(
                ((self.red_alpha + u64::from(background.red) * inverse + maximum_alpha / 2)
                    / maximum_alpha) as u8,
                ((self.green_alpha + u64::from(background.green) * inverse + maximum_alpha / 2)
                    / maximum_alpha) as u8,
                ((self.blue_alpha + u64::from(background.blue) * inverse + maximum_alpha / 2)
                    / maximum_alpha) as u8,
            ));
        }
        Some(Color::rgb(
            ((self.red_alpha + self.alpha / 2) / self.alpha) as u8,
            ((self.green_alpha + self.alpha / 2) / self.alpha) as u8,
            ((self.blue_alpha + self.alpha / 2) / self.alpha) as u8,
        ))
    }
}

fn sample_area<const NORMALIZE: bool>(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    render_x: i64,
    render_y: i64,
) -> Option<AlphaSample> {
    if geometry.sample_width >= source.width() || geometry.sample_height >= source.height() {
        return sample_bilinear::<NORMALIZE>(source, geometry, render_x, render_y);
    }

    let render_x = u64::try_from(render_x).ok()?;
    let render_y = u64::try_from(render_y).ok()?;
    let source_width = u64::from(source.width());
    let source_height = u64::from(source.height());
    let destination_width = u64::from(geometry.sample_width);
    let destination_height = u64::from(geometry.sample_height);
    let start_x = render_x * source_width / destination_width;
    let end_x = ((render_x + 1) * source_width).div_ceil(destination_width);
    let start_y = render_y * source_height / destination_height;
    let end_y = ((render_y + 1) * source_height).div_ceil(destination_height);

    let mut red_alpha = 0_u64;
    let mut green_alpha = 0_u64;
    let mut blue_alpha = 0_u64;
    let mut alpha = 0_u64;
    let mut count = 0_u64;
    for source_y in start_y..end_y.min(source_height) {
        for source_x in start_x..end_x.min(source_width) {
            let color = read_pixel::<NORMALIZE>(
                source,
                u32::try_from(source_x).ok()?,
                u32::try_from(source_y).ok()?,
            )?;
            red_alpha += color.red_alpha;
            green_alpha += color.green_alpha;
            blue_alpha += color.blue_alpha;
            alpha += color.alpha;
            count += 1;
        }
    }
    if count == 0 {
        return None;
    }
    Some(AlphaSample {
        red_alpha,
        green_alpha,
        blue_alpha,
        alpha,
        weight: count,
    })
}

fn sample_nearest<const NORMALIZE: bool>(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    render_x: i64,
    render_y: i64,
) -> Option<AlphaSample> {
    let source_x = u64::try_from(render_x).ok()? * u64::from(source.width())
        / u64::from(geometry.sample_width);
    let source_y = u64::try_from(render_y).ok()? * u64::from(source.height())
        / u64::from(geometry.sample_height);
    read_pixel::<NORMALIZE>(
        source,
        u32::try_from(source_x).ok()?,
        u32::try_from(source_y).ok()?,
    )
}

fn sample_bilinear<const NORMALIZE: bool>(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    render_x: i64,
    render_y: i64,
) -> Option<AlphaSample> {
    let x = interpolation_axis(
        u32::try_from(render_x).ok()?,
        source.width(),
        geometry.sample_width,
    );
    let y = interpolation_axis(
        u32::try_from(render_y).ok()?,
        source.height(),
        geometry.sample_height,
    );
    let top_left = read_pixel::<NORMALIZE>(source, x.low, y.low)?;
    let top_right = read_pixel::<NORMALIZE>(source, x.high, y.low)?;
    let bottom_left = read_pixel::<NORMALIZE>(source, x.low, y.high)?;
    let bottom_right = read_pixel::<NORMALIZE>(source, x.high, y.high)?;
    if [top_left, top_right, bottom_left, bottom_right]
        .iter()
        .all(|sample| sample.alpha == 255)
    {
        return Some(AlphaSample {
            red_alpha: bilinear_rounded_value(
                top_left.red_alpha / 255,
                top_right.red_alpha / 255,
                bottom_left.red_alpha / 255,
                bottom_right.red_alpha / 255,
                x.fraction,
                y.fraction,
            ) * 255,
            green_alpha: bilinear_rounded_value(
                top_left.green_alpha / 255,
                top_right.green_alpha / 255,
                bottom_left.green_alpha / 255,
                bottom_right.green_alpha / 255,
                x.fraction,
                y.fraction,
            ) * 255,
            blue_alpha: bilinear_rounded_value(
                top_left.blue_alpha / 255,
                top_right.blue_alpha / 255,
                bottom_left.blue_alpha / 255,
                bottom_right.blue_alpha / 255,
                x.fraction,
                y.fraction,
            ) * 255,
            alpha: 255,
            weight: 1,
        });
    }
    let weight = 65_536_u64 * 65_536_u64;

    Some(AlphaSample {
        red_alpha: bilinear_weighted_sum(
            top_left.red_alpha,
            top_right.red_alpha,
            bottom_left.red_alpha,
            bottom_right.red_alpha,
            x.fraction,
            y.fraction,
        ),
        green_alpha: bilinear_weighted_sum(
            top_left.green_alpha,
            top_right.green_alpha,
            bottom_left.green_alpha,
            bottom_right.green_alpha,
            x.fraction,
            y.fraction,
        ),
        blue_alpha: bilinear_weighted_sum(
            top_left.blue_alpha,
            top_right.blue_alpha,
            bottom_left.blue_alpha,
            bottom_right.blue_alpha,
            x.fraction,
            y.fraction,
        ),
        alpha: bilinear_weighted_sum(
            top_left.alpha,
            top_right.alpha,
            bottom_left.alpha,
            bottom_right.alpha,
            x.fraction,
            y.fraction,
        ),
        weight,
    })
}

#[derive(Clone, Copy)]
struct AxisSample {
    low: u32,
    high: u32,
    fraction: u16,
}

/// Successive quotients `(start + k·step) / denominator` for k = 0, 1, …,
/// without dividing: the quotient and remainder advance by fixed amounts.
/// Exact as long as the numerators fit in `u64`, which callers check.
#[derive(Clone, Copy)]
struct DivWalk {
    quotient: u64,
    remainder: u64,
    step_quotient: u64,
    step_remainder: u64,
    denominator: u64,
}

impl DivWalk {
    fn new(start: u64, step: u64, denominator: u64) -> Self {
        Self {
            quotient: start / denominator,
            remainder: start % denominator,
            step_quotient: step / denominator,
            step_remainder: step % denominator,
            denominator,
        }
    }

    /// The current quotient; then advances to the next numerator.
    #[inline(always)]
    fn next(&mut self) -> u64 {
        let quotient = self.quotient;
        self.quotient += self.step_quotient;
        self.remainder += self.step_remainder;
        if self.remainder >= self.denominator {
            self.quotient += 1;
            self.remainder -= self.denominator;
        }
        quotient
    }
}

/// [`interpolation_axis`] for destinations `first`, `first + 1`, … in turn,
/// or `None` if its numerators could exceed `u64` (then call it directly).
fn interpolation_walk(
    first: u32,
    source_len: u32,
    destination_len: u32,
) -> Option<impl FnMut() -> AxisSample> {
    const ONE: u64 = 1 << 16;
    let source = u64::from(source_len);
    // The largest numerator, for destination_len - 1: (2·D − 1)·S·ONE.
    (u64::from(destination_len) * 2)
        .checked_mul(source)?
        .checked_mul(ONE)?;
    let mut walk = DivWalk::new(
        (u64::from(first) * 2 + 1) * source * ONE,
        2 * source * ONE,
        u64::from(destination_len) * 2,
    );
    let (maximum, last) = ((source - 1) * ONE, source_len - 1);
    Some(move || {
        // interpolation_axis: centre − ½, clamped to [0, (S − 1)·ONE].
        let fixed = walk.next().saturating_sub(ONE / 2).min(maximum);
        let low = (fixed / ONE) as u32;
        AxisSample {
            low,
            high: low.saturating_add(1).min(last),
            fraction: (fixed & (ONE - 1)) as u16,
        }
    })
}

fn interpolation_axis(destination: u32, source_len: u32, destination_len: u32) -> AxisSample {
    const ONE: i128 = 1 << 16;
    // Same fixed-point centre as the i128 form below, computed in u64 when it
    // fits (it does for any realistic image): i128 division is a slow
    // library call and dominated bilinear sampling.
    let centered = (u64::from(destination) * 2 + 1)
        .checked_mul(u64::from(source_len))
        .and_then(|value| value.checked_mul(1 << 16))
        .map(|numerator| i128::from(numerator / (u64::from(destination_len) * 2)) - ONE / 2)
        .unwrap_or_else(|| {
            ((i128::from(destination) * 2 + 1) * i128::from(source_len) * ONE
                / (i128::from(destination_len) * 2))
                - ONE / 2
        });
    let maximum = i128::from(source_len - 1) * ONE;
    let fixed = centered.clamp(0, maximum);
    let low = u32::try_from(fixed / ONE).unwrap_or(source_len - 1);
    AxisSample {
        low,
        high: low.saturating_add(1).min(source_len - 1),
        fraction: u16::try_from(fixed & (ONE - 1)).unwrap_or(u16::MAX),
    }
}

fn bilinear_weighted_sum(
    top_left: u64,
    top_right: u64,
    bottom_left: u64,
    bottom_right: u64,
    x_fraction: u16,
    y_fraction: u16,
) -> u64 {
    let right_weight = u64::from(x_fraction);
    let left_weight = 65_536 - right_weight;
    let bottom_weight = u64::from(y_fraction);
    let top_weight = 65_536 - bottom_weight;
    top_left * left_weight * top_weight
        + top_right * right_weight * top_weight
        + bottom_left * left_weight * bottom_weight
        + bottom_right * right_weight * bottom_weight
}

fn bilinear_rounded_value(
    top_left: u64,
    top_right: u64,
    bottom_left: u64,
    bottom_right: u64,
    x_fraction: u16,
    y_fraction: u16,
) -> u64 {
    let top = interpolate_rounded(top_left, top_right, x_fraction);
    let bottom = interpolate_rounded(bottom_left, bottom_right, x_fraction);
    interpolate_rounded(top, bottom, y_fraction)
}

fn interpolate_rounded(left: u64, right: u64, fraction: u16) -> u64 {
    let inverse = 65_536_u64 - u64::from(fraction);
    (left * inverse + right * u64::from(fraction) + 32_768) >> 16
}

/// Chooses the pixel reader once per sample, so filter loops over many
/// source pixels stay branch-free for the canonical layout.
fn filtered(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    render_x: i64,
    render_y: i64,
    filter: SamplingFilter,
) -> Option<AlphaSample> {
    fn run<const NORMALIZE: bool>(
        source: &PixelFrameView<'_>,
        geometry: &RenderGeometry,
        render_x: i64,
        render_y: i64,
        filter: SamplingFilter,
    ) -> Option<AlphaSample> {
        match filter {
            SamplingFilter::Nearest => {
                sample_nearest::<NORMALIZE>(source, geometry, render_x, render_y)
            }
            SamplingFilter::Bilinear => {
                sample_bilinear::<NORMALIZE>(source, geometry, render_x, render_y)
            }
            SamplingFilter::Area => sample_area::<NORMALIZE>(source, geometry, render_x, render_y),
        }
    }
    if source.is_straight_rgba_top_down() {
        run::<false>(source, geometry, render_x, render_y, filter)
    } else {
        run::<true>(source, geometry, render_x, render_y, filter)
    }
}

/// Reads one pixel as straight RGBA.
///
/// `NORMALIZE = false` is only valid for straight, top-down RGBA8 views and
/// reads the bytes directly. `NORMALIZE = true` goes through
/// [`PixelFrameView::rgba`], which handles every layout.
#[inline(always)]
fn read_rgba<const NORMALIZE: bool>(
    source: &PixelFrameView<'_>,
    x: u32,
    y: u32,
) -> Option<[u8; 4]> {
    if NORMALIZE {
        source.rgba(x, y)
    } else {
        let offset = usize::try_from(y)
            .ok()?
            .checked_mul(source.stride())?
            .checked_add(usize::try_from(x).ok()?.checked_mul(4)?)?;
        let pixel = source.pixels().get(offset..offset + 4)?;
        Some([pixel[0], pixel[1], pixel[2], pixel[3]])
    }
}

/// Reads one pixel and weights its color by alpha for filtering.
///
/// Deliberately not written in terms of [`read_rgba`]: that form measured
/// 10–20% slower in the Area filter's inner loop.
#[inline(always)]
fn read_pixel<const NORMALIZE: bool>(
    source: &PixelFrameView<'_>,
    x: u32,
    y: u32,
) -> Option<AlphaSample> {
    let [red, green, blue, alpha] = if NORMALIZE {
        source.rgba(x, y)?
    } else {
        let offset = usize::try_from(y)
            .ok()?
            .checked_mul(source.stride())?
            .checked_add(usize::try_from(x).ok()?.checked_mul(4)?)?;
        let pixel = source.pixels().get(offset..offset + 4)?;
        [pixel[0], pixel[1], pixel[2], pixel[3]]
    };
    let alpha = u64::from(alpha);
    Some(AlphaSample {
        red_alpha: u64::from(red) * alpha,
        green_alpha: u64::from(green) * alpha,
        blue_alpha: u64::from(blue) * alpha,
        alpha,
        weight: 1,
    })
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::*;
    use crate::framebuffer::PixelFormat;
    use crate::geometry::{FitMode, Viewport};

    /// Walking the interpolation axis gives exactly `interpolation_axis`,
    /// including the clamped ends and enlargement, from any first column.
    #[test]
    fn interpolation_walk_matches_interpolation_axis() {
        for (source, destination) in [
            (1, 1),
            (1, 7),
            (2, 3),
            (7, 3),
            (640, 534),
            (640, 800),
            (1920, 401),
            (3, 1000),
            (65_537, 257),
        ] {
            for first in [0, 1, destination / 2, destination - 1] {
                let mut walk = interpolation_walk(first, source, destination).unwrap();
                for d in first..destination {
                    let (walked, direct) = (walk(), interpolation_axis(d, source, destination));
                    assert_eq!(
                        (walked.low, walked.high, walked.fraction),
                        (direct.low, direct.high, direct.fraction),
                        "{source} -> {destination} at {d}"
                    );
                }
            }
        }
        assert!(interpolation_walk(0, u32::MAX, u32::MAX).is_none());
    }

    /// `DivWalk` yields the same quotients as dividing each numerator.
    #[test]
    fn division_walk_is_exact() {
        for (start, step, denominator) in [
            (0, 1, 1),
            (5, 7, 3),
            (640 * 3, 640, 534),
            (1_000_003, 999, 1_000),
            (7, 0, 2),
        ] {
            let mut walk = DivWalk::new(start, step, denominator);
            for k in 0..5_000 {
                assert_eq!(walk.next(), (start + k * step) / denominator);
            }
        }
    }

    /// The Area row path equals the per-pixel sampler for every layout,
    /// alpha mix, matte, and placement, shrinking and enlarging.
    #[test]
    fn area_rows_match_per_pixel_sampling() {
        use crate::framebuffer::{AlphaMode, PixelLayout, RowOrder};
        use crate::geometry::{RasterSpec, RenderTransform, SubcellGrid};

        let (width, height) = (97_u32, 61_u32);
        let mut state = 0x9E37_79B9_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for alpha_style in 0..3 {
            let bytes: Vec<u8> = (0..width * height)
                .flat_map(|_| {
                    let value = next();
                    let alpha = match alpha_style {
                        0 => 255,
                        1 => [0, 255, 255, 128][(value >> 24) as usize % 4],
                        _ => (value >> 24) as u8,
                    };
                    [value as u8, (value >> 8) as u8, (value >> 16) as u8, alpha]
                })
                .collect();
            for (format, alpha_mode, order) in [
                (PixelFormat::Rgba8, AlphaMode::Straight, RowOrder::TopDown),
                (
                    PixelFormat::Bgra8,
                    AlphaMode::Premultiplied,
                    RowOrder::BottomUp,
                ),
                (PixelFormat::Rgba8, AlphaMode::Ignore, RowOrder::BottomUp),
            ] {
                let layout =
                    PixelLayout::new(width, height, format, alpha_mode).with_row_order(order);
                let source = PixelFrameView::from_layout(&bytes, layout).unwrap();
                for (viewport, grid, fit, transform) in [
                    (
                        (9, 4),
                        SubcellGrid::HALF_BLOCK,
                        FitMode::Contain,
                        RenderTransform::default(),
                    ),
                    (
                        (31, 11),
                        SubcellGrid::SEXTANT,
                        FitMode::Contain,
                        RenderTransform::default(),
                    ),
                    (
                        (17, 6),
                        SubcellGrid::QUADRANT,
                        FitMode::Cover,
                        RenderTransform::default(),
                    ),
                    (
                        (20, 8),
                        SubcellGrid::BRAILLE,
                        FitMode::Stretch,
                        RenderTransform {
                            zoom_percent: 130,
                            pan_x_cells: 3,
                            pan_y_cells: -2,
                            ..RenderTransform::default()
                        },
                    ),
                    (
                        (70, 40),
                        SubcellGrid::HALF_BLOCK,
                        FitMode::Contain,
                        RenderTransform::default(),
                    ),
                ] {
                    let spec = RasterSpec::new(
                        Viewport {
                            width: viewport.0,
                            height: viewport.1,
                        },
                        grid,
                        crate::geometry::CellAspect::default(),
                    )
                    .unwrap();
                    let geometry =
                        RenderGeometry::calculate_for_spec(width, height, &spec, fit, transform)
                            .unwrap();
                    for matte in [None, Some(Color::rgb(9, 99, 199))] {
                        let mut row = vec![0_u8; spec.width() as usize * 4];
                        for y in 0..spec.height() {
                            area_row(&source, &geometry, 0, y, &mut row, matte);
                            for x in 0..spec.width() {
                                let expected = sample_rgba(
                                    &source,
                                    &geometry,
                                    x,
                                    y,
                                    SamplingFilter::Area,
                                    matte,
                                );
                                assert_eq!(
                                    row[x as usize * 4..x as usize * 4 + 4],
                                    expected,
                                    "({x}, {y}) {format:?} {alpha_mode:?} {fit:?} {matte:?} {viewport:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// The row fast path equals the per-pixel sampler for every layout,
    /// alpha mix, matte, and placement (including letterboxing and pans).
    #[test]
    fn bilinear_rows_match_per_pixel_sampling() {
        use crate::framebuffer::{AlphaMode, PixelLayout, RowOrder};
        use crate::geometry::{RasterSpec, RenderTransform, SubcellGrid};

        let (width, height) = (23_u32, 17_u32);
        let mut state = 0x2545_F491_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for alpha_style in 0..3 {
            let bytes: Vec<u8> = (0..width * height)
                .flat_map(|_| {
                    let value = next();
                    let alpha = match alpha_style {
                        0 => 255,
                        1 => [0, 255, 255, 128][(value >> 24) as usize % 4],
                        _ => (value >> 24) as u8,
                    };
                    [value as u8, (value >> 8) as u8, (value >> 16) as u8, alpha]
                })
                .collect();
            for (format, alpha_mode, order) in [
                (PixelFormat::Rgba8, AlphaMode::Straight, RowOrder::TopDown),
                (
                    PixelFormat::Bgra8,
                    AlphaMode::Premultiplied,
                    RowOrder::BottomUp,
                ),
                (PixelFormat::Rgba8, AlphaMode::Ignore, RowOrder::TopDown),
            ] {
                let layout =
                    PixelLayout::new(width, height, format, alpha_mode).with_row_order(order);
                let source = PixelFrameView::from_layout(&bytes, layout).unwrap();
                for (viewport, fit, transform) in [
                    ((31, 9), FitMode::Contain, RenderTransform::default()),
                    ((12, 5), FitMode::Cover, RenderTransform::default()),
                    (
                        (20, 8),
                        FitMode::Stretch,
                        RenderTransform {
                            zoom_percent: 170,
                            pan_x_cells: 3,
                            pan_y_cells: -2,
                            ..RenderTransform::default()
                        },
                    ),
                ] {
                    let spec = RasterSpec::new(
                        Viewport {
                            width: viewport.0,
                            height: viewport.1,
                        },
                        SubcellGrid::QUADRANT,
                        crate::geometry::CellAspect::default(),
                    )
                    .unwrap();
                    let geometry =
                        RenderGeometry::calculate_for_spec(width, height, &spec, fit, transform)
                            .unwrap();
                    for matte in [None, Some(Color::rgb(9, 99, 199))] {
                        let mut row = vec![0_u8; spec.width() as usize * 4];
                        for y in 0..spec.height() {
                            bilinear_row(&source, &geometry, 0, y, &mut row, matte);
                            for x in 0..spec.width() {
                                let expected = sample_rgba(
                                    &source,
                                    &geometry,
                                    x,
                                    y,
                                    SamplingFilter::Bilinear,
                                    matte,
                                );
                                assert_eq!(
                                    row[x as usize * 4..x as usize * 4 + 4],
                                    expected,
                                    "({x}, {y}) {format:?} {alpha_mode:?} {fit:?} {matte:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn bilinear_interpolates_center_between_four_pixels() {
        let pixels = [
            0, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255, 255, 255, 255, 255,
        ];
        let source =
            PixelFrameView::new(&pixels, 2, 2, 8, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let geometry = RenderGeometry::calculate(
            2,
            2,
            Viewport {
                width: 3,
                height: 2,
            },
            FitMode::Stretch,
        )
        .unwrap();

        assert_eq!(
            sample(&source, &geometry, 1, 1, SamplingFilter::Bilinear),
            Some(Color::rgb(128, 64, 32))
        );
    }

    #[test]
    fn nearest_preserves_source_color_exactly() {
        let pixels = [7, 11, 13, 255];
        let source =
            PixelFrameView::new(&pixels, 1, 1, 4, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let geometry = RenderGeometry::calculate(
            1,
            1,
            Viewport {
                width: 2,
                height: 1,
            },
            FitMode::Stretch,
        )
        .unwrap();

        assert_eq!(
            sample(&source, &geometry, 1, 1, SamplingFilter::Nearest),
            Some(Color::rgb(7, 11, 13))
        );
    }

    #[test]
    fn area_filter_averages_source_coverage_when_downscaling() {
        let pixels = [
            0, 0, 0, 255, 200, 0, 0, 255, 0, 100, 0, 255, 200, 100, 0, 255,
        ];
        let source =
            PixelFrameView::new(&pixels, 2, 2, 8, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let geometry = RenderGeometry::calculate(
            2,
            2,
            Viewport {
                width: 1,
                height: 1,
            },
            FitMode::Stretch,
        )
        .unwrap();

        assert_eq!(
            sample(&source, &geometry, 0, 0, SamplingFilter::Area),
            Some(Color::rgb(100, 0, 0))
        );
        assert_eq!(
            sample(&source, &geometry, 0, 1, SamplingFilter::Area),
            Some(Color::rgb(100, 100, 0))
        );
    }

    #[test]
    fn transparent_pixels_are_not_opaque_black() {
        let geometry = RenderGeometry::calculate(
            1,
            1,
            Viewport {
                width: 1,
                height: 1,
            },
            FitMode::Stretch,
        )
        .unwrap();
        for pixels in [[0, 0, 0, 0], [255, 0, 0, 0]] {
            let source =
                PixelFrameView::new(&pixels, 1, 1, 4, PixelFormat::Rgba8, Duration::ZERO).unwrap();
            assert_eq!(
                sample(&source, &geometry, 0, 0, SamplingFilter::Nearest),
                None
            );
        }

        let opaque = [0, 0, 0, 255];
        let source =
            PixelFrameView::new(&opaque, 1, 1, 4, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        assert_eq!(
            sample(&source, &geometry, 0, 0, SamplingFilter::Nearest),
            Some(Color::rgb(0, 0, 0))
        );
    }

    #[test]
    fn partial_alpha_is_straight_without_a_matte_and_composites_when_requested() {
        let pixels = [200, 100, 50, 128];
        let source =
            PixelFrameView::new(&pixels, 1, 1, 4, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let geometry = RenderGeometry::calculate(
            1,
            1,
            Viewport {
                width: 1,
                height: 1,
            },
            FitMode::Stretch,
        )
        .unwrap();

        assert_eq!(
            sample(&source, &geometry, 0, 0, SamplingFilter::Nearest),
            Some(Color::rgb(200, 100, 50))
        );
        assert_eq!(
            sample_with_alpha_background(
                &source,
                &geometry,
                0,
                0,
                SamplingFilter::Nearest,
                Some(Color::rgb(20, 40, 60)),
            ),
            Some(Color::rgb(110, 70, 55))
        );
    }

    /// The original all-i128 axis calculation, kept as a reference.
    fn reference_axis(destination: u32, source_len: u32, destination_len: u32) -> AxisSample {
        const ONE: i128 = 1 << 16;
        let centered = ((i128::from(destination) * 2 + 1) * i128::from(source_len) * ONE
            / (i128::from(destination_len) * 2))
            - ONE / 2;
        let maximum = i128::from(source_len - 1) * ONE;
        let fixed = centered.clamp(0, maximum);
        let low = u32::try_from(fixed / ONE).unwrap_or(source_len - 1);
        AxisSample {
            low,
            high: low.saturating_add(1).min(source_len - 1),
            fraction: u16::try_from(fixed & (ONE - 1)).unwrap_or(u16::MAX),
        }
    }

    #[test]
    fn fast_interpolation_axis_matches_the_i128_reference() {
        let lengths = [
            1_u32,
            2,
            3,
            7,
            37,
            64,
            300,
            1080,
            1920,
            4096,
            65_535,
            u32::MAX,
        ];
        for &source_len in &lengths {
            for &destination_len in &lengths {
                let destinations = [0, 1, destination_len / 2, destination_len.saturating_sub(1)];
                for destination in destinations {
                    let (fast, reference) = (
                        interpolation_axis(destination, source_len, destination_len),
                        reference_axis(destination, source_len, destination_len),
                    );
                    assert_eq!(
                        (fast.low, fast.high, fast.fraction),
                        (reference.low, reference.high, reference.fraction),
                        "{destination} of {destination_len} over {source_len}"
                    );
                }
            }
        }
    }

    #[test]
    fn single_pixel_resolution_matches_weighted_resolution() {
        // Every alpha and a spread of channels, with and without a matte.
        let mattes = [
            None,
            Some(Color::rgb(20, 40, 60)),
            Some(Color::rgb(255, 0, 255)),
        ];
        for alpha in 0..=255_u16 {
            for channel in (0..=255_u16).step_by(3) {
                let pixel = [
                    channel as u8,
                    (255 - channel) as u8,
                    (channel / 2) as u8,
                    alpha as u8,
                ];
                let weighted = AlphaSample {
                    red_alpha: u64::from(pixel[0]) * u64::from(alpha),
                    green_alpha: u64::from(pixel[1]) * u64::from(alpha),
                    blue_alpha: u64::from(pixel[2]) * u64::from(alpha),
                    alpha: u64::from(alpha),
                    weight: 1,
                };
                for matte in mattes {
                    assert_eq!(
                        resolve_pixel(pixel, matte),
                        weighted.resolve(matte),
                        "{pixel:?} {matte:?}"
                    );
                }
                assert_eq!(pixel[3], weighted.coverage());
            }
        }
    }

    #[test]
    fn rgba_sample_keeps_faint_coverage_visible() {
        // One of four pixels has alpha 1: the rounded average is 0, but the
        // color path treats the sample as visible, so the RGBA path must too.
        let pixels = [0, 0, 0, 0, 90, 80, 70, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        let source =
            PixelFrameView::new(&pixels, 2, 2, 8, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let geometry = RenderGeometry::calculate(
            2,
            2,
            Viewport {
                width: 1,
                height: 1,
            },
            FitMode::Stretch,
        )
        .unwrap();
        let geometry = RenderGeometry {
            sample_height: 1,
            ..geometry
        };

        assert_eq!(
            sample(&source, &geometry, 0, 0, SamplingFilter::Area),
            Some(Color::rgb(90, 80, 70))
        );
        assert_eq!(
            sample_rgba(&source, &geometry, 0, 0, SamplingFilter::Area, None),
            [90, 80, 70, 1]
        );
        assert_eq!(
            sample_rgba(
                &source,
                &geometry,
                0,
                0,
                SamplingFilter::Area,
                Some(Color::rgb(0, 0, 0))
            ),
            [0, 0, 0, 255]
        );
        assert_eq!(
            sample_rgba(&source, &geometry, 1, 0, SamplingFilter::Area, None),
            [0; 4]
        );
    }

    #[test]
    fn alpha_aware_area_filter_does_not_create_black_fringe() {
        let pixels = [255, 0, 0, 255, 0, 0, 0, 0, 0, 255, 0, 0, 0, 0, 255, 0];
        let source =
            PixelFrameView::new(&pixels, 2, 2, 8, PixelFormat::Rgba8, Duration::ZERO).unwrap();
        let geometry = RenderGeometry::calculate(
            2,
            2,
            Viewport {
                width: 1,
                height: 1,
            },
            FitMode::Stretch,
        )
        .unwrap();

        assert_eq!(
            sample(&source, &geometry, 0, 0, SamplingFilter::Area),
            Some(Color::rgb(255, 0, 0))
        );
        assert_eq!(
            sample_with_alpha_background(
                &source,
                &geometry,
                0,
                0,
                SamplingFilter::Area,
                Some(Color::rgb(255, 255, 255)),
            ),
            Some(Color::rgb(255, 128, 128))
        );
    }
}
