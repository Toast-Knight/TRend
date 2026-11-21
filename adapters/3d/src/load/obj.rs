//! Wavefront OBJ, with MTL material libraries (diffuse colours and opacity, and with a texture
//! loader: diffuse, emissive, normal, roughness and metallic maps) and the common `v x y z r g b`
//! vertex-colour extension. Texture coordinates (`vt`) are kept; corners that share a position but
//! not a texture coordinate become separate vertices.

use std::collections::HashMap;
use std::sync::Arc;

use super::{ModelError, ModelLimits, check_limit, fan, parse_error};
use crate::color::srgb_float_to_linear;
use crate::math::Vec3;
use crate::mesh::{Material, Mesh, Texture};

/// Faces without a material, when other faces have one.
const DEFAULT_MATERIAL: [u8; 4] = [200, 200, 200, 255];

/// No texture coordinate on this corner.
const NO_UV: u32 = u32::MAX;

/// Parses OBJ `text`. `mtl` is asked for each `mtllib` file name and returns its text, or `None`
/// to skip it. Material colours become per-triangle colours; textures are not loaded (see
/// [`parse_obj_with`]). The mesh is not sanitized and has no computed normals yet; [`super::load`]
/// does that.
pub fn parse_obj(
    text: &str,
    mtl: impl FnMut(&str) -> Option<String>,
    limits: ModelLimits,
) -> Result<Mesh, ModelError> {
    parse_obj_with(text, mtl, |_| None, limits)
}

/// Like [`parse_obj`], with `texture` loading the pictures MTL files name (`map_Kd`, `map_Ke`,
/// `map_Bump`/`norm`, `map_Pr`, `map_Pm`, `map_d`). When a material uses a texture or other
/// physically based property, the mesh gets [`Mesh::materials`] instead of per-triangle colours.
pub fn parse_obj_with(
    text: &str,
    mut mtl: impl FnMut(&str) -> Option<String>,
    mut texture: impl FnMut(&str) -> Option<Texture>,
    limits: ModelLimits,
) -> Result<Mesh, ModelError> {
    let mut positions: Vec<Vec3> = Vec::new();
    let mut normals: Vec<Vec3> = Vec::new();
    let mut texcoords: Vec<[f32; 2]> = Vec::new();
    let mut vertex_colors: Vec<Option<[u8; 4]>> = Vec::new();
    let mut materials: HashMap<String, MtlMaterial> = HashMap::new();
    let mut textures: HashMap<String, Option<Arc<Texture>>> = HashMap::new();
    let mut current_material: Option<String> = None;
    let mut triangle_materials: Vec<Option<String>> = Vec::new();
    let mut every_face_has_normals = true;
    let mut corner_normals: Vec<Vec3> = Vec::new();
    // Triangle corners as (position, texture coordinate or NO_UV).
    let mut corners: Vec<[(u32, u32); 3]> = Vec::new();
    let mut polygon: Vec<(u32, u32)> = Vec::new();
    let mut polygon_normals: Vec<Option<u32>> = Vec::new();

    for (number, line) in text.lines().enumerate() {
        let line_number = number + 1;
        let line = line.split('#').next().unwrap_or_default().trim();
        let mut words = line.split_ascii_whitespace();
        let Some(keyword) = words.next() else {
            continue;
        };
        match keyword {
            "v" => {
                check_limit(positions.len() + 1, limits.max_vertices, "vertices")?;
                let values = floats(words, line_number)?;
                if values.len() < 3 {
                    return Err(parse_error(line_number, "vertex needs x y z"));
                }
                positions.push([values[0], values[1], values[2]]);
                vertex_colors.push((values.len() >= 6).then(|| {
                    [
                        unit_to_byte(values[3]),
                        unit_to_byte(values[4]),
                        unit_to_byte(values[5]),
                        255,
                    ]
                }));
            }
            "vn" => {
                check_limit(normals.len() + 1, limits.max_vertices, "normals")?;
                let values = floats(words, line_number)?;
                if values.len() < 3 {
                    return Err(parse_error(line_number, "normal needs x y z"));
                }
                normals.push([values[0], values[1], values[2]]);
            }
            "vt" => {
                check_limit(
                    texcoords.len() + 1,
                    limits.max_vertices,
                    "texture coordinates",
                )?;
                let values = floats(words, line_number)?;
                let u = values.first().copied().unwrap_or(0.0);
                let v = values.get(1).copied().unwrap_or(0.0);
                // OBJ's v points up; meshes store it pointing down.
                texcoords.push([u, 1.0 - v]);
            }
            "f" => {
                polygon.clear();
                polygon_normals.clear();
                for word in words {
                    let mut parts = word.split('/');
                    let vertex = resolve(parts.next(), positions.len(), line_number)?
                        .ok_or_else(|| parse_error(line_number, "face corner without a vertex"))?;
                    // A bad texture coordinate index loses the texture, not the face.
                    let uv = resolve(parts.next(), texcoords.len(), line_number)
                        .ok()
                        .flatten()
                        .unwrap_or(NO_UV);
                    let normal = resolve(parts.next(), normals.len(), line_number)?;
                    polygon.push((vertex, uv));
                    polygon_normals.push(normal);
                }
                if polygon.len() < 3 {
                    return Err(parse_error(line_number, "face needs at least 3 corners"));
                }
                let has_normals = polygon_normals.iter().all(Option::is_some);
                every_face_has_normals &= has_normals;
                check_limit(
                    corners.len() + polygon.len() - 2,
                    limits.max_triangles,
                    "triangles",
                )?;
                let indices: Vec<u32> = (0..polygon.len() as u32).collect();
                fan(&indices, |[a, b, c]| {
                    corners.push([a, b, c].map(|corner| polygon[corner as usize]));
                    triangle_materials.push(current_material.clone());
                    if every_face_has_normals {
                        for corner in [a, b, c] {
                            let index = polygon_normals[corner as usize].unwrap_or(0);
                            corner_normals.push(normals[index as usize]);
                        }
                    }
                });
            }
            "mtllib" => {
                // File names may contain spaces; OBJ writers emit one library per line in practice.
                let name = line["mtllib".len()..].trim();
                if let Some(library) = mtl(name) {
                    parse_mtl(&library, &mut materials);
                }
            }
            "usemtl" => {
                let name = line["usemtl".len()..].trim();
                current_material = materials.contains_key(name).then(|| name.to_owned());
            }
            _ => {}
        }
    }

    // Split vertices whose corners use different texture coordinates.
    let mut mesh = Mesh::default();
    let uses_uvs = corners
        .iter()
        .any(|triangle| triangle.iter().any(|&(_, uv)| uv != NO_UV));
    let mut kept_colors = Vec::new();
    if uses_uvs {
        let mut split: HashMap<(u32, u32), u32> = HashMap::new();
        for triangle in &corners {
            let triangle = triangle.map(|key| {
                *split.entry(key).or_insert_with(|| {
                    mesh.positions.push(positions[key.0 as usize]);
                    mesh.uvs.push(if key.1 == NO_UV {
                        [0.0; 2]
                    } else {
                        texcoords[key.1 as usize]
                    });
                    kept_colors.push(vertex_colors[key.0 as usize]);
                    mesh.positions.len() as u32 - 1
                })
            });
            mesh.triangles.push(triangle);
        }
        check_limit(mesh.positions.len(), limits.max_vertices, "vertices")?;
    } else {
        mesh.positions = positions;
        mesh.triangles = corners
            .iter()
            .map(|triangle| triangle.map(|(vertex, _)| vertex))
            .collect();
        kept_colors = vertex_colors;
    }

    if every_face_has_normals {
        mesh.corner_normals = corner_normals;
    }
    let rich = triangle_materials
        .iter()
        .flatten()
        .any(|name| materials.get(name).is_some_and(MtlMaterial::is_rich));
    if rich {
        let mut slots: HashMap<Option<&str>, u32> = HashMap::new();
        for name in &triangle_materials {
            let next = slots.len() as u32;
            let slot = *slots.entry(name.as_deref()).or_insert_with(|| {
                let material = match name.as_deref().and_then(|name| materials.get(name)) {
                    Some(source) => source.build(name.as_deref().unwrap_or(""), &mut |file| {
                        textures
                            .entry(file.to_owned())
                            .or_insert_with(|| texture(file).map(Arc::new))
                            .clone()
                    }),
                    None => {
                        let mut material = Material::new("default");
                        material.color = DEFAULT_MATERIAL;
                        material
                    }
                };
                mesh.materials.push(material);
                next
            });
            mesh.triangle_materials.push(slot);
        }
    } else if triangle_materials.iter().any(Option::is_some) {
        mesh.triangle_colors = triangle_materials
            .iter()
            .map(|name| {
                name.as_deref()
                    .and_then(|name| materials.get(name))
                    .map_or(DEFAULT_MATERIAL, |material| material.color)
            })
            .collect();
    }
    if kept_colors.iter().any(Option::is_some) {
        mesh.vertex_colors = kept_colors
            .into_iter()
            .map(|color| color.unwrap_or([255; 4]))
            .collect();
    }
    Ok(mesh)
}

/// One `newmtl` block.
#[derive(Clone, Debug, Default)]
struct MtlMaterial {
    color: [u8; 4],
    diffuse_map: Option<String>,
    emissive: Option<[f32; 3]>,
    emissive_map: Option<String>,
    normal_map: Option<(String, f32)>,
    roughness: Option<f32>,
    roughness_map: Option<String>,
    metallic: Option<f32>,
    metallic_map: Option<String>,
    alpha_map: bool,
    shininess: Option<f32>,
}

impl MtlMaterial {
    /// Uses more than a plain colour.
    fn is_rich(&self) -> bool {
        self.diffuse_map.is_some()
            || self
                .emissive
                .is_some_and(|emissive| emissive.iter().any(|&value| value > 0.0))
            || self.emissive_map.is_some()
            || self.normal_map.is_some()
            || self.roughness.is_some()
            || self.metallic.is_some_and(|metallic| metallic > 0.0)
            || self.roughness_map.is_some()
            || self.metallic_map.is_some()
    }

    fn build(&self, name: &str, texture: &mut dyn FnMut(&str) -> Option<Arc<Texture>>) -> Material {
        let mut material = Material::new(name);
        material.color = self.color;
        if let Some(file) = &self.diffuse_map {
            material.texture = texture(file);
            if material.texture.is_some() {
                // Exporters write a grey Kd next to the map; the map is the colour.
                material.color = [255, 255, 255, self.color[3]];
            }
        }
        if self.alpha_map
            || material
                .texture
                .as_ref()
                .is_some_and(|texture| has_cutouts(texture))
        {
            material.alpha_test = Some(128);
        } else if self.color[3] < 250 {
            material.alpha_blend = true;
        }
        material.emissive_texture = self.emissive_map.as_deref().and_then(&mut *texture);
        material.emissive = match self.emissive {
            Some(emissive) => emissive,
            None if material.emissive_texture.is_some() => [1.0; 3],
            None => [0.0; 3],
        };
        if let Some((file, scale)) = &self.normal_map
            && let Some(map) = texture(file)
            // Old files put grey height maps under `bump`; only colourful maps are normal maps.
            && !is_greyscale(&map)
        {
            material.normal_texture = Some(map);
            material.normal_scale = *scale;
        }
        material.metallic = self.metallic.unwrap_or(0.0);
        // Blender writes Ns = (1 - roughness)² × 1000; use it when there is no Pr.
        material.roughness = self.roughness.unwrap_or_else(|| {
            self.shininess.map_or(0.6, |shininess| {
                1.0 - (shininess.clamp(0.0, 1000.0) / 1000.0).sqrt()
            })
        });
        let roughness_map = self.roughness_map.as_deref().and_then(&mut *texture);
        let metallic_map = self.metallic_map.as_deref().and_then(&mut *texture);
        if roughness_map.is_some() || metallic_map.is_some() {
            material.metallic_roughness_texture =
                combine_channels(roughness_map.as_deref(), metallic_map.as_deref());
            // The maps hold the values; the factors multiply them.
            if roughness_map.is_some() && self.roughness.is_none() {
                material.roughness = 1.0;
            }
            if metallic_map.is_some() && self.metallic.is_none() {
                material.metallic = 1.0;
            }
        }
        material
    }
}

/// A glTF-layout metallic/roughness texture (green = roughness, blue = metallic) from single-channel
/// maps (their red channel), at the first map's size.
fn combine_channels(
    roughness: Option<&Texture>,
    metallic: Option<&Texture>,
) -> Option<Arc<Texture>> {
    let size = roughness.or(metallic)?;
    let (width, height) = (size.width(), size.height());
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    let nearest = |texture: Option<&Texture>, x: u32, y: u32| -> u8 {
        texture.map_or(255, |texture| {
            let tx = (u64::from(x) * u64::from(texture.width()) / u64::from(width)) as usize;
            let ty = (u64::from(y) * u64::from(texture.height()) / u64::from(height)) as usize;
            texture.pixels()[(ty * texture.width() as usize + tx) * 4]
        })
    };
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[
                255,
                nearest(roughness, x, y),
                nearest(metallic, x, y),
                255,
            ]);
        }
    }
    Texture::new(width, height, pixels).map(Arc::new)
}

/// True when every sampled texel is grey (a height map, not a normal map).
fn is_greyscale(texture: &Texture) -> bool {
    let pixels = texture.pixels();
    let step = (pixels.len() / 4 / 4096).max(1) * 4;
    pixels
        .chunks_exact(4)
        .step_by(step / 4)
        .all(|texel| texel[0].abs_diff(texel[1]) < 8 && texel[1].abs_diff(texel[2]) < 8)
}

/// True when some texels are clearly see-through (leaves, hair cards, decals).
fn has_cutouts(texture: &Texture) -> bool {
    texture.pixels().chunks_exact(4).any(|texel| texel[3] < 128)
}

/// The file name in a texture statement, past options such as `-bm 0.5` or `-s 1 1 1`. Returns the
/// name and the `-bm` (bump multiplier) value.
fn map_statement(rest: &str) -> Option<(String, f32)> {
    let words: Vec<&str> = rest.split_ascii_whitespace().collect();
    let mut index = 0;
    let mut bump = 1.0;
    // Options never take the last word: that is the file name.
    while index + 1 < words.len() && words[index].starts_with('-') {
        let option = words[index];
        index += 1;
        let (minimum, maximum) = match option {
            "-o" | "-s" | "-t" => (1, 3),
            "-mm" => (2, 2),
            _ => (1, 1),
        };
        let mut taken = 0;
        while taken < maximum && index + 1 < words.len() {
            let numeric = words[index].parse::<f32>().is_ok();
            if taken >= minimum && !numeric {
                break;
            }
            if option == "-bm" {
                bump = words[index].parse().unwrap_or(1.0);
            }
            index += 1;
            taken += 1;
        }
    }
    let name = words.get(index..)?.join(" ");
    (!name.is_empty()).then_some((name, bump))
}

/// Reads `newmtl` blocks: `Kd`, `d`/`Tr`, `Ke`, `Pr`, `Pm`, `Ns` and the texture maps.
fn parse_mtl(text: &str, materials: &mut HashMap<String, MtlMaterial>) {
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let mut words = line.split_ascii_whitespace();
        let Some(keyword) = words.next() else {
            continue;
        };
        if keyword == "newmtl" {
            let name = line["newmtl".len()..].trim().to_owned();
            materials.insert(
                name.clone(),
                MtlMaterial {
                    color: DEFAULT_MATERIAL,
                    ..MtlMaterial::default()
                },
            );
            current = Some(name);
            continue;
        }
        let Some(material) = current.as_ref().and_then(|name| materials.get_mut(name)) else {
            continue;
        };
        let rest = line[keyword.len()..].trim();
        let numbers: Vec<f32> = words.filter_map(|word| word.parse().ok()).collect();
        match keyword {
            "Kd" => {
                if let [r, g, b, ..] = numbers.as_slice() {
                    material.color[..3].copy_from_slice(&[*r, *g, *b].map(unit_to_byte));
                }
            }
            "d" | "Tr" => {
                if let Some(&value) = numbers.first() {
                    let opacity = if keyword == "Tr" { 1.0 - value } else { value };
                    material.color[3] = unit_to_byte(opacity);
                }
            }
            "Ke" => {
                if let [r, g, b, ..] = numbers.as_slice() {
                    // MTL colours are display (sRGB) values; values above 1 are kept as strength.
                    material.emissive = Some([*r, *g, *b].map(|value| {
                        if value <= 1.0 {
                            srgb_float_to_linear(value.max(0.0) * 255.0)
                        } else {
                            value
                        }
                    }));
                }
            }
            "Pr" => material.roughness = numbers.first().map(|value| value.clamp(0.0, 1.0)),
            "Pm" => material.metallic = numbers.first().map(|value| value.clamp(0.0, 1.0)),
            "Ns" => material.shininess = numbers.first().copied(),
            _ => {
                let Some((file, bump)) = map_statement(rest) else {
                    continue;
                };
                match keyword.to_ascii_lowercase().as_str() {
                    "map_kd" => material.diffuse_map = Some(file),
                    "map_ke" => material.emissive_map = Some(file),
                    "map_bump" | "bump" | "norm" | "map_norm" => {
                        material.normal_map = Some((file, bump))
                    }
                    "map_pr" => material.roughness_map = Some(file),
                    "map_pm" => material.metallic_map = Some(file),
                    "map_d" => material.alpha_map = true,
                    _ => {}
                }
            }
        }
    }
}

fn floats<'a>(words: impl Iterator<Item = &'a str>, line: usize) -> Result<Vec<f32>, ModelError> {
    words
        .map(|word| {
            word.parse::<f32>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| parse_error(line, format!("not a number: {word}")))
        })
        .collect()
}

/// An OBJ index (1-based, or negative from the end) as a 0-based index into `count` items.
fn resolve(word: Option<&str>, count: usize, line: usize) -> Result<Option<u32>, ModelError> {
    let Some(word) = word.filter(|word| !word.is_empty()) else {
        return Ok(None);
    };
    let value: i64 = word
        .parse()
        .map_err(|_| parse_error(line, format!("bad index: {word}")))?;
    let index = if value > 0 {
        value - 1
    } else if value < 0 {
        count as i64 + value
    } else {
        return Err(parse_error(line, "index 0 is not valid in OBJ"));
    };
    if index < 0 || index >= count as i64 {
        return Err(parse_error(line, format!("index {value} out of range")));
    }
    Ok(Some(index as u32))
}

pub(crate) fn unit_to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_materials_normals_and_negative_indices() {
        let obj = "\
# comment
mtllib colors.mtl
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
vn 0 0 1
usemtl red
f 1//1 2//1 3//1 4//1
usemtl unknown
f -4/7/1 -2/8/1 -1/9/1
";
        let mesh = parse_obj(
            obj,
            |name| {
                assert_eq!(name, "colors.mtl");
                Some("newmtl red\nKd 1 0 0\nd 0.5\n".to_owned())
            },
            ModelLimits::default(),
        )
        .unwrap();
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 2, 3], [0, 2, 3]]);
        assert_eq!(mesh.corner_normals.len(), 9);
        assert_eq!(
            mesh.triangle_colors,
            vec![[255, 0, 0, 128], [255, 0, 0, 128], DEFAULT_MATERIAL]
        );
    }

    #[test]
    fn texture_coordinates_split_vertices_and_load_maps() {
        let obj = "\
mtllib m.mtl
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
vt 0 0
vt 1 0
vt 1 1
vt 0 1
vt 0.5 0.5
usemtl skin
f 1/1 2/2 3/3
f 1/5 3/3 4/4
";
        let mtl = "newmtl skin\nKd 0.8 0.8 0.8\nKe 1 0 0\nmap_Kd -s 1 1 1 tex/skin file.png\nmap_Bump -bm 0.5 normal.png\nbump height.png\nPr 0.25\n";
        let mut asked = Vec::new();
        let mesh = parse_obj_with(
            obj,
            |_| Some(mtl.to_owned()),
            |name| {
                asked.push(name.to_owned());
                // A grey map (rejected as a normal map) for the height map, colour otherwise.
                let pixel = if name == "height.png" {
                    [90, 90, 90, 255]
                } else {
                    [128, 128, 255, 255]
                };
                Texture::new(1, 1, pixel.to_vec())
            },
            ModelLimits::default(),
        )
        .unwrap();
        // Vertex 1 is used with two texture coordinates, so it is split: 5 vertices.
        assert_eq!(mesh.positions.len(), 5);
        assert_eq!(mesh.uvs[0], [0.0, 1.0], "v flipped to point down");
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [3, 2, 4]]);
        let material = &mesh.materials[0];
        assert!(material.texture.is_some());
        assert_eq!(material.color, [255; 4]);
        assert!(
            material.normal_texture.is_none(),
            "the later grey bump map wins and is rejected"
        );
        assert_eq!(material.emissive, [1.0, 0.0, 0.0]);
        assert_eq!(material.roughness, 0.25);
        assert_eq!(asked, vec!["tex/skin file.png", "height.png"]);
        assert!(mesh.triangle_colors.is_empty());
    }

    #[test]
    fn map_statements_skip_options() {
        assert_eq!(map_statement("a.png"), Some(("a.png".to_owned(), 1.0)));
        assert_eq!(
            map_statement("-bm 0.3 n.png"),
            Some(("n.png".to_owned(), 0.3))
        );
        assert_eq!(
            map_statement("-o 0 0 0 -s 2 2 1 my tex.jpg"),
            Some(("my tex.jpg".to_owned(), 1.0))
        );
        assert_eq!(
            map_statement("-clamp on t.png"),
            Some(("t.png".to_owned(), 1.0))
        );
        assert_eq!(
            map_statement("-imfchan r rough.png"),
            Some(("rough.png".to_owned(), 1.0))
        );
        assert_eq!(map_statement(""), None);
    }

    #[test]
    fn vertex_colors_and_missing_normals() {
        let obj = "v 0 0 0 1 0 0\nv 1 0 0 0 1 0\nv 0 1 0\nf 1 2 3\n";
        let mesh = parse_obj(obj, |_| None, ModelLimits::default()).unwrap();
        assert_eq!(
            mesh.vertex_colors,
            vec![[255, 0, 0, 255], [0, 255, 0, 255], [255; 4]]
        );
        assert!(mesh.corner_normals.is_empty());
    }

    #[test]
    fn errors_are_reported_with_line_numbers() {
        for (obj, line) in [
            ("v 0 0\n", 1),
            ("v 0 0 0\nf 1 2 3\n", 2),
            ("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n", 4),
            ("v a b c\n", 1),
            ("v 0 0 0\nv 0 0 0\nf 1 2\n", 3),
        ] {
            match parse_obj(obj, |_| None, ModelLimits::default()) {
                Err(ModelError::Parse { line: found, .. }) => assert_eq!(found, line, "{obj}"),
                other => panic!("{obj}: {other:?}"),
            }
        }
        let limits = ModelLimits {
            max_triangles: 1,
            ..ModelLimits::default()
        };
        let quad = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n";
        assert!(matches!(
            parse_obj(quad, |_| None, limits),
            Err(ModelError::TooLarge(_))
        ));
    }
}
