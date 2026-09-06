use crate::cell::Color;
use crate::framebuffer::{PixelFormat, PixelFrameView};
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

pub(crate) fn sample(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    viewport_x: u32,
    viewport_y: u32,
    filter: SamplingFilter,
) -> Option<Color> {
    sample_with_alpha_background(source, geometry, viewport_x, viewport_y, filter, None)
}

pub(crate) fn sample_with_alpha_background(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    viewport_x: u32,
    viewport_y: u32,
    filter: SamplingFilter,
    alpha_background: Option<Color>,
) -> Option<Color> {
    let render_x = i64::from(viewport_x) - geometry.offset_x;
    let render_y = i64::from(viewport_y) - geometry.offset_y;
    if render_x < 0
        || render_y < 0
        || render_x >= i64::from(geometry.sample_width)
        || render_y >= i64::from(geometry.sample_height)
    {
        return None;
    }

    let sample = match filter {
        SamplingFilter::Nearest => sample_nearest(source, geometry, render_x, render_y),
        SamplingFilter::Bilinear => sample_bilinear(source, geometry, render_x, render_y),
        SamplingFilter::Area => sample_area(source, geometry, render_x, render_y),
    }?;
    sample.resolve(alpha_background)
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

fn sample_area(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    render_x: i64,
    render_y: i64,
) -> Option<AlphaSample> {
    if geometry.sample_width >= source.width() || geometry.sample_height >= source.height() {
        return sample_bilinear(source, geometry, render_x, render_y);
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
            let color = read_pixel(
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

fn sample_nearest(
    source: &PixelFrameView<'_>,
    geometry: &RenderGeometry,
    render_x: i64,
    render_y: i64,
) -> Option<AlphaSample> {
    let source_x = u64::try_from(render_x).ok()? * u64::from(source.width())
        / u64::from(geometry.sample_width);
    let source_y = u64::try_from(render_y).ok()? * u64::from(source.height())
        / u64::from(geometry.sample_height);
    read_pixel(
        source,
        u32::try_from(source_x).ok()?,
        u32::try_from(source_y).ok()?,
    )
}

fn sample_bilinear(
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
    let top_left = read_pixel(source, x.low, y.low)?;
    let top_right = read_pixel(source, x.high, y.low)?;
    let bottom_left = read_pixel(source, x.low, y.high)?;
    let bottom_right = read_pixel(source, x.high, y.high)?;
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

fn interpolation_axis(destination: u32, source_len: u32, destination_len: u32) -> AxisSample {
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

fn read_pixel(source: &PixelFrameView<'_>, x: u32, y: u32) -> Option<AlphaSample> {
    let pixel_offset = usize::try_from(y)
        .ok()?
        .checked_mul(source.stride())?
        .checked_add(
            usize::try_from(x)
                .ok()?
                .checked_mul(source.format().bytes_per_pixel())?,
        )?;

    match source.format() {
        PixelFormat::Rgba8 => {
            let pixel = source.pixels().get(pixel_offset..pixel_offset + 4)?;
            let alpha = u64::from(pixel[3]);
            Some(AlphaSample {
                red_alpha: u64::from(pixel[0]) * alpha,
                green_alpha: u64::from(pixel[1]) * alpha,
                blue_alpha: u64::from(pixel[2]) * alpha,
                alpha,
                weight: 1,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::*;
    use crate::framebuffer::PixelFormat;
    use crate::geometry::{FitMode, Viewport};

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
