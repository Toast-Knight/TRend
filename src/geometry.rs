use core::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FitMode {
    Contain,
    Cover,
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

    pub fn calculate_transformed(
        source_width: u32,
        source_height: u32,
        viewport: Viewport,
        fit: FitMode,
        transform: RenderTransform,
    ) -> Result<Self, GeometryError> {
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

        Ok(Self {
            viewport,
            sample_width: u32::try_from(sample_width).map_err(|_| GeometryError::SizeOverflow)?,
            sample_height: u32::try_from(sample_height).map_err(|_| GeometryError::SizeOverflow)?,
            offset_x,
            offset_y,
        })
    }
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
