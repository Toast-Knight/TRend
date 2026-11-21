//! Triangle meshes.

use std::collections::HashMap;
use std::sync::Arc;

use crate::math::{Vec3, add, cross, dot, length, normalize_or, scale, sub};

/// An indexed triangle mesh, the one geometry type the renderer draws.
///
/// Colours are optional and checked in this order: per triangle, per vertex, then the render style's
/// base colour. Normals are per triangle corner (three per triangle) so hard edges and smooth surfaces
/// both survive; when a file has none, [`Mesh::compute_normals`] derives them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// One normal per triangle corner (`3 * triangles.len()`), or empty.
    pub corner_normals: Vec<Vec3>,
    /// One tangent per triangle corner (xyz, w = ±1 handedness of the bitangent), or empty;
    /// needed by normal maps. [`Mesh::ensure_tangents`] derives them from texture coordinates.
    pub corner_tangents: Vec<[f32; 4]>,
    /// One RGBA colour per vertex, or empty.
    pub vertex_colors: Vec<[u8; 4]>,
    /// One RGBA colour per triangle, or empty.
    pub triangle_colors: Vec<[u8; 4]>,
    /// One texture coordinate per vertex (`u` right, `v` down, repeating), or empty.
    pub uvs: Vec<[f32; 2]>,
    /// The material of each triangle, as an index into [`Self::materials`], or empty.
    pub triangle_materials: Vec<u32>,
    pub materials: Vec<Material>,
}

/// A surface's look: a colour and an optional texture.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    /// Base colour (sRGB); multiplies the base colour texture.
    pub color: [u8; 4],
    /// Base colour texture (sRGB, alpha = coverage).
    pub texture: Option<Arc<Texture>>,
    /// Texels with alpha below this are not drawn (cut-outs such as hair and lashes).
    pub alpha_test: Option<u8>,
    /// See-through by alpha: drawn as a fine dither pattern, which needs no sorting.
    pub alpha_blend: bool,
    /// Tangent-space normal map (linear; X right, Y up unless `normal_green_down`).
    pub normal_texture: Option<Arc<Texture>>,
    pub normal_scale: f32,
    /// DirectX-style normal maps (Source, Unreal) store Y pointing down.
    pub normal_green_down: bool,
    /// Light the surface gives off (linear RGB, may exceed 1).
    pub emissive: [f32; 3],
    /// Multiplies `emissive` (sRGB).
    pub emissive_texture: Option<Arc<Texture>>,
    /// 0 = dielectric, 1 = metal.
    pub metallic: f32,
    /// 0 = mirror, 1 = matte.
    pub roughness: f32,
    /// glTF layout: green = roughness, blue = metallic (linear); multiplies the factors.
    pub metallic_roughness_texture: Option<Arc<Texture>>,
    /// Ambient occlusion in red (linear).
    pub occlusion_texture: Option<Arc<Texture>>,
    pub occlusion_strength: f32,
    /// Shown as its colour, without lighting.
    pub unlit: bool,
    /// Textures are sampled without smoothing: crisp texels, as in pixel-art and block games.
    pub pixelated: bool,
}

impl Material {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            color: [255; 4],
            texture: None,
            alpha_test: None,
            alpha_blend: false,
            normal_texture: None,
            normal_scale: 1.0,
            normal_green_down: false,
            emissive: [0.0; 3],
            emissive_texture: None,
            metallic: 0.0,
            roughness: 0.6,
            metallic_roughness_texture: None,
            occlusion_texture: None,
            occlusion_strength: 1.0,
            unlit: false,
            pixelated: false,
        }
    }

    /// Samples one of this material's textures, smoothly or pixelated as the material asks.
    pub fn sample(&self, texture: &Texture, u: f32, v: f32) -> [f32; 4] {
        if self.pixelated {
            texture.sample_nearest(u, v)
        } else {
            texture.sample(u, v)
        }
    }

    /// True when the renderer must look at texture alpha while drawing.
    pub fn uses_alpha(&self) -> bool {
        self.alpha_test.is_some() || self.alpha_blend
    }
}

/// An RGBA8 image, sampled with wrapping and bilinear filtering.
#[derive(Clone, PartialEq, Eq)]
pub struct Texture {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl std::fmt::Debug for Texture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Texture({}x{})", self.width, self.height)
    }
}

impl Texture {
    /// `pixels` must hold `width * height` tightly packed RGBA8 texels.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        (width > 0 && height > 0 && pixels.len() == width as usize * height as usize * 4).then_some(
            Self {
                width,
                height,
                pixels,
            },
        )
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The texel colour at `(u, v)` (0..1 across the image, repeating), bilinear, as 0..255 floats.
    /// The texel under `(u, v)`, without smoothing (repeating).
    pub fn sample_nearest(&self, u: f32, v: f32) -> [f32; 4] {
        if !(u.is_finite() && v.is_finite()) {
            return [255.0; 4];
        }
        let x = (((u - u.floor()) * self.width as f32) as usize).min(self.width as usize - 1);
        let y = (((v - v.floor()) * self.height as f32) as usize).min(self.height as usize - 1);
        let offset = (y * self.width as usize + x) * 4;
        std::array::from_fn(|channel| f32::from(self.pixels[offset + channel]))
    }

    pub fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        if !(u.is_finite() && v.is_finite()) {
            return [255.0; 4];
        }
        let x = (u - u.floor()) * self.width as f32 - 0.5;
        let y = (v - v.floor()) * self.height as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let wrap = |value: f32, size: u32| (value as i64).rem_euclid(i64::from(size)) as usize;
        let (left, right) = (wrap(x0, self.width), wrap(x0 + 1.0, self.width));
        let (top, bottom) = (wrap(y0, self.height), wrap(y0 + 1.0, self.height));
        let texel = |x: usize, y: usize| {
            let offset = (y * self.width as usize + x) * 4;
            &self.pixels[offset..offset + 4]
        };
        let (a, b, c, d) = (
            texel(left, top),
            texel(right, top),
            texel(left, bottom),
            texel(right, bottom),
        );
        std::array::from_fn(|channel| {
            let upper =
                f32::from(a[channel]) + (f32::from(b[channel]) - f32::from(a[channel])) * fx;
            let lower =
                f32::from(c[channel]) + (f32::from(d[channel]) - f32::from(c[channel])) * fx;
            upper + (lower - upper) * fy
        })
    }
}

/// An axis-aligned box around a mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    pub fn center(&self) -> Vec3 {
        scale(add(self.min, self.max), 0.5)
    }

    pub fn size(&self) -> Vec3 {
        sub(self.max, self.min)
    }

    /// Radius of the sphere through the box corners.
    pub fn radius(&self) -> f32 {
        length(self.size()) * 0.5
    }
}

/// Counts that describe a loaded mesh.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MeshStats {
    pub vertices: usize,
    pub triangles: usize,
    pub has_normals: bool,
    pub has_colors: bool,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    pub fn stats(&self) -> MeshStats {
        MeshStats {
            vertices: self.positions.len(),
            triangles: self.triangles.len(),
            has_normals: self.corner_normals.len() == self.triangles.len() * 3,
            has_colors: !self.vertex_colors.is_empty() || !self.triangle_colors.is_empty(),
        }
    }

    /// The box around every vertex that a triangle uses, or `None` for an empty mesh.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut bounds: Option<Bounds> = None;
        for triangle in &self.triangles {
            for &index in triangle {
                let point = self.positions[index as usize];
                if !point.iter().all(|value| value.is_finite()) {
                    continue;
                }
                bounds = Some(match bounds {
                    None => Bounds {
                        min: point,
                        max: point,
                    },
                    Some(Bounds { min, max }) => Bounds {
                        min: [
                            min[0].min(point[0]),
                            min[1].min(point[1]),
                            min[2].min(point[2]),
                        ],
                        max: [
                            max[0].max(point[0]),
                            max[1].max(point[1]),
                            max[2].max(point[2]),
                        ],
                    },
                });
            }
        }
        bounds
    }

    /// The sphere around the bounding box's centre that holds every used vertex: tighter than the
    /// box's own corner radius, so a framed model fills more of the view.
    pub fn bounding_sphere(&self) -> Option<(Vec3, f32)> {
        let center = self.bounds()?.center();
        let mut radius_squared: f32 = 0.0;
        for triangle in &self.triangles {
            for &index in triangle {
                let offset = sub(self.positions[index as usize], center);
                let distance = dot(offset, offset);
                if distance.is_finite() {
                    radius_squared = radius_squared.max(distance);
                }
            }
        }
        Some((center, radius_squared.sqrt()))
    }

    /// Drops triangles that reference a missing vertex or repeat a vertex, and colour or normal
    /// arrays whose length does not match. Loaders call it; call it after editing a mesh by hand.
    pub fn sanitize(&mut self) {
        let vertex_count = self.positions.len();
        let normals = self.corner_normals.len() == self.triangles.len() * 3;
        let tangents = self.corner_tangents.len() == self.triangles.len() * 3;
        let colors = self.triangle_colors.len() == self.triangles.len();
        let materials = self.triangle_materials.len() == self.triangles.len();
        let mut kept_normals = Vec::new();
        let mut kept_tangents = Vec::new();
        let mut kept_colors = Vec::new();
        let mut kept_materials = Vec::new();
        let mut kept = Vec::with_capacity(self.triangles.len());
        for (index, triangle) in self.triangles.iter().enumerate() {
            let [a, b, c] = *triangle;
            if [a, b, c]
                .iter()
                .any(|&vertex| vertex as usize >= vertex_count)
                || a == b
                || b == c
                || a == c
            {
                continue;
            }
            kept.push(*triangle);
            if normals {
                kept_normals.extend_from_slice(&self.corner_normals[index * 3..index * 3 + 3]);
            }
            if tangents {
                kept_tangents.extend_from_slice(&self.corner_tangents[index * 3..index * 3 + 3]);
            }
            if colors {
                kept_colors.push(self.triangle_colors[index]);
            }
            if materials {
                kept_materials.push(self.triangle_materials[index]);
            }
        }
        self.triangles = kept;
        self.corner_normals = kept_normals;
        self.corner_tangents = kept_tangents;
        self.triangle_colors = kept_colors;
        self.triangle_materials = kept_materials;
        if self.vertex_colors.len() != vertex_count {
            self.vertex_colors.clear();
        }
        if self.uvs.len() != vertex_count {
            self.uvs.clear();
        }
    }

    /// Merges vertices at exactly the same position. Formats such as STL repeat every corner, which
    /// would make every edge look hard; welding lets [`Self::compute_normals`] smooth them.
    pub fn weld(&mut self) {
        self.sanitize();
        let mut remap = Vec::with_capacity(self.positions.len());
        let mut unique: HashMap<[u32; 3], u32> = HashMap::with_capacity(self.positions.len());
        let mut positions = Vec::new();
        let mut colors = Vec::new();
        let keep_colors = self.vertex_colors.len() == self.positions.len();
        let keep_uvs = self.uvs.len() == self.positions.len();
        let mut uvs = Vec::new();
        let mut unique_uv: HashMap<[u32; 5], u32> = HashMap::new();
        for (index, point) in self.positions.iter().enumerate() {
            // +0.0 and -0.0 are the same point.
            let key = point.map(|value| (value + 0.0).to_bits());
            let next = positions.len() as u32;
            let add = || {
                positions.push(*point);
                if keep_colors {
                    colors.push(self.vertex_colors[index]);
                }
                if keep_uvs {
                    uvs.push(self.uvs[index]);
                }
                next
            };
            // Corners with different texture coordinates are different vertices (texture seams).
            let target = if keep_uvs {
                let [u, v] = self.uvs[index].map(|value| (value + 0.0).to_bits());
                *unique_uv
                    .entry([key[0], key[1], key[2], u, v])
                    .or_insert_with(add)
            } else {
                *unique.entry(key).or_insert_with(add)
            };
            remap.push(target);
        }
        for triangle in &mut self.triangles {
            *triangle = triangle.map(|index| remap[index as usize]);
        }
        self.positions = positions;
        self.vertex_colors = colors;
        self.uvs = uvs;
        self.sanitize();
    }

    /// Fills [`Self::corner_normals`] from the geometry. Faces meeting at a vertex are smoothed
    /// together when their normals are within `crease_degrees` of each other, so a sphere is smooth
    /// and a cube keeps its edges. Each face counts by its angle at the vertex, so how a surface
    /// happens to be split into triangles does not bend its normals.
    pub fn compute_normals(&mut self, crease_degrees: f32) {
        let unit: Vec<Vec3> = self
            .triangles
            .iter()
            .map(|&[a, b, c]| {
                let [a, b, c] = [a, b, c].map(|index| self.positions[index as usize]);
                normalize_or(cross(sub(b, a), sub(c, a)), [0.0, 0.0, 0.0])
            })
            .collect();
        let angles: Vec<[f32; 3]> = self
            .triangles
            .iter()
            .map(|triangle| {
                let points = triangle.map(|index| self.positions[index as usize]);
                std::array::from_fn(|corner| {
                    let here = points[corner];
                    let first = normalize_or(sub(points[(corner + 1) % 3], here), [0.0; 3]);
                    let second = normalize_or(sub(points[(corner + 2) % 3], here), [0.0; 3]);
                    dot(first, second).clamp(-1.0, 1.0).acos()
                })
            })
            .collect();

        // Faces around each vertex, as a compressed adjacency list.
        let mut starts = vec![0_u32; self.positions.len() + 1];
        for triangle in &self.triangles {
            for &vertex in triangle {
                starts[vertex as usize + 1] += 1;
            }
        }
        for index in 1..starts.len() {
            starts[index] += starts[index - 1];
        }
        let mut faces = vec![0_u32; self.triangles.len() * 3];
        let mut fill = starts.clone();
        for (face, triangle) in self.triangles.iter().enumerate() {
            for &vertex in triangle {
                faces[fill[vertex as usize] as usize] = face as u32;
                fill[vertex as usize] += 1;
            }
        }

        let threshold = crease_degrees.clamp(0.0, 180.0).to_radians().cos();
        let mut normals = Vec::with_capacity(self.triangles.len() * 3);
        for (face, triangle) in self.triangles.iter().enumerate() {
            for &vertex in triangle {
                let around =
                    &faces[starts[vertex as usize] as usize..starts[vertex as usize + 1] as usize];
                let mut sum = [0.0; 3];
                for &other in around {
                    let other = other as usize;
                    if dot(unit[other], unit[face]) >= threshold {
                        let corner = self.triangles[other]
                            .iter()
                            .position(|&index| index == vertex)
                            .unwrap_or(0);
                        sum = add(sum, scale(unit[other], angles[other][corner]));
                    }
                }
                normals.push(normalize_or(sum, unit[face]));
            }
        }
        self.corner_normals = normals;
    }

    /// Fills [`Self::corner_tangents`] from texture coordinates and corner normals, if the mesh has
    /// texture coordinates and no tangents yet. Tangents are accumulated per vertex (so they are
    /// smooth across triangles) and made perpendicular to each corner's normal.
    pub fn ensure_tangents(&mut self) {
        if self.corner_tangents.len() == self.triangles.len() * 3
            || self.uvs.len() != self.positions.len()
        {
            return;
        }
        self.ensure_normals();
        let mut tangents = vec![[0.0_f32; 3]; self.positions.len()];
        let mut bitangents = vec![[0.0_f32; 3]; self.positions.len()];
        for &[a, b, c] in &self.triangles {
            let [pa, pb, pc] = [a, b, c].map(|index| self.positions[index as usize]);
            let [ua, ub, uc] = [a, b, c].map(|index| self.uvs[index as usize]);
            let (e1, e2) = (sub(pb, pa), sub(pc, pa));
            let (du1, dv1) = (ub[0] - ua[0], ub[1] - ua[1]);
            let (du2, dv2) = (uc[0] - ua[0], uc[1] - ua[1]);
            let determinant = du1 * dv2 - du2 * dv1;
            if determinant.abs() < 1e-12 || !determinant.is_finite() {
                continue;
            }
            let inverse = 1.0 / determinant;
            let tangent = scale(sub(scale(e1, dv2), scale(e2, dv1)), inverse);
            let bitangent = scale(sub(scale(e2, du1), scale(e1, du2)), inverse);
            for index in [a, b, c] {
                tangents[index as usize] = add(tangents[index as usize], tangent);
                bitangents[index as usize] = add(bitangents[index as usize], bitangent);
            }
        }
        let mut corner_tangents = Vec::with_capacity(self.triangles.len() * 3);
        for (triangle, corners) in self.triangles.iter().enumerate() {
            for (corner, &vertex) in corners.iter().enumerate() {
                let normal = self.corner_normals[triangle * 3 + corner];
                let raw = tangents[vertex as usize];
                // Gram-Schmidt: remove the normal's share; any perpendicular axis if degenerate.
                let mut tangent = normalize_or(sub(raw, scale(normal, dot(normal, raw))), [0.0; 3]);
                if tangent == [0.0; 3] {
                    let axis = if normal[0].abs() < 0.9 {
                        [1.0, 0.0, 0.0]
                    } else {
                        [0.0, 1.0, 0.0]
                    };
                    tangent = normalize_or(cross(axis, normal), [1.0, 0.0, 0.0]);
                }
                // glTF convention: the bitangent cross(normal, tangent) × w points to the top of
                // the picture (decreasing v, as v points down), where a normal map's green goes.
                let handedness = if dot(cross(normal, tangent), bitangents[vertex as usize]) > 0.0 {
                    -1.0
                } else {
                    1.0
                };
                corner_tangents.push([tangent[0], tangent[1], tangent[2], handedness]);
            }
        }
        self.corner_tangents = corner_tangents;
    }

    /// Computes normals if the mesh has none (or the wrong number).
    pub fn ensure_normals(&mut self) {
        if self.corner_normals.len() != self.triangles.len() * 3 {
            self.compute_normals(DEFAULT_CREASE_DEGREES);
        }
    }
}

/// Faces closer than this are smoothed together by [`Mesh::ensure_normals`].
pub const DEFAULT_CREASE_DEGREES: f32 = 50.0;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A unit cube, 12 triangles, every corner shared.
    pub(crate) fn cube() -> Mesh {
        let positions = vec![
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        let quads = [
            [4, 5, 6, 7],
            [1, 0, 3, 2],
            [5, 1, 2, 6],
            [0, 4, 7, 3],
            [7, 6, 2, 3],
            [0, 1, 5, 4],
        ];
        let mut triangles = Vec::new();
        for [a, b, c, d] in quads {
            triangles.push([a, b, c]);
            triangles.push([a, c, d]);
        }
        Mesh {
            positions,
            triangles,
            ..Mesh::default()
        }
    }

    #[test]
    fn bounds_center_and_radius() {
        let bounds = cube().bounds().unwrap();
        assert_eq!(bounds.center(), [0.0, 0.0, 0.0]);
        assert!((bounds.radius() - 3.0_f32.sqrt()).abs() < 1e-6);
        assert!(Mesh::default().bounds().is_none());
    }

    #[test]
    fn crease_keeps_cube_edges_hard_and_smooths_when_wide() {
        let mut mesh = cube();
        mesh.compute_normals(DEFAULT_CREASE_DEGREES);
        // Every corner normal of a face equals the face's axis.
        for (face, normals) in mesh.corner_normals.chunks_exact(3).enumerate() {
            for normal in normals {
                assert!(
                    normal.iter().filter(|value| value.abs() > 0.999).count() == 1,
                    "face {face}: {normal:?}"
                );
            }
        }
        mesh.compute_normals(180.0);
        // Fully smoothed: corners point diagonally outward.
        let corner = mesh.corner_normals[0];
        assert!(
            corner
                .iter()
                .all(|value| (value.abs() - 1.0 / 3.0_f32.sqrt()).abs() < 1e-5)
        );
    }

    #[test]
    fn weld_merges_repeated_corners_and_sanitize_drops_bad_triangles() {
        let mut mesh = Mesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, -0.0],
            ],
            triangles: vec![[0, 1, 2], [3, 5, 4], [0, 0, 1], [0, 1, 99]],
            ..Mesh::default()
        };
        mesh.weld();
        assert_eq!(mesh.positions.len(), 4);
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [1, 3, 2]]);
    }
}
