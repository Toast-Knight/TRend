//! Environments ("world boxes"): the world around the model, as a panorama of light.
//!
//! An environment lights the model (soft light from every direction) and is what shiny surfaces
//! reflect; it can also be shown behind the model as a sky box. It is stored as equirectangular
//! (latitude/longitude) images in linear light, pre-blurred into levels so rough surfaces reflect a
//! blurrier world, plus a very blurred irradiance map for diffuse light.

use std::f32::consts::{PI, TAU};

use crate::color::srgb_float_to_linear;
use crate::math::{Vec3, dot, normalize_or};
use crate::mesh::Texture;

/// Built-in environments, drawn procedurally (no image files needed).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum EnvironmentPreset {
    /// Grey studio with soft boxes: even, flattering light.
    Studio,
    /// Clear day: blue sky, bright sun, earth below.
    Sky,
    /// Low orange sun under a purple sky.
    Sunset,
    /// Dark blue night with stars and a moon.
    Night,
    /// Dark room ringed with magenta and cyan light strips.
    Neon,
    /// Soft white overcast sky.
    Overcast,
}

impl EnvironmentPreset {
    pub const ALL: [Self; 6] = [
        Self::Studio,
        Self::Sky,
        Self::Sunset,
        Self::Night,
        Self::Neon,
        Self::Overcast,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Studio => "Studio",
            Self::Sky => "Sky",
            Self::Sunset => "Sunset",
            Self::Night => "Night",
            Self::Neon => "Neon",
            Self::Overcast => "Overcast",
        }
    }
}

/// One equirectangular image of linear RGB.
#[derive(Clone, Debug, PartialEq)]
struct LatLong {
    width: usize,
    height: usize,
    texels: Vec<[f32; 3]>,
}

/// Image coordinates of a direction in the Y-up frame: u around (0 at -Z), v down from the top.
fn direction_to_uv(direction: Vec3) -> (f32, f32) {
    let u = 0.5 + direction[0].atan2(-direction[2]) / TAU;
    let v = direction[1].clamp(-1.0, 1.0).acos() / PI;
    (u, v)
}

fn uv_to_direction(u: f32, v: f32) -> Vec3 {
    let phi = (u - 0.5) * TAU;
    let theta = v * PI;
    [
        theta.sin() * phi.sin(),
        theta.cos(),
        -theta.sin() * phi.cos(),
    ]
}

impl LatLong {
    fn from_fn(width: usize, height: usize, light: impl Fn(Vec3) -> [f32; 3]) -> Self {
        let mut texels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let u = (x as f32 + 0.5) / width as f32;
                let v = (y as f32 + 0.5) / height as f32;
                texels.push(light(uv_to_direction(u, v)));
            }
        }
        Self {
            width,
            height,
            texels,
        }
    }

    fn sample(&self, direction: Vec3) -> [f32; 3] {
        let (u, v) = direction_to_uv(direction);
        let x = u * self.width as f32 - 0.5;
        let y = (v * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let column = |x: f32| (x as i64).rem_euclid(self.width as i64) as usize;
        let (left, right) = (column(x0), column(x0 + 1.0));
        let (top, bottom) = (y0 as usize, (y0 as usize + 1).min(self.height - 1));
        let texel = |x: usize, y: usize| self.texels[y * self.width + x];
        let (a, b, c, d) = (
            texel(left, top),
            texel(right, top),
            texel(left, bottom),
            texel(right, bottom),
        );
        std::array::from_fn(|channel| {
            let upper = a[channel] + (b[channel] - a[channel]) * fx;
            let lower = c[channel] + (d[channel] - c[channel]) * fx;
            upper + (lower - upper) * fy
        })
    }

    /// Half the size, each texel the average of four.
    fn downsample(&self) -> Self {
        let (width, height) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut texels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let mut sum = [0.0; 3];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let source = self.texels[((y * 2 + dy).min(self.height - 1)) * self.width
                        + (x * 2 + dx).min(self.width - 1)];
                    for channel in 0..3 {
                        sum[channel] += source[channel] * 0.25;
                    }
                }
                texels.push(sum);
            }
        }
        Self {
            width,
            height,
            texels,
        }
    }

    /// Box blur of `radius` texels (widened toward the poles, where texels are narrower), in
    /// two passes; horizontal wraps around.
    fn blur(&self, radius: usize) -> Self {
        let mut horizontal = self.texels.clone();
        for y in 0..self.height {
            let latitude = ((y as f32 + 0.5) / self.height as f32 * PI).sin().max(0.15);
            let reach = ((radius as f32 / latitude).round() as usize).min(self.width / 2);
            let count = (2 * reach + 1) as f32;
            for x in 0..self.width {
                let mut sum = [0.0; 3];
                for offset in 0..=2 * reach {
                    let source = self.texels
                        [y * self.width + (x + self.width + offset - reach) % self.width];
                    for channel in 0..3 {
                        sum[channel] += source[channel];
                    }
                }
                horizontal[y * self.width + x] = sum.map(|value| value / count);
            }
        }
        let mut texels = horizontal.clone();
        for y in 0..self.height {
            for x in 0..self.width {
                let mut sum = [0.0; 3];
                let mut count = 0.0;
                for source_y in y.saturating_sub(radius)..=(y + radius).min(self.height - 1) {
                    let source = horizontal[source_y * self.width + x];
                    for channel in 0..3 {
                        sum[channel] += source[channel];
                    }
                    count += 1.0;
                }
                texels[y * self.width + x] = sum.map(|value| value / count);
            }
        }
        Self {
            width: self.width,
            height: self.height,
            texels,
        }
    }

    /// Cosine-weighted average of the whole sphere around each direction (diffuse light).
    fn irradiance(&self, width: usize, height: usize) -> Self {
        let mut small = self.clone();
        while small.width > 32 {
            small = small.downsample();
        }
        // Solid angle of each source texel.
        let mut sources = Vec::with_capacity(small.texels.len());
        for y in 0..small.height {
            let v = (y as f32 + 0.5) / small.height as f32;
            let weight = (v * PI).sin();
            for x in 0..small.width {
                let u = (x as f32 + 0.5) / small.width as f32;
                sources.push((
                    uv_to_direction(u, v),
                    weight,
                    small.texels[y * small.width + x],
                ));
            }
        }
        Self::from_fn(width, height, |normal| {
            let mut sum = [0.0; 3];
            let mut total = 0.0;
            for &(direction, weight, light) in &sources {
                let cosine = dot(normal, direction);
                if cosine <= 0.0 {
                    continue;
                }
                let weight = weight * cosine;
                for channel in 0..3 {
                    sum[channel] += light[channel] * weight;
                }
                total += weight;
            }
            if total > 0.0 {
                sum.map(|value| value / total)
            } else {
                [0.0; 3]
            }
        })
    }
}

/// The world around the model: what lights it and what it reflects.
#[derive(Clone, Debug, PartialEq)]
pub struct Environment {
    name: String,
    /// Level 0 is sharp; each next level is half the size and blurrier.
    levels: Vec<LatLong>,
    irradiance: LatLong,
}

/// Prefiltered levels: reflections of roughness 0 to 1 spread over these.
const LEVELS: usize = 6;

impl Environment {
    fn build(name: impl Into<String>, mut base: LatLong) -> Self {
        while base.width > 1024 {
            base = base.downsample();
        }
        let mut levels = vec![base];
        for level in 1..LEVELS {
            let previous = levels.last().expect("at least one level");
            let next = if previous.width > 16 {
                previous.downsample()
            } else {
                previous.clone()
            };
            levels.push(next.blur(level.min(3)));
        }
        let irradiance = levels[0].irradiance(32, 16);
        Self {
            name: name.into(),
            levels,
            irradiance,
        }
    }

    /// An environment from linear RGB texels of a `width` x `height` equirectangular image.
    pub fn from_linear(
        name: impl Into<String>,
        width: usize,
        height: usize,
        texels: Vec<[f32; 3]>,
    ) -> Option<Self> {
        (width >= 4 && height >= 2 && texels.len() == width * height).then(|| {
            Self::build(
                name,
                LatLong {
                    width,
                    height,
                    texels,
                },
            )
        })
    }

    /// An environment from an equirectangular sRGB picture (PNG, JPEG...). Pictures hold no light
    /// brighter than white, so their brightest parts are lifted a little to make reflections read.
    pub fn from_texture(name: impl Into<String>, texture: &Texture) -> Option<Self> {
        let texels = texture
            .pixels()
            .chunks_exact(4)
            .map(|texel| {
                let linear =
                    [0, 1, 2].map(|channel| srgb_float_to_linear(f32::from(texel[channel])));
                let peak = linear[0].max(linear[1]).max(linear[2]);
                let lift = 1.0 + 3.0 * (peak - 0.85).max(0.0) / 0.15;
                linear.map(|value| value * lift)
            })
            .collect();
        Self::from_linear(
            name,
            texture.width() as usize,
            texture.height() as usize,
            texels,
        )
    }

    /// An environment from a Radiance `.hdr` (RGBE) panorama, the usual format of HDRI downloads.
    pub fn from_hdr(name: impl Into<String>, bytes: &[u8]) -> Option<Self> {
        let (width, height, texels) = decode_hdr(bytes)?;
        Self::from_linear(name, width, height, texels)
    }

    /// A built-in environment.
    pub fn preset(preset: EnvironmentPreset) -> Self {
        let base = match preset {
            EnvironmentPreset::Studio => LatLong::from_fn(512, 256, studio),
            EnvironmentPreset::Sky => LatLong::from_fn(512, 256, sky),
            EnvironmentPreset::Sunset => LatLong::from_fn(512, 256, sunset),
            EnvironmentPreset::Night => LatLong::from_fn(512, 256, night),
            EnvironmentPreset::Neon => LatLong::from_fn(512, 256, neon),
            EnvironmentPreset::Overcast => LatLong::from_fn(512, 256, overcast),
        };
        Self::build(preset.label(), base)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Light arriving from `direction` (Y-up frame), blurred for a surface of `roughness` 0..1.
    pub fn radiance(&self, direction: Vec3, roughness: f32) -> [f32; 3] {
        let direction = normalize_or(direction, [0.0, 1.0, 0.0]);
        let level = roughness.clamp(0.0, 1.0).sqrt() * (self.levels.len() - 1) as f32;
        let lower = level.floor() as usize;
        let upper = (lower + 1).min(self.levels.len() - 1);
        let fraction = level - lower as f32;
        let a = self.levels[lower].sample(direction);
        if fraction <= 0.0 || upper == lower {
            return a;
        }
        let b = self.levels[upper].sample(direction);
        std::array::from_fn(|channel| a[channel] + (b[channel] - a[channel]) * fraction)
    }

    /// Diffuse light falling on a surface facing `normal` (Y-up frame).
    pub fn irradiance(&self, normal: Vec3) -> [f32; 3] {
        self.irradiance
            .sample(normalize_or(normal, [0.0, 1.0, 0.0]))
    }

    /// Average light over the whole sphere, as one luminance number.
    pub fn average_luminance(&self) -> f32 {
        let texels = &self.irradiance.texels;
        let sum: f32 = texels
            .iter()
            .map(|texel| 0.2126 * texel[0] + 0.7152 * texel[1] + 0.0722 * texel[2])
            .sum();
        sum / texels.len().max(1) as f32
    }
}

// --- Built-in environments: light arriving from a direction (Y-up), linear RGB. -------------

fn elevation(direction: Vec3) -> f32 {
    direction[1].clamp(-1.0, 1.0).asin()
}

/// How close `direction` is to the point at `azimuth`/`elevation` (degrees), as an angle cosine.
fn toward(direction: Vec3, azimuth: f32, elevation: f32) -> f32 {
    let (azimuth, elevation) = (azimuth.to_radians(), elevation.to_radians());
    let target = [
        elevation.cos() * azimuth.sin(),
        elevation.sin(),
        -elevation.cos() * azimuth.cos(),
    ];
    dot(direction, target)
}

/// A soft, bright patch: full strength inside `radius` degrees, fading over `soft` more.
fn patch(direction: Vec3, azimuth: f32, elevation: f32, radius: f32, soft: f32) -> f32 {
    let angle = toward(direction, azimuth, elevation)
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees();
    (1.0 - (angle - radius).max(0.0) / soft.max(0.01)).clamp(0.0, 1.0)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|channel| a[channel] + (b[channel] - a[channel]) * t)
}

fn add_light(base: [f32; 3], color: [f32; 3], amount: f32) -> [f32; 3] {
    std::array::from_fn(|channel| base[channel] + color[channel] * amount)
}

fn studio(direction: Vec3) -> [f32; 3] {
    let height = elevation(direction) / (PI / 2.0);
    let mut light = if height >= 0.0 {
        mix([0.16, 0.16, 0.17], [0.32, 0.32, 0.34], height)
    } else {
        mix([0.12, 0.11, 0.10], [0.05, 0.05, 0.05], -height * 3.0)
    };
    light = add_light(
        light,
        [1.0, 0.97, 0.92],
        5.0 * patch(direction, 45.0, 30.0, 12.0, 8.0),
    );
    light = add_light(
        light,
        [0.9, 0.95, 1.0],
        2.0 * patch(direction, -70.0, 15.0, 15.0, 10.0),
    );
    light = add_light(
        light,
        [1.0, 1.0, 1.0],
        2.5 * patch(direction, 0.0, 80.0, 18.0, 12.0),
    );
    add_light(
        light,
        [1.0, 1.0, 1.0],
        3.0 * patch(direction, 170.0, 20.0, 8.0, 6.0),
    )
}

fn sky(direction: Vec3) -> [f32; 3] {
    let height = elevation(direction) / (PI / 2.0);
    let mut light = if height >= 0.0 {
        mix([0.55, 0.7, 0.95], [0.12, 0.3, 0.8], height.powf(0.6))
    } else {
        mix(
            [0.32, 0.3, 0.25],
            [0.12, 0.11, 0.08],
            (-height * 4.0).min(1.0),
        )
    };
    let sun = toward(direction, 120.0, 40.0);
    light = add_light(
        light,
        [1.0, 0.95, 0.85],
        60.0 * patch(direction, 120.0, 40.0, 1.5, 1.0),
    );
    add_light(light, [1.0, 0.9, 0.7], 0.8 * sun.max(0.0).powi(16))
}

fn sunset(direction: Vec3) -> [f32; 3] {
    let height = elevation(direction) / (PI / 2.0);
    let mut light = if height >= 0.0 {
        let base = mix([1.1, 0.45, 0.18], [0.25, 0.12, 0.35], height.powf(0.45));
        mix(base, [0.05, 0.06, 0.16], (height - 0.4).max(0.0) * 1.6)
    } else {
        mix(
            [0.15, 0.08, 0.06],
            [0.03, 0.025, 0.03],
            (-height * 4.0).min(1.0),
        )
    };
    let glow = toward(direction, -100.0, 3.0).max(0.0);
    light = add_light(light, [1.0, 0.5, 0.2], 1.2 * glow.powi(8));
    add_light(
        light,
        [1.0, 0.6, 0.3],
        30.0 * patch(direction, -100.0, 3.0, 2.0, 1.0),
    )
}

/// A cheap repeatable hash of a direction cell, for stars.
fn star(direction: Vec3) -> f32 {
    let cell = direction.map(|value| (value * 180.0).floor() as i64);
    let mut hash = (cell[0] * 73_856_093) ^ (cell[1] * 19_349_663) ^ (cell[2] * 83_492_791);
    hash = hash.wrapping_mul(0x2545_F491).rem_euclid(10_000);
    if hash < 12 { 1.0 + hash as f32 } else { 0.0 }
}

fn night(direction: Vec3) -> [f32; 3] {
    let height = elevation(direction) / (PI / 2.0);
    let mut light = if height >= 0.0 {
        mix([0.03, 0.04, 0.09], [0.006, 0.008, 0.025], height.powf(0.5))
    } else {
        [0.01, 0.01, 0.015]
    };
    if height > 0.02 {
        light = add_light(light, [0.9, 0.95, 1.0], 0.6 * star(direction));
    }
    add_light(
        light,
        [0.85, 0.9, 1.0],
        6.0 * patch(direction, 60.0, 35.0, 2.0, 1.5),
    )
}

fn neon(direction: Vec3) -> [f32; 3] {
    let height = elevation(direction).to_degrees();
    let mut light = [0.012, 0.012, 0.02];
    let band = |center: f32, width: f32| (1.0 - ((height - center).abs() / width)).clamp(0.0, 1.0);
    light = add_light(light, [1.0, 0.1, 0.8], 4.0 * band(8.0, 3.0));
    light = add_light(light, [0.1, 0.9, 1.0], 4.0 * band(32.0, 2.5));
    // Pillars of light all the way round.
    let azimuth = direction[0].atan2(-direction[2]).to_degrees();
    let pillar = (1.0 - ((azimuth.rem_euclid(60.0)) - 30.0).abs() / 2.0).clamp(0.0, 1.0);
    if height > -5.0 && height < 45.0 {
        light = add_light(light, [0.6, 0.3, 1.0], 2.0 * pillar);
    }
    light
}

fn overcast(direction: Vec3) -> [f32; 3] {
    let height = elevation(direction) / (PI / 2.0);
    if height >= 0.0 {
        mix([0.75, 0.77, 0.8], [1.0, 1.0, 1.02], height)
    } else {
        mix(
            [0.3, 0.3, 0.3],
            [0.18, 0.18, 0.18],
            (-height * 3.0).min(1.0),
        )
    }
}

/// Decodes a Radiance RGBE (`.hdr`) image: flat or run-length encoded scanlines.
fn decode_hdr(bytes: &[u8]) -> Option<(usize, usize, Vec<[f32; 3]>)> {
    if !(bytes.starts_with(b"#?RADIANCE") || bytes.starts_with(b"#?RGBE")) {
        return None;
    }
    // Header lines end at an empty line; then the resolution line.
    let mut position = 0;
    let line = |position: &mut usize| -> Option<String> {
        let start = *position;
        let end = bytes.get(start..)?.iter().position(|&byte| byte == b'\n')? + start;
        *position = end + 1;
        Some(
            String::from_utf8_lossy(&bytes[start..end])
                .trim()
                .to_owned(),
        )
    };
    loop {
        let header = line(&mut position)?;
        if header.is_empty() {
            break;
        }
        if header.starts_with("FORMAT=") && header != "FORMAT=32-bit_rle_rgbe" {
            return None;
        }
    }
    let resolution = line(&mut position)?;
    let parts: Vec<&str> = resolution.split_whitespace().collect();
    let (height, width) = match parts.as_slice() {
        ["-Y", height, "+X", width] => {
            (height.parse::<usize>().ok()?, width.parse::<usize>().ok()?)
        }
        _ => return None,
    };
    if width == 0 || height == 0 || width > 16_384 || height > 8_192 {
        return None;
    }
    let mut texels = Vec::with_capacity(width * height);
    let mut scanline = vec![[0_u8; 4]; width];
    for _ in 0..height {
        let header = bytes.get(position..position + 4)?;
        if (8..32_768).contains(&width) && header[0] == 2 && header[1] == 2 && header[2] & 0x80 == 0
        {
            // New run-length encoding: each channel separately.
            position += 4;
            for channel in 0..4 {
                let mut x = 0;
                while x < width {
                    let count = *bytes.get(position)?;
                    position += 1;
                    if count > 128 {
                        let run = usize::from(count - 128);
                        let value = *bytes.get(position)?;
                        position += 1;
                        for texel in scanline.get_mut(x..x + run)? {
                            texel[channel] = value;
                        }
                        x += run;
                    } else {
                        let run = usize::from(count);
                        let values = bytes.get(position..position + run)?;
                        position += run;
                        for (texel, &value) in scanline.get_mut(x..x + run)?.iter_mut().zip(values)
                        {
                            texel[channel] = value;
                        }
                        x += run;
                    }
                }
            }
        } else {
            // Flat RGBE.
            for texel in scanline.iter_mut() {
                texel.copy_from_slice(bytes.get(position..position + 4)?);
                position += 4;
            }
        }
        texels.extend(scanline.iter().map(|&[r, g, b, e]| {
            if e == 0 {
                [0.0; 3]
            } else {
                let scale = 2.0_f32.powi(i32::from(e) - 136);
                [
                    f32::from(r) * scale,
                    f32::from(g) * scale,
                    f32::from(b) * scale,
                ]
            }
        }));
    }
    Some((width, height, texels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directions_and_image_coordinates_agree() {
        for direction in [
            [0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.6, -0.48, 0.64],
        ] {
            let (u, v) = direction_to_uv(direction);
            let back = uv_to_direction(u, v);
            for axis in 0..3 {
                assert!(
                    (back[axis] - direction[axis]).abs() < 1e-4,
                    "{direction:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn presets_light_from_above_and_blur_with_roughness() {
        for preset in EnvironmentPreset::ALL {
            let environment = Environment::preset(preset);
            assert_eq!(environment.name(), preset.label());
            assert!(environment.average_luminance() > 0.0, "{preset:?}");
            let up = environment.irradiance([0.0, 1.0, 0.0]);
            assert!(up.iter().all(|value| value.is_finite() && *value >= 0.0));
        }
        let sky = Environment::preset(EnvironmentPreset::Sky);
        // The sun is far brighter seen sharply than through a rough surface.
        let sun = [
            40_f32.to_radians().cos() * 120_f32.to_radians().sin(),
            40_f32.to_radians().sin(),
            -40_f32.to_radians().cos() * 120_f32.to_radians().cos(),
        ];
        let sharp = sky.radiance(sun, 0.0)[0];
        let rough = sky.radiance(sun, 1.0)[0];
        assert!(sharp > 10.0 && rough < sharp / 4.0, "{sharp} {rough}");
        // Sky above is brighter than ground below.
        assert!(sky.irradiance([0.0, 1.0, 0.0])[2] > sky.irradiance([0.0, -1.0, 0.0])[2]);
    }

    #[test]
    fn hdr_files_decode_flat_and_run_length() {
        // 8x2: row 0 run-length (all texels (128,64,32,129) -> 1.0, 0.5, 0.25), row 1 flat.
        let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 8\n".to_vec();
        bytes.extend_from_slice(&[2, 2, 0, 8]);
        for value in [128_u8, 64, 32, 129] {
            bytes.extend_from_slice(&[128 + 8, value]);
        }
        for _ in 0..8 {
            bytes.extend_from_slice(&[64, 64, 64, 129]);
        }
        let (width, height, texels) = decode_hdr(&bytes).unwrap();
        assert_eq!((width, height), (8, 2));
        assert_eq!(texels[0], [1.0, 0.5, 0.25]);
        assert_eq!(texels[15], [0.5, 0.5, 0.5]);
        assert!(Environment::from_hdr("test", &bytes).is_some());
        assert!(decode_hdr(b"not hdr").is_none());
        assert!(decode_hdr(&bytes[..bytes.len() - 3]).is_none());
    }

    #[test]
    fn pictures_become_environments() {
        let texture = Texture::new(8, 4, [200_u8, 220, 255, 255].repeat(32)).unwrap();
        let environment = Environment::from_texture("picture", &texture).unwrap();
        let light = environment.radiance([0.0, 0.0, -1.0], 0.0);
        assert!(light[2] > light[0]);
    }
}
