//! STL, text and binary. Stored facet normals are often wrong, so they are ignored: the vertices
//! are welded and normals recomputed.

use super::{ModelError, ModelLimits, check_limit, parse_error};
use crate::mesh::Mesh;

/// Parses STL `bytes`, choosing binary or text by the binary format's exact size rule.
pub fn parse_stl(bytes: &[u8], limits: ModelLimits) -> Result<Mesh, ModelError> {
    let mut mesh = if is_binary(bytes) {
        parse_binary(bytes, limits)?
    } else {
        parse_text(bytes, limits)?
    };
    mesh.weld();
    Ok(mesh)
}

/// Binary STL is an 80-byte header, a triangle count, and 50 bytes per triangle. Many binary files
/// also start with "solid", so the size is the reliable test.
fn is_binary(bytes: &[u8]) -> bool {
    bytes.len() >= 84 && {
        let count = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as u64;
        84 + count * 50 == bytes.len() as u64
    }
}

fn parse_binary(bytes: &[u8], limits: ModelLimits) -> Result<Mesh, ModelError> {
    let count = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    check_limit(count, limits.max_triangles, "triangles")?;
    check_limit(count.saturating_mul(3), limits.max_vertices, "vertices")?;
    let mut mesh = Mesh {
        positions: Vec::with_capacity(count * 3),
        triangles: Vec::with_capacity(count),
        ..Mesh::default()
    };
    for record in bytes[84..].chunks_exact(50) {
        let base = mesh.positions.len() as u32;
        for corner in 0..3 {
            let offset = 12 + corner * 12;
            let value = |axis: usize| {
                let start = offset + axis * 4;
                f32::from_le_bytes([
                    record[start],
                    record[start + 1],
                    record[start + 2],
                    record[start + 3],
                ])
            };
            let point = [value(0), value(1), value(2)];
            if !point.iter().all(|value| value.is_finite()) {
                return Err(parse_error(0, "vertex is not a finite number"));
            }
            mesh.positions.push(point);
        }
        mesh.triangles.push([base, base + 1, base + 2]);
    }
    Ok(mesh)
}

fn parse_text(bytes: &[u8], limits: ModelLimits) -> Result<Mesh, ModelError> {
    let text = std::str::from_utf8(bytes).map_err(|_| parse_error(0, "text STL is not UTF-8"))?;
    let mut mesh = Mesh::default();
    let mut corners: Vec<[f32; 3]> = Vec::with_capacity(3);
    let mut saw_solid = false;
    for (number, line) in text.lines().enumerate() {
        let line_number = number + 1;
        let mut words = line.split_ascii_whitespace();
        match words.next() {
            Some("solid") => saw_solid = true,
            Some("outer") => corners.clear(),
            Some("vertex") => {
                let values: Vec<f32> = words
                    .map(|word| {
                        word.parse::<f32>()
                            .ok()
                            .filter(|value| value.is_finite())
                            .ok_or_else(|| {
                                parse_error(line_number, format!("not a number: {word}"))
                            })
                    })
                    .collect::<Result<_, _>>()?;
                let [x, y, z] = values[..] else {
                    return Err(parse_error(line_number, "vertex needs x y z"));
                };
                corners.push([x, y, z]);
            }
            Some("endloop") => {
                if corners.len() < 3 {
                    return Err(parse_error(line_number, "facet needs 3 vertices"));
                }
                check_limit(
                    mesh.triangles.len() + corners.len() - 2,
                    limits.max_triangles,
                    "triangles",
                )?;
                check_limit(
                    mesh.positions.len() + corners.len(),
                    limits.max_vertices,
                    "vertices",
                )?;
                let base = mesh.positions.len() as u32;
                mesh.positions.extend_from_slice(&corners);
                for index in 1..corners.len() as u32 - 1 {
                    mesh.triangles.push([base, base + index, base + index + 1]);
                }
                corners.clear();
            }
            _ => {}
        }
    }
    if !saw_solid {
        return Err(parse_error(0, "neither binary STL nor text STL"));
    }
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn binary(triangles: &[[[f32; 3]; 3]]) -> Vec<u8> {
        let mut bytes = b"solid looks like text but is binary".to_vec();
        bytes.resize(80, b' ');
        bytes.extend_from_slice(&(triangles.len() as u32).to_le_bytes());
        for triangle in triangles {
            bytes.extend_from_slice(&[0; 12]);
            for corner in triangle {
                for value in corner {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            bytes.extend_from_slice(&[0; 2]);
        }
        bytes
    }

    #[test]
    fn binary_triangles_are_welded() {
        let bytes = binary(&[
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
        ]);
        let mesh = parse_stl(&bytes, ModelLimits::default()).unwrap();
        assert_eq!(mesh.positions.len(), 4);
        assert_eq!(mesh.triangles.len(), 2);
        let limits = ModelLimits {
            max_triangles: 1,
            ..ModelLimits::default()
        };
        assert!(matches!(
            parse_stl(&bytes, limits),
            Err(ModelError::TooLarge(_))
        ));
    }

    #[test]
    fn text_facets_and_errors() {
        let text = b"solid t
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid t
";
        let mesh = parse_stl(text, ModelLimits::default()).unwrap();
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
        assert!(matches!(
            parse_stl(
                b"solid t\nouter loop\nvertex 0 0\nendloop\n",
                ModelLimits::default()
            ),
            Err(ModelError::Parse { line: 3, .. })
        ));
        assert!(parse_stl(b"garbage", ModelLimits::default()).is_err());
    }
}
