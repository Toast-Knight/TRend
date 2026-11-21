//! PLY (Stanford polygon format): text, binary little-endian and binary big-endian. Reads vertex
//! positions, optional vertex normals and colours, polygon faces (fan-triangulated) and optional face
//! colours; other elements and properties are skipped.

use super::{ModelError, ModelLimits, check_limit, fan, parse_error};
use crate::mesh::Mesh;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Encoding {
    Ascii,
    LittleEndian,
    BigEndian,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => return None,
        })
    }

    const fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }

    const fn is_integer(self) -> bool {
        !matches!(self, Self::F32 | Self::F64)
    }
}

#[derive(Clone, Debug)]
enum Property {
    Scalar(Scalar, String),
    List(Scalar, Scalar, String),
}

#[derive(Clone, Debug)]
struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

/// Reads values from the body in the file's encoding.
struct Body<'a> {
    encoding: Encoding,
    bytes: &'a [u8],
    position: usize,
}

impl Body<'_> {
    fn value(&mut self, scalar: Scalar) -> Result<f64, ModelError> {
        if self.encoding == Encoding::Ascii {
            return self.ascii_value(scalar);
        }
        let size = scalar.size();
        let raw = self
            .bytes
            .get(self.position..self.position + size)
            .ok_or_else(|| parse_error(0, "binary data ends early"))?;
        self.position += size;
        let mut buffer = [0_u8; 8];
        buffer[..size].copy_from_slice(raw);
        if self.encoding == Encoding::BigEndian {
            buffer[..size].reverse();
        }
        Ok(match scalar {
            Scalar::I8 => f64::from(buffer[0] as i8),
            Scalar::U8 => f64::from(buffer[0]),
            Scalar::I16 => f64::from(i16::from_le_bytes([buffer[0], buffer[1]])),
            Scalar::U16 => f64::from(u16::from_le_bytes([buffer[0], buffer[1]])),
            Scalar::I32 => f64::from(i32::from_le_bytes([
                buffer[0], buffer[1], buffer[2], buffer[3],
            ])),
            Scalar::U32 => f64::from(u32::from_le_bytes([
                buffer[0], buffer[1], buffer[2], buffer[3],
            ])),
            Scalar::F32 => f64::from(f32::from_le_bytes([
                buffer[0], buffer[1], buffer[2], buffer[3],
            ])),
            Scalar::F64 => f64::from_le_bytes(buffer),
        })
    }

    fn ascii_value(&mut self, scalar: Scalar) -> Result<f64, ModelError> {
        while self
            .bytes
            .get(self.position)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.position += 1;
        }
        let start = self.position;
        while self
            .bytes
            .get(self.position)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            self.position += 1;
        }
        if start == self.position {
            return Err(parse_error(0, "text data ends early"));
        }
        let word = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| parse_error(0, "text data is not UTF-8"))?;
        let value: f64 = word
            .parse()
            .map_err(|_| parse_error(0, format!("not a number: {word}")))?;
        if scalar.is_integer() && value.fract() != 0.0 {
            return Err(parse_error(0, format!("not an integer: {word}")));
        }
        Ok(value)
    }
}

/// Parses PLY `bytes`. The mesh is not sanitized yet; [`super::load`] does that.
pub fn parse_ply(bytes: &[u8], limits: ModelLimits) -> Result<Mesh, ModelError> {
    let (encoding, elements, body_start) = parse_header(bytes)?;
    let mut body = Body {
        encoding,
        bytes: &bytes[body_start..],
        position: 0,
    };
    let mut mesh = Mesh::default();
    let mut vertex_normals: Vec<[f32; 3]> = Vec::new();
    let mut polygon: Vec<u32> = Vec::new();
    for element in &elements {
        match element.name.as_str() {
            "vertex" => {
                check_limit(element.count, limits.max_vertices, "vertices")?;
                read_vertices(&mut body, element, &mut mesh, &mut vertex_normals)?;
            }
            "face" => {
                check_limit(element.count, limits.max_triangles, "triangles")?;
                read_faces(&mut body, element, &mut mesh, &mut polygon, limits)?;
            }
            _ => {
                for _ in 0..element.count {
                    for property in &element.properties {
                        skip(&mut body, property)?;
                    }
                }
            }
        }
    }
    if vertex_normals.len() == mesh.positions.len() && !vertex_normals.is_empty() {
        let mut corner_normals = Vec::with_capacity(mesh.triangles.len() * 3);
        for triangle in &mesh.triangles {
            for &vertex in triangle {
                corner_normals.push(
                    vertex_normals
                        .get(vertex as usize)
                        .copied()
                        .unwrap_or_default(),
                );
            }
        }
        mesh.corner_normals = corner_normals;
    }
    Ok(mesh)
}

fn parse_header(bytes: &[u8]) -> Result<(Encoding, Vec<Element>, usize), ModelError> {
    let end =
        find(bytes, b"end_header").ok_or_else(|| parse_error(0, "PLY header has no end_header"))?;
    let mut body_start = end + b"end_header".len();
    // The header ends at the newline after end_header (\n or \r\n).
    if bytes.get(body_start) == Some(&b'\r') {
        body_start += 1;
    }
    if bytes.get(body_start) == Some(&b'\n') {
        body_start += 1;
    }
    let header =
        std::str::from_utf8(&bytes[..end]).map_err(|_| parse_error(0, "PLY header is not text"))?;
    let mut lines = header.lines().enumerate();
    if lines.next().map(|(_, line)| line.trim()) != Some("ply") {
        return Err(parse_error(1, "missing ply signature"));
    }
    let mut encoding = None;
    let mut elements: Vec<Element> = Vec::new();
    for (number, line) in lines {
        let line_number = number + 1;
        let words: Vec<&str> = line.split_ascii_whitespace().collect();
        match words.as_slice() {
            ["format", name, ..] => {
                encoding = Some(match *name {
                    "ascii" => Encoding::Ascii,
                    "binary_little_endian" => Encoding::LittleEndian,
                    "binary_big_endian" => Encoding::BigEndian,
                    other => {
                        return Err(parse_error(line_number, format!("unknown format {other}")));
                    }
                });
            }
            ["element", name, count] => elements.push(Element {
                name: (*name).to_owned(),
                count: count
                    .parse()
                    .map_err(|_| parse_error(line_number, "bad element count"))?,
                properties: Vec::new(),
            }),
            ["property", "list", count, item, name] => {
                let element = elements
                    .last_mut()
                    .ok_or_else(|| parse_error(line_number, "property before element"))?;
                let count = Scalar::parse(count)
                    .filter(|scalar| scalar.is_integer())
                    .ok_or_else(|| parse_error(line_number, "bad list count type"))?;
                let item = Scalar::parse(item)
                    .ok_or_else(|| parse_error(line_number, "bad list item type"))?;
                element
                    .properties
                    .push(Property::List(count, item, (*name).to_owned()));
            }
            ["property", scalar, name] => {
                let element = elements
                    .last_mut()
                    .ok_or_else(|| parse_error(line_number, "property before element"))?;
                let scalar = Scalar::parse(scalar)
                    .ok_or_else(|| parse_error(line_number, format!("unknown type {scalar}")))?;
                element
                    .properties
                    .push(Property::Scalar(scalar, (*name).to_owned()));
            }
            _ => {}
        }
    }
    let encoding = encoding.ok_or_else(|| parse_error(0, "PLY header has no format"))?;
    Ok((encoding, elements, body_start))
}

fn read_vertices(
    body: &mut Body<'_>,
    element: &Element,
    mesh: &mut Mesh,
    normals: &mut Vec<[f32; 3]>,
) -> Result<(), ModelError> {
    let names: Vec<&str> = element
        .properties
        .iter()
        .map(|property| match property {
            Property::Scalar(_, name) | Property::List(_, _, name) => name.as_str(),
        })
        .collect();
    let has = |wanted: &[&str]| wanted.iter().all(|name| names.contains(name));
    if !has(&["x", "y", "z"]) {
        return Err(parse_error(0, "vertices have no x y z"));
    }
    let has_normals = has(&["nx", "ny", "nz"]);
    let has_colors = has(&["red", "green", "blue"]) || has(&["r", "g", "b"]);
    mesh.positions.reserve(element.count);
    let mut values = [0.0_f64; 10];
    for _ in 0..element.count {
        let mut color_scale = [1.0_f64; 4];
        values[6..].copy_from_slice(&[255.0; 4]);
        for property in &element.properties {
            match property {
                Property::Scalar(scalar, name) => {
                    let value = body.value(*scalar)?;
                    let slot = match name.as_str() {
                        "x" => 0,
                        "y" => 1,
                        "z" => 2,
                        "nx" => 3,
                        "ny" => 4,
                        "nz" => 5,
                        "red" | "r" | "diffuse_red" => 6,
                        "green" | "g" | "diffuse_green" => 7,
                        "blue" | "b" | "diffuse_blue" => 8,
                        "alpha" | "a" => 9,
                        _ => continue,
                    };
                    if slot >= 6 && !scalar.is_integer() {
                        color_scale[slot - 6] = 255.0;
                    }
                    values[slot] = value;
                }
                Property::List(..) => skip(body, property)?,
            }
        }
        let point = [values[0] as f32, values[1] as f32, values[2] as f32];
        if !point.iter().all(|value| value.is_finite()) {
            return Err(parse_error(0, "vertex is not a finite number"));
        }
        mesh.positions.push(point);
        if has_normals {
            normals.push([values[3] as f32, values[4] as f32, values[5] as f32]);
        }
        if has_colors {
            mesh.vertex_colors.push(std::array::from_fn(|channel| {
                (values[6 + channel] * color_scale[channel])
                    .clamp(0.0, 255.0)
                    .round() as u8
            }));
        }
    }
    Ok(())
}

fn read_faces(
    body: &mut Body<'_>,
    element: &Element,
    mesh: &mut Mesh,
    polygon: &mut Vec<u32>,
    limits: ModelLimits,
) -> Result<(), ModelError> {
    let vertex_count = mesh.positions.len();
    let has_colors = element.properties.iter().any(|property| {
        matches!(property, Property::Scalar(_, name) if matches!(name.as_str(), "red" | "r"))
    });
    let mut found_indices = false;
    for _ in 0..element.count {
        let mut color = [255_u8; 4];
        polygon.clear();
        for property in &element.properties {
            match property {
                Property::List(count, item, name)
                    if name == "vertex_indices" || name == "vertex_index" =>
                {
                    found_indices = true;
                    let length = body.value(*count)?;
                    if !(0.0..=1024.0).contains(&length) {
                        return Err(parse_error(0, "face has too many corners"));
                    }
                    for _ in 0..length as usize {
                        let index = body.value(*item)?;
                        if index < 0.0 || index as usize >= vertex_count {
                            return Err(parse_error(0, format!("face index {index} out of range")));
                        }
                        polygon.push(index as u32);
                    }
                }
                Property::Scalar(scalar, name) => {
                    let value = body.value(*scalar)?;
                    let factor = if scalar.is_integer() { 1.0 } else { 255.0 };
                    let channel = match name.as_str() {
                        "red" | "r" => 0,
                        "green" | "g" => 1,
                        "blue" | "b" => 2,
                        "alpha" | "a" => 3,
                        _ => continue,
                    };
                    color[channel] = (value * factor).clamp(0.0, 255.0).round() as u8;
                }
                Property::List(..) => skip(body, property)?,
            }
        }
        let triangles = polygon.len().saturating_sub(2);
        check_limit(
            mesh.triangles.len() + triangles,
            limits.max_triangles,
            "triangles",
        )?;
        fan(polygon, |triangle| {
            mesh.triangles.push(triangle);
            if has_colors {
                mesh.triangle_colors.push(color);
            }
        });
    }
    if element.count > 0 && !found_indices {
        return Err(parse_error(0, "faces have no vertex_indices"));
    }
    Ok(())
}

fn skip(body: &mut Body<'_>, property: &Property) -> Result<(), ModelError> {
    match property {
        Property::Scalar(scalar, _) => {
            body.value(*scalar)?;
        }
        Property::List(count, item, _) => {
            let length = body.value(*count)?;
            if !(0.0..=1_000_000.0).contains(&length) {
                return Err(parse_error(0, "list is too long"));
            }
            for _ in 0..length as usize {
                body.value(*item)?;
            }
        }
    }
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .take(64 * 1024)
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASCII: &str = "ply
format ascii 1.0
comment made by hand
element vertex 4
property float x
property float y
property float z
property uchar red
property uchar green
property uchar blue
element face 1
property list uchar int vertex_indices
element edge 1
property int vertex1
property int vertex2
end_header
0 0 0 255 0 0
1 0 0 0 255 0
1 1 0 0 0 255
0 1 0 255 255 255
4 0 1 2 3
0 2
";

    #[test]
    fn ascii_quad_with_vertex_colors() {
        let mesh = parse_ply(ASCII.as_bytes(), ModelLimits::default()).unwrap();
        assert_eq!(mesh.positions.len(), 4);
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
        assert_eq!(mesh.vertex_colors[1], [0, 255, 0, 255]);
        assert!(mesh.triangle_colors.is_empty());
    }

    fn binary(big_endian: bool) -> Vec<u8> {
        let format = if big_endian {
            "binary_big_endian"
        } else {
            "binary_little_endian"
        };
        let mut bytes = format!(
            "ply\r\nformat {format} 1.0\r\nelement vertex 3\r\nproperty double x\r\nproperty double y\r\nproperty double z\r\nproperty float nx\r\nproperty float ny\r\nproperty float nz\r\nelement face 1\r\nproperty uchar red\r\nproperty list uchar uint vertex_indices\r\nproperty float green\r\nend_header\r\n"
        )
        .into_bytes();
        let mut push = |raw: &[u8]| {
            let mut raw = raw.to_vec();
            if big_endian {
                raw.reverse();
            }
            bytes.extend_from_slice(&raw);
        };
        for point in [[0.0_f64, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]] {
            for value in point {
                push(&value.to_le_bytes());
            }
            for value in [0.0_f32, 0.0, 1.0] {
                push(&value.to_le_bytes());
            }
        }
        push(&[200]);
        push(&[3]);
        for index in [0_u32, 1, 2] {
            push(&index.to_le_bytes());
        }
        push(&0.5_f32.to_le_bytes());
        bytes
    }

    #[test]
    fn binary_both_endians_with_normals_and_face_colors() {
        for big_endian in [false, true] {
            let mesh = parse_ply(&binary(big_endian), ModelLimits::default()).unwrap();
            assert_eq!(mesh.positions[1], [2.0, 0.0, 0.0]);
            assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
            assert_eq!(mesh.corner_normals, vec![[0.0, 0.0, 1.0]; 3]);
            assert_eq!(mesh.triangle_colors, vec![[200, 128, 255, 255]]);
        }
    }

    #[test]
    fn bad_files_are_errors() {
        let truncated = &binary(false)[..binary(false).len() - 3];
        assert!(parse_ply(truncated, ModelLimits::default()).is_err());
        let out_of_range = ASCII.replace("4 0 1 2 3", "3 0 1 9");
        assert!(parse_ply(out_of_range.as_bytes(), ModelLimits::default()).is_err());
        assert!(parse_ply(b"ply\nformat ascii 1.0\n", ModelLimits::default()).is_err());
        let limits = ModelLimits {
            max_vertices: 3,
            ..ModelLimits::default()
        };
        assert!(matches!(
            parse_ply(ASCII.as_bytes(), limits),
            Err(ModelError::TooLarge(_))
        ));
    }
}
