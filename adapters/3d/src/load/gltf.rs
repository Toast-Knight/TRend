//! glTF 2.0, as `.gltf` (JSON with `.bin` files or embedded data) and `.glb` (binary).
//!
//! Reads the default scene's node tree (with transforms), triangle primitives (lists, strips and
//! fans; indexed or not), positions, normals, tangents, texture coordinates, vertex colours and
//! metallic/roughness materials with base colour, normal, emissive, metallic/roughness and
//! occlusion textures, alpha modes and `KHR_materials_emissive_strength` / `KHR_materials_unlit`.
//! Images are decoded by the caller's [`ImageDecoder`](super::ImageDecoder), so this crate stays
//! free of image codecs. Skins and animations are ignored: models are shown in their rest pose.

use std::collections::HashMap;
use std::sync::Arc;

use super::json::{self, Json};
use super::{ImageDecoder, ModelError, ModelLimits, check_limit, parse_error};
use crate::color::linear_to_srgb;
use crate::math::{Vec3, cross, dot, normalize_or};
use crate::mesh::{Material, Mesh, Texture};

type Matrix = [f32; 16];

/// A buffer, and a view's offset, length and stride in it.
type BufferView = (Arc<Vec<u8>>, usize, usize, usize);

const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// Column-major 4x4 product `a * b`.
fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            out[column * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[column * 4 + k]).sum();
        }
    }
    out
}

fn transform_point(m: &Matrix, p: Vec3) -> Vec3 {
    std::array::from_fn(|row| m[row] * p[0] + m[4 + row] * p[1] + m[8 + row] * p[2] + m[12 + row])
}

fn transform_direction(m: &Matrix, d: Vec3) -> Vec3 {
    std::array::from_fn(|row| m[row] * d[0] + m[4 + row] * d[1] + m[8 + row] * d[2])
}

/// The matrix for normals: the inverse transpose of the upper 3x3 (as cofactors; normals are
/// renormalized afterwards), and the determinant's sign.
fn normal_matrix(m: &Matrix) -> (Matrix, f32) {
    let column = |c: usize| [m[c * 4], m[c * 4 + 1], m[c * 4 + 2]];
    let (a, b, c) = (column(0), column(1), column(2));
    let determinant = dot(a, cross(b, c));
    let (x, y, z) = (cross(b, c), cross(c, a), cross(a, b));
    let sign = if determinant < 0.0 { -1.0 } else { 1.0 };
    let mut out = IDENTITY;
    for (index, value) in [x, y, z].into_iter().enumerate() {
        out[index * 4] = value[0] * sign;
        out[index * 4 + 1] = value[1] * sign;
        out[index * 4 + 2] = value[2] * sign;
    }
    (out, determinant)
}

fn node_matrix(node: &Json) -> Matrix {
    if let Some(values) = node.get("matrix").and_then(Json::as_floats)
        && values.len() == 16
    {
        return values.try_into().unwrap_or(IDENTITY);
    }
    let translation = node
        .get("translation")
        .and_then(Json::as_floats)
        .filter(|values| values.len() == 3)
        .unwrap_or_else(|| vec![0.0; 3]);
    let rotation = node
        .get("rotation")
        .and_then(Json::as_floats)
        .filter(|values| values.len() == 4)
        .unwrap_or_else(|| vec![0.0, 0.0, 0.0, 1.0]);
    let scale = node
        .get("scale")
        .and_then(Json::as_floats)
        .filter(|values| values.len() == 3)
        .unwrap_or_else(|| vec![1.0; 3]);
    let [x, y, z, w] = [rotation[0], rotation[1], rotation[2], rotation[3]];
    let rotation = [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
    ];
    let mut out = IDENTITY;
    for column in 0..3 {
        for row in 0..3 {
            out[column * 4 + row] = rotation[column * 3 + row] * scale[column];
        }
    }
    out[12] = translation[0];
    out[13] = translation[1];
    out[14] = translation[2];
    out
}

fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0_u32;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(value) = bytes
                .get(index + 1..index + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(value);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Document<'a> {
    root: Json,
    binary: Option<&'a [u8]>,
    read: &'a dyn Fn(&str) -> Option<Vec<u8>>,
    images: &'a ImageDecoder<'a>,
    buffers: HashMap<usize, Arc<Vec<u8>>>,
    textures: HashMap<usize, Option<Arc<Texture>>>,
}

impl Document<'_> {
    fn array(&self, key: &str) -> &[Json] {
        self.root.get(key).map_or(&[], Json::as_array)
    }

    fn uri_bytes(&self, uri: &str) -> Option<Vec<u8>> {
        if let Some(rest) = uri.strip_prefix("data:") {
            let (header, data) = rest.split_once(',')?;
            return if header.ends_with(";base64") {
                base64(data)
            } else {
                Some(percent_decode(data).into_bytes())
            };
        }
        (self.read)(&percent_decode(uri))
    }

    fn buffer(&mut self, index: usize) -> Result<Arc<Vec<u8>>, ModelError> {
        if let Some(buffer) = self.buffers.get(&index) {
            return Ok(Arc::clone(buffer));
        }
        let entry = self
            .array("buffers")
            .get(index)
            .ok_or_else(|| parse_error(0, format!("buffer {index} is missing")))?;
        let data = match entry.get("uri").and_then(Json::as_str) {
            Some(uri) => self
                .uri_bytes(uri)
                .ok_or_else(|| parse_error(0, format!("cannot read buffer {uri}")))?,
            None => self
                .binary
                .ok_or_else(|| parse_error(0, "buffer without data"))?
                .to_vec(),
        };
        let buffer = Arc::new(data);
        self.buffers.insert(index, Arc::clone(&buffer));
        Ok(buffer)
    }

    /// A buffer view: its buffer, offset, length and stride (0 = tightly packed).
    fn view(&mut self, index: usize) -> Result<BufferView, ModelError> {
        let view = self
            .array("bufferViews")
            .get(index)
            .cloned()
            .ok_or_else(|| parse_error(0, format!("buffer view {index} is missing")))?;
        let buffer = self.buffer(view.get("buffer").and_then(Json::as_usize).unwrap_or(0))?;
        let offset = view.get("byteOffset").and_then(Json::as_usize).unwrap_or(0);
        let length = view
            .get("byteLength")
            .and_then(Json::as_usize)
            .ok_or_else(|| parse_error(0, "buffer view without byteLength"))?;
        let stride = view.get("byteStride").and_then(Json::as_usize).unwrap_or(0);
        if offset
            .checked_add(length)
            .is_none_or(|end| end > buffer.len())
        {
            return Err(parse_error(
                0,
                format!("buffer view {index} is out of range"),
            ));
        }
        Ok((buffer, offset, length, stride))
    }

    /// An accessor's elements as floats (normalized integers scaled to 0..1 or -1..1).
    fn floats(&mut self, index: usize, limit: usize) -> Result<(Vec<f32>, usize), ModelError> {
        let accessor = self
            .array("accessors")
            .get(index)
            .cloned()
            .ok_or_else(|| parse_error(0, format!("accessor {index} is missing")))?;
        let count = accessor.get("count").and_then(Json::as_usize).unwrap_or(0);
        check_limit(count, limit, "accessor elements")?;
        let components = match accessor.get("type").and_then(Json::as_str) {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            other => {
                return Err(parse_error(
                    0,
                    format!("accessor type {other:?} not supported"),
                ));
            }
        };
        let component_type = accessor
            .get("componentType")
            .and_then(Json::as_usize)
            .unwrap_or(0);
        let normalized = accessor
            .get("normalized")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        let size = match component_type {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            other => {
                return Err(parse_error(
                    0,
                    format!("component type {other} not supported"),
                ));
            }
        };
        let Some(view_index) = accessor.get("bufferView").and_then(Json::as_usize) else {
            // No data: all zeros (sparse accessors are not supported).
            return Ok((vec![0.0; count * components], components));
        };
        let (buffer, view_offset, view_length, stride) = self.view(view_index)?;
        let offset = view_offset
            + accessor
                .get("byteOffset")
                .and_then(Json::as_usize)
                .unwrap_or(0);
        let element = size * components;
        let stride = if stride == 0 { element } else { stride };
        if count > 0
            && (count - 1)
                .checked_mul(stride)
                .and_then(|end| end.checked_add(offset + element))
                .is_none_or(|end| end > view_offset + view_length)
        {
            return Err(parse_error(0, format!("accessor {index} is out of range")));
        }
        let mut out = Vec::with_capacity(count * components);
        for item in 0..count {
            let start = offset + item * stride;
            for component in 0..components {
                let at = start + component * size;
                let bytes = &buffer[at..at + size];
                let value = match component_type {
                    5120 => {
                        let value = f32::from(bytes[0] as i8);
                        if normalized {
                            (value / 127.0).max(-1.0)
                        } else {
                            value
                        }
                    }
                    5121 => {
                        let value = f32::from(bytes[0]);
                        if normalized { value / 255.0 } else { value }
                    }
                    5122 => {
                        let value = f32::from(i16::from_le_bytes([bytes[0], bytes[1]]));
                        if normalized {
                            (value / 32767.0).max(-1.0)
                        } else {
                            value
                        }
                    }
                    5123 => {
                        let value = f32::from(u16::from_le_bytes([bytes[0], bytes[1]]));
                        if normalized { value / 65535.0 } else { value }
                    }
                    5125 => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32,
                    _ => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                };
                out.push(value);
            }
        }
        Ok((out, components))
    }

    /// Index accessors, read exactly (as integers, not floats).
    fn indices(&mut self, index: usize, limit: usize) -> Result<Vec<u32>, ModelError> {
        let accessor = self
            .array("accessors")
            .get(index)
            .cloned()
            .ok_or_else(|| parse_error(0, format!("accessor {index} is missing")))?;
        let count = accessor.get("count").and_then(Json::as_usize).unwrap_or(0);
        check_limit(count, limit, "indices")?;
        let size = match accessor.get("componentType").and_then(Json::as_usize) {
            Some(5121) => 1,
            Some(5123) => 2,
            Some(5125) => 4,
            other => {
                return Err(parse_error(
                    0,
                    format!("index type {other:?} not supported"),
                ));
            }
        };
        let view_index = accessor
            .get("bufferView")
            .and_then(Json::as_usize)
            .ok_or_else(|| parse_error(0, "indices without data"))?;
        let (buffer, view_offset, view_length, stride) = self.view(view_index)?;
        let offset = view_offset
            + accessor
                .get("byteOffset")
                .and_then(Json::as_usize)
                .unwrap_or(0);
        let stride = if stride == 0 { size } else { stride };
        if count > 0 && offset + (count - 1) * stride + size > view_offset + view_length {
            return Err(parse_error(0, format!("accessor {index} is out of range")));
        }
        Ok((0..count)
            .map(|item| {
                let at = offset + item * stride;
                match size {
                    1 => u32::from(buffer[at]),
                    2 => u32::from(u16::from_le_bytes([buffer[at], buffer[at + 1]])),
                    _ => u32::from_le_bytes([
                        buffer[at],
                        buffer[at + 1],
                        buffer[at + 2],
                        buffer[at + 3],
                    ]),
                }
            })
            .collect())
    }

    fn texture(&mut self, info: Option<&Json>) -> Option<Arc<Texture>> {
        let index = info?.get("index")?.as_usize()?;
        if let Some(cached) = self.textures.get(&index) {
            return cached.clone();
        }
        let texture = self.decode_texture(index);
        self.textures.insert(index, texture.clone());
        texture
    }

    fn decode_texture(&mut self, index: usize) -> Option<Arc<Texture>> {
        let texture = self.array("textures").get(index)?.clone();
        // Plain source, or the WebP extension's (the caller's decoder may read WebP).
        let sources = [
            texture.get("source").and_then(Json::as_usize),
            texture
                .get("extensions")
                .and_then(|extensions| extensions.get("EXT_texture_webp"))
                .and_then(|webp| webp.get("source"))
                .and_then(Json::as_usize),
        ];
        for source in sources.into_iter().flatten() {
            let image = self.array("images").get(source)?.clone();
            let bytes = if let Some(view) = image.get("bufferView").and_then(Json::as_usize) {
                let (buffer, offset, length, _) = self.view(view).ok()?;
                buffer[offset..offset + length].to_vec()
            } else {
                let uri = image.get("uri").and_then(Json::as_str)?;
                self.uri_bytes(uri)?
            };
            if let Some(decoded) = (self.images)(&bytes) {
                return Some(Arc::new(decoded));
            }
        }
        None
    }

    fn material(&mut self, index: usize) -> Material {
        let Some(entry) = self.array("materials").get(index).cloned() else {
            return Material::new("default");
        };
        let mut material = Material::new(
            entry
                .get("name")
                .and_then(Json::as_str)
                .unwrap_or("material")
                .to_owned(),
        );
        let pbr = entry.get("pbrMetallicRoughness");
        let factor = pbr
            .and_then(|pbr| pbr.get("baseColorFactor"))
            .and_then(Json::as_floats)
            .filter(|values| values.len() == 4)
            .unwrap_or_else(|| vec![1.0; 4]);
        // glTF factors are linear; materials store sRGB.
        material.color = [
            linear_to_srgb(factor[0]),
            linear_to_srgb(factor[1]),
            linear_to_srgb(factor[2]),
            (factor[3].clamp(0.0, 1.0) * 255.0).round() as u8,
        ];
        material.texture = self.texture(pbr.and_then(|pbr| pbr.get("baseColorTexture")));
        material.metallic = pbr
            .and_then(|pbr| pbr.get("metallicFactor"))
            .and_then(Json::as_f32)
            .unwrap_or(1.0);
        material.roughness = pbr
            .and_then(|pbr| pbr.get("roughnessFactor"))
            .and_then(Json::as_f32)
            .unwrap_or(1.0);
        material.metallic_roughness_texture =
            self.texture(pbr.and_then(|pbr| pbr.get("metallicRoughnessTexture")));
        let normal = entry.get("normalTexture");
        material.normal_texture = self.texture(normal);
        material.normal_scale = normal
            .and_then(|normal| normal.get("scale"))
            .and_then(Json::as_f32)
            .unwrap_or(1.0);
        let occlusion = entry.get("occlusionTexture");
        material.occlusion_texture = self.texture(occlusion);
        material.occlusion_strength = occlusion
            .and_then(|occlusion| occlusion.get("strength"))
            .and_then(Json::as_f32)
            .unwrap_or(1.0);
        material.emissive_texture = self.texture(entry.get("emissiveTexture"));
        let strength = entry
            .get("extensions")
            .and_then(|extensions| extensions.get("KHR_materials_emissive_strength"))
            .and_then(|extension| extension.get("emissiveStrength"))
            .and_then(Json::as_f32)
            .unwrap_or(1.0);
        if let Some(values) = entry
            .get("emissiveFactor")
            .and_then(Json::as_floats)
            .filter(|values| values.len() == 3)
        {
            material.emissive = [values[0], values[1], values[2]].map(|value| value * strength);
        }
        match entry.get("alphaMode").and_then(Json::as_str) {
            Some("MASK") => {
                let cutoff = entry
                    .get("alphaCutoff")
                    .and_then(Json::as_f32)
                    .unwrap_or(0.5);
                material.alpha_test = Some((cutoff.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            Some("BLEND") => material.alpha_blend = true,
            _ => {}
        }
        material.unlit = entry
            .get("extensions")
            .and_then(|extensions| extensions.get("KHR_materials_unlit"))
            .is_some();
        material
    }
}

/// Splits a GLB file into its JSON and binary chunks.
fn split_glb(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>), ModelError> {
    let word = |at: usize| -> Result<u32, ModelError> {
        bytes
            .get(at..at + 4)
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .ok_or_else(|| parse_error(0, "GLB is truncated"))
    };
    if word(4)? != 2 {
        return Err(parse_error(0, "only glTF 2.0 GLB files are supported"));
    }
    let total = (word(8)? as usize).min(bytes.len());
    let mut position = 12;
    let (mut json, mut binary) = (None, None);
    while position + 8 <= total {
        let length = word(position)? as usize;
        let kind = word(position + 4)?;
        let data = bytes
            .get(position + 8..position + 8 + length)
            .ok_or_else(|| parse_error(0, "GLB chunk is truncated"))?;
        match kind {
            0x4E4F_534A => json = Some(data),
            0x004E_4942 => binary = Some(data),
            _ => {}
        }
        position += 8 + length.div_ceil(4) * 4;
    }
    Ok((
        json.ok_or_else(|| parse_error(0, "GLB has no JSON chunk"))?,
        binary,
    ))
}

/// Parses a `.gltf` or `.glb` file. `read` returns files the model refers to (relative paths);
/// `images` decodes embedded or referenced pictures.
pub fn parse_gltf(
    bytes: &[u8],
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
    images: &ImageDecoder<'_>,
    limits: ModelLimits,
) -> Result<Mesh, ModelError> {
    let (text, binary) = if bytes.starts_with(b"glTF") {
        split_glb(bytes)?
    } else {
        (bytes, None)
    };
    let root = json::parse(text).map_err(|message| parse_error(0, message))?;
    let version = root
        .get("asset")
        .and_then(|asset| asset.get("version"))
        .and_then(Json::as_str)
        .unwrap_or("");
    if !version.starts_with('2') {
        return Err(parse_error(
            0,
            format!("glTF version {version:?} is not supported"),
        ));
    }
    for required in root
        .get("extensionsRequired")
        .map_or(&[][..], Json::as_array)
    {
        let name = required.as_str().unwrap_or("");
        if !matches!(
            name,
            "KHR_materials_emissive_strength"
                | "KHR_materials_unlit"
                | "KHR_mesh_quantization"
                | "KHR_texture_transform"
                | "EXT_texture_webp"
        ) {
            return Err(parse_error(
                0,
                format!("glTF extension {name} is not supported"),
            ));
        }
    }
    let mut document = Document {
        root,
        binary,
        read,
        images,
        buffers: HashMap::new(),
        textures: HashMap::new(),
    };

    // The nodes to draw: the default scene's, or every root node.
    let nodes = document.array("nodes").to_vec();
    let scene = document
        .root
        .get("scene")
        .and_then(Json::as_usize)
        .unwrap_or(0);
    let roots: Vec<usize> = match document.array("scenes").get(scene) {
        Some(scene) => scene
            .get("nodes")
            .map_or(&[][..], Json::as_array)
            .iter()
            .filter_map(Json::as_usize)
            .collect(),
        None => {
            let children: Vec<usize> = nodes
                .iter()
                .flat_map(|node| node.get("children").map_or(&[][..], Json::as_array))
                .filter_map(Json::as_usize)
                .collect();
            (0..nodes.len())
                .filter(|index| !children.contains(index))
                .collect()
        }
    };

    let mut builder = Builder::default();
    let mut material_slots: HashMap<Option<usize>, u32> = HashMap::new();
    // A stack, filled in reverse so nodes come out in file order.
    let mut pending: Vec<(usize, Matrix, usize)> = roots
        .into_iter()
        .rev()
        .map(|node| (node, IDENTITY, 0))
        .collect();
    while let Some((index, parent, depth)) = pending.pop() {
        let Some(node) = nodes.get(index) else {
            continue;
        };
        if depth > 64 {
            return Err(parse_error(0, "node tree is too deep"));
        }
        let world = multiply(&parent, &node_matrix(node));
        if let Some(mesh) = node.get("mesh").and_then(Json::as_usize) {
            let primitives = document
                .array("meshes")
                .get(mesh)
                .and_then(|mesh| mesh.get("primitives"))
                .map_or(&[][..], Json::as_array)
                .to_vec();
            for primitive in &primitives {
                builder.add_primitive(
                    &mut document,
                    primitive,
                    &world,
                    &mut material_slots,
                    limits,
                )?;
            }
        }
        for child in node
            .get("children")
            .map_or(&[][..], Json::as_array)
            .iter()
            .rev()
        {
            if let Some(child) = child.as_usize() {
                pending.push((child, world, depth + 1));
            }
        }
    }
    let mut mesh = builder.finish();
    // Materials in slot order.
    let mut slots: Vec<(Option<usize>, u32)> = material_slots.into_iter().collect();
    slots.sort_by_key(|&(_, slot)| slot);
    mesh.materials = slots
        .into_iter()
        .map(|(index, _)| match index {
            Some(index) => document.material(index),
            None => Material::new("default"),
        })
        .collect();
    Ok(mesh)
}

#[derive(Default)]
struct Builder {
    mesh: Mesh,
    normals: Vec<Option<Vec3>>,
    tangents: Vec<Option<[f32; 4]>>,
    uvs: Vec<Option<[f32; 2]>>,
    colors: Vec<Option<[u8; 4]>>,
}

impl Builder {
    fn add_primitive(
        &mut self,
        document: &mut Document<'_>,
        primitive: &Json,
        world: &Matrix,
        material_slots: &mut HashMap<Option<usize>, u32>,
        limits: ModelLimits,
    ) -> Result<(), ModelError> {
        let mode = primitive.get("mode").and_then(Json::as_usize).unwrap_or(4);
        if !(4..=6).contains(&mode) {
            return Ok(()); // Points and lines are not drawn.
        }
        if primitive
            .get("extensions")
            .is_some_and(|extensions| extensions.get("KHR_draco_mesh_compression").is_some())
        {
            return Err(parse_error(0, "Draco-compressed meshes are not supported"));
        }
        let attributes = primitive
            .get("attributes")
            .ok_or_else(|| parse_error(0, "primitive without attributes"))?;
        let Some(position_accessor) = attributes.get("POSITION").and_then(Json::as_usize) else {
            return Ok(());
        };
        let remaining_vertices = limits
            .max_vertices
            .saturating_sub(self.mesh.positions.len());
        let (positions, _) = document.floats(position_accessor, remaining_vertices)?;
        let count = positions.len() / 3;
        let mut attribute = |name: &str, width: usize| -> Result<Option<Vec<f32>>, ModelError> {
            match attributes.get(name).and_then(Json::as_usize) {
                Some(accessor) => {
                    let (values, components) = document.floats(accessor, count)?;
                    Ok(
                        (values.len() / components.max(1) == count && components >= width).then(
                            || {
                                values
                                    .chunks_exact(components)
                                    .flat_map(|chunk| {
                                        let mut item = chunk[..width.min(components)].to_vec();
                                        item.resize(width, 1.0);
                                        item
                                    })
                                    .collect()
                            },
                        ),
                    )
                }
                None => Ok(None),
            }
        };
        let normals = attribute("NORMAL", 3)?;
        let tangents = attribute("TANGENT", 4)?;
        let uvs = attribute("TEXCOORD_0", 2)?;
        let colors = attribute("COLOR_0", 4)?;

        let base = self.mesh.positions.len() as u32;
        let (normal_matrix, determinant) = normal_matrix(world);
        for vertex in 0..count {
            let position = [
                positions[vertex * 3],
                positions[vertex * 3 + 1],
                positions[vertex * 3 + 2],
            ];
            self.mesh.positions.push(transform_point(world, position));
            self.normals.push(normals.as_ref().map(|normals| {
                let normal = [
                    normals[vertex * 3],
                    normals[vertex * 3 + 1],
                    normals[vertex * 3 + 2],
                ];
                normalize_or(transform_direction(&normal_matrix, normal), [0.0, 0.0, 1.0])
            }));
            self.tangents.push(tangents.as_ref().map(|tangents| {
                let t = &tangents[vertex * 4..vertex * 4 + 4];
                let [x, y, z] = normalize_or(
                    transform_direction(world, [t[0], t[1], t[2]]),
                    [1.0, 0.0, 0.0],
                );
                let w = if t[3] < 0.0 { -1.0 } else { 1.0 } * determinant.signum();
                [x, y, z, w]
            }));
            self.uvs.push(
                uvs.as_ref()
                    .map(|uvs| [uvs[vertex * 2], uvs[vertex * 2 + 1]]),
            );
            // Vertex colours are linear in glTF; meshes store sRGB.
            self.colors.push(colors.as_ref().map(|colors| {
                let c = &colors[vertex * 4..vertex * 4 + 4];
                [
                    linear_to_srgb(c[0]),
                    linear_to_srgb(c[1]),
                    linear_to_srgb(c[2]),
                    (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
                ]
            }));
        }

        let indices = match primitive.get("indices").and_then(Json::as_usize) {
            Some(accessor) => document.indices(accessor, limits.max_triangles.saturating_mul(3))?,
            None => (0..count as u32).collect(),
        };
        let mut triangles = Vec::new();
        match mode {
            4 => triangles.extend(indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]])),
            5 => {
                for index in 0..indices.len().saturating_sub(2) {
                    let (a, b, c) = (indices[index], indices[index + 1], indices[index + 2]);
                    triangles.push(if index % 2 == 0 { [a, b, c] } else { [b, a, c] });
                }
            }
            _ => {
                for index in 1..indices.len().saturating_sub(1) {
                    triangles.push([indices[0], indices[index], indices[index + 1]]);
                }
            }
        }
        check_limit(
            self.mesh.triangles.len() + triangles.len(),
            limits.max_triangles,
            "triangles",
        )?;
        let next_slot = material_slots.len() as u32;
        let slot = *material_slots
            .entry(primitive.get("material").and_then(Json::as_usize))
            .or_insert(next_slot);
        for [a, b, c] in triangles {
            if [a, b, c].iter().any(|&index| index as usize >= count) {
                return Err(parse_error(0, "index out of range"));
            }
            // A mirroring transform turns the winding around.
            let triangle = if determinant < 0.0 {
                [a, c, b]
            } else {
                [a, b, c]
            };
            self.mesh.triangles.push(triangle.map(|index| base + index));
            self.mesh.triangle_materials.push(slot);
        }
        Ok(())
    }

    fn finish(mut self) -> Mesh {
        let corners = |values: &[Option<Vec3>], triangles: &[[u32; 3]]| -> Option<Vec<Vec3>> {
            let mut out = Vec::with_capacity(triangles.len() * 3);
            for triangle in triangles {
                for &vertex in triangle {
                    out.push(values[vertex as usize]?);
                }
            }
            Some(out)
        };
        if let Some(normals) = corners(&self.normals, &self.mesh.triangles) {
            self.mesh.corner_normals = normals;
            let mut tangents = Vec::with_capacity(self.mesh.triangles.len() * 3);
            let all = self.mesh.triangles.iter().all(|triangle| {
                triangle.iter().all(|&vertex| {
                    self.tangents[vertex as usize]
                        .map(|tangent| tangents.push(tangent))
                        .is_some()
                })
            });
            if all {
                self.mesh.corner_tangents = tangents;
            }
        }
        if self.uvs.iter().any(Option::is_some) {
            self.mesh.uvs = self.uvs.iter().map(|uv| uv.unwrap_or([0.0; 2])).collect();
        }
        if self.colors.iter().any(Option::is_some) {
            self.mesh.vertex_colors = self
                .colors
                .iter()
                .map(|color| color.unwrap_or([255; 4]))
                .collect();
        }
        self.mesh
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A GLB with one quad (two nodes: one plain, one scaled by -1 in X and moved), a material with
    /// an embedded 2x1 "image" (decoded by the test decoder) and emissive strength.
    pub(crate) fn sample_glb() -> Vec<u8> {
        let mut binary = Vec::new();
        for value in [
            -1.0_f32, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0,
        ] {
            binary.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0.0_f32, 0.0, 1.0].repeat(4) {
            binary.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0.0_f32, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0] {
            binary.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0_u16, 1, 2, 0, 2, 3] {
            binary.extend_from_slice(&value.to_le_bytes());
        }
        let image_offset = binary.len();
        binary.extend_from_slice(b"IMG:red-blue");
        while binary.len() % 4 != 0 {
            binary.push(0);
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0,1]}}],
            "nodes":[{{"mesh":0}},{{"mesh":0,"translation":[3,0,0],"scale":[-1,1,1]}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2}},"indices":3,"material":0}}]}}],
            "materials":[{{"name":"glow","pbrMetallicRoughness":{{"baseColorFactor":[1,1,1,1],"baseColorTexture":{{"index":0}},"metallicFactor":0.25,"roughnessFactor":0.5}},
              "emissiveFactor":[1,0.5,0],"extensions":{{"KHR_materials_emissive_strength":{{"emissiveStrength":4}}}},"alphaMode":"MASK","alphaCutoff":0.25,"doubleSided":true}}],
            "textures":[{{"source":0}}],"images":[{{"bufferView":4,"mimeType":"image/png"}}],
            "accessors":[
              {{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-1,-1,0],"max":[1,1,0]}},
              {{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"}},
              {{"bufferView":2,"componentType":5126,"count":4,"type":"VEC2"}},
              {{"bufferView":3,"componentType":5123,"count":6,"type":"SCALAR"}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":48}},{{"buffer":0,"byteOffset":48,"byteLength":48}},
              {{"buffer":0,"byteOffset":96,"byteLength":32}},{{"buffer":0,"byteOffset":128,"byteLength":12}},
              {{"buffer":0,"byteOffset":{image_offset},"byteLength":12}}],
            "buffers":[{{"byteLength":{}}}]}}"#,
            binary.len()
        );
        let mut json = json.into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut glb = b"glTF".to_vec();
        glb.extend_from_slice(&2_u32.to_le_bytes());
        glb.extend_from_slice(&((12 + 8 + json.len() + 8 + binary.len()) as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F_534A_u32.to_le_bytes());
        glb.extend_from_slice(&json);
        glb.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x004E_4942_u32.to_le_bytes());
        glb.extend_from_slice(&binary);
        glb
    }

    pub(crate) fn test_decoder(bytes: &[u8]) -> Option<Texture> {
        (bytes == b"IMG:red-blue")
            .then(|| Texture::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap())
    }

    #[test]
    fn glb_nodes_materials_and_textures() {
        let mesh = parse_gltf(
            &sample_glb(),
            &|_| None,
            &test_decoder,
            ModelLimits::default(),
        )
        .unwrap();
        assert_eq!(mesh.positions.len(), 8);
        assert_eq!(mesh.triangles.len(), 4);
        // Second node: mirrored and moved to x = 2..4, with its winding turned around.
        assert_eq!(mesh.positions[5], [2.0, -1.0, 0.0]);
        assert_eq!(mesh.triangles[2], [4, 6, 5]);
        assert_eq!(mesh.corner_normals.len(), 12);
        assert_eq!(mesh.uvs[1], [1.0, 1.0]);
        let material = &mesh.materials[0];
        assert_eq!(material.name, "glow");
        assert!(material.texture.is_some());
        assert_eq!((material.metallic, material.roughness), (0.25, 0.5));
        assert_eq!(material.emissive, [4.0, 2.0, 0.0]);
        assert_eq!(material.alpha_test, Some(64));
        assert!(mesh.triangle_materials.iter().all(|&slot| slot == 0));
    }

    #[test]
    fn gltf_json_with_data_uri_and_strips() {
        // A triangle strip of 4 vertices in a base64 data URI, no indices, no material.
        let mut data = Vec::new();
        for value in [
            0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0,
        ] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::new();
        for chunk in data.chunks(3) {
            let bits = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
                if index <= chunk.len() {
                    encoded.push(ALPHABET[((bits >> shift) & 63) as usize] as char);
                } else {
                    encoded.push('=');
                }
            }
        }
        let gltf = format!(
            r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"mesh":0,"rotation":[0,0,0.7071068,0.7071068]}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"mode":5}}]}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3"}}],
            "bufferViews":[{{"buffer":0,"byteLength":48}}],
            "buffers":[{{"byteLength":48,"uri":"data:application/octet-stream;base64,{encoded}"}}]}}"#
        );
        let mesh = parse_gltf(
            gltf.as_bytes(),
            &|_| None,
            &|_| None,
            ModelLimits::default(),
        )
        .unwrap();
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [2, 1, 3]]);
        // Rotated 90° about Z: (1, 0, 0) -> (0, 1, 0).
        let moved = mesh.positions[1];
        assert!(moved[0].abs() < 1e-5 && (moved[1] - 1.0).abs() < 1e-5);
        assert_eq!(mesh.materials.len(), 1);
        assert!(
            mesh.corner_normals.is_empty(),
            "computed later by the loader"
        );
    }

    #[test]
    fn bad_files_are_errors() {
        let decoder = |_: &[u8]| None;
        let none = |_: &str| None;
        for bad in [
            &b"glTF\x01\x00\x00\x00"[..],
            b"{\"asset\":{\"version\":\"1.0\"}}",
            b"{\"asset\":{\"version\":\"2.0\"},\"extensionsRequired\":[\"KHR_draco_mesh_compression\"]}",
            b"not json",
        ] {
            assert!(parse_gltf(bad, &none, &decoder, ModelLimits::default()).is_err());
        }
        let glb = sample_glb();
        let limits = ModelLimits {
            max_triangles: 3,
            ..ModelLimits::default()
        };
        assert!(matches!(
            parse_gltf(&glb, &none, &decoder, limits),
            Err(ModelError::TooLarge(_))
        ));
        assert!(
            parse_gltf(
                &glb[..glb.len() - 40],
                &none,
                &decoder,
                ModelLimits::default()
            )
            .is_err()
        );
    }
}
