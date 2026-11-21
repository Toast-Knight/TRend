//! Model file loading: glTF 2.0 (`.gltf` and `.glb`, with PBR materials), OBJ (with MTL materials
//! and textures), STL (text and binary) and PLY (text and binary).
//!
//! Pictures (textures) are decoded by the caller through an [`ImageDecoder`], so this crate needs no
//! image codecs; without one, models load with their colours and factors only.
//!
//! Every loader is bounded by [`ModelLimits`], checks every index, and never panics on malformed
//! input; a bad file is an error, not a crash.

mod gltf;
mod json;
mod obj;
mod ply;
mod stl;

use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use crate::mesh::{Mesh, Texture};

use gltf::parse_gltf;
use obj::{parse_obj, parse_obj_with};
use ply::parse_ply;
use stl::parse_stl;

/// A model file format. More may be added.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ModelFormat {
    Obj,
    Stl,
    Ply,
    /// glTF 2.0 as JSON, with its buffers and images embedded or in files beside it.
    Gltf,
    /// glTF 2.0 binary: everything in one file.
    Glb,
}

/// Decodes a picture file's bytes (PNG, JPEG, ...) into a texture, or `None` if it cannot.
pub type ImageDecoder<'a> = dyn Fn(&[u8]) -> Option<Texture> + 'a;

impl ModelFormat {
    pub const ALL: [Self; 5] = [Self::Obj, Self::Stl, Self::Ply, Self::Gltf, Self::Glb];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Obj => "OBJ",
            Self::Stl => "STL",
            Self::Ply => "PLY",
            Self::Gltf => "glTF",
            Self::Glb => "GLB",
        }
    }

    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Obj => &["obj"],
            Self::Stl => &["stl"],
            Self::Ply => &["ply"],
            Self::Gltf => &["gltf"],
            Self::Glb => &["glb"],
        }
    }

    /// The format a file extension names (any case), if any.
    pub fn from_extension(extension: &str) -> Option<Self> {
        let extension = extension.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|format| format.extensions().contains(&extension.as_str()))
    }

    /// The format of `head` (the first bytes of a file), from content alone. OBJ has no magic number,
    /// so it is recognised by its usual first statements.
    pub fn sniff(head: &[u8]) -> Option<Self> {
        if head.starts_with(b"ply\n") || head.starts_with(b"ply\r\n") {
            return Some(Self::Ply);
        }
        if head.starts_with(b"glTF") {
            return Some(Self::Glb);
        }
        let text_start = head
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .map_or(&[][..], |start| &head[start..]);
        let text_start = text_start
            .strip_prefix(b"\xEF\xBB\xBF")
            .unwrap_or(text_start);
        if text_start.starts_with(b"{") && contains(head, b"\"asset\"") {
            return Some(Self::Gltf);
        }
        if text_start.starts_with(b"solid") && contains(head, b"facet") {
            return Some(Self::Stl);
        }
        let looks_obj = head.split(|&byte| byte == b'\n').take(64).any(|line| {
            let line = line.trim_ascii_start();
            line.starts_with(b"v ") || line.starts_with(b"f ") || line.starts_with(b"mtllib ")
        });
        if looks_obj && head.iter().take(512).all(|&byte| byte != 0) {
            return Some(Self::Obj);
        }
        None
    }

    /// The format of the file at `path`: its extension, confirmed by content where the format has a
    /// signature. Binary STL has none, so `.stl` files are trusted by extension and checked on load.
    pub fn detect(path: &Path) -> Option<Self> {
        let by_extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .and_then(Self::from_extension);
        let mut head = [0_u8; 1024];
        let read = fs::File::open(path)
            .and_then(|mut file| read_up_to(&mut file, &mut head))
            .ok()?;
        let head = &head[..read];
        match by_extension {
            Some(Self::Ply) => head.starts_with(b"ply").then_some(Self::Ply),
            Some(Self::Stl) => Some(Self::Stl),
            Some(Self::Glb) => head.starts_with(b"glTF").then_some(Self::Glb),
            Some(Self::Gltf) => {
                let text = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
                (text.trim_ascii_start().starts_with(b"{")).then_some(Self::Gltf)
            }
            // OBJ has no signature and may open with long comments: any non-empty text file will do.
            Some(Self::Obj) => (read > 0 && !head.contains(&0)).then_some(Self::Obj),
            None => Self::sniff(head),
        }
    }
}

/// Bounds on what a loader accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelLimits {
    pub max_file_bytes: u64,
    pub max_vertices: usize,
    pub max_triangles: usize,
}

impl Default for ModelLimits {
    /// 512 MiB files, 8 million vertices and 8 million triangles (about 600 MB of mesh at most).
    fn default() -> Self {
        Self {
            max_file_bytes: 512 * 1024 * 1024,
            max_vertices: 8_000_000,
            max_triangles: 8_000_000,
        }
    }
}

/// Why a model could not be loaded.
#[derive(Debug)]
#[non_exhaustive]
pub enum ModelError {
    Io(io::Error),
    /// Not a model format this crate reads.
    UnknownFormat,
    /// The file breaks its format, at `line` (1-based, 0 for binary data).
    Parse {
        line: usize,
        message: String,
    },
    /// The file is bigger than [`ModelLimits`] allow.
    TooLarge(&'static str),
    /// The file holds no triangles.
    Empty,
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::UnknownFormat => formatter.write_str("not a glTF, GLB, OBJ, STL or PLY model"),
            Self::Parse { line: 0, message } => write!(formatter, "invalid model: {message}"),
            Self::Parse { line, message } => {
                write!(formatter, "invalid model at line {line}: {message}")
            }
            Self::TooLarge(what) => write!(formatter, "model too large: {what}"),
            Self::Empty => formatter.write_str("model has no triangles"),
        }
    }
}

impl std::error::Error for ModelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ModelError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub(crate) fn parse_error(line: usize, message: impl Into<String>) -> ModelError {
    ModelError::Parse {
        line,
        message: message.into(),
    }
}

/// Loads the model at `path`, without textures. See [`load_with`].
pub fn load(path: &Path, limits: ModelLimits) -> Result<Mesh, ModelError> {
    load_with(path, limits, &|_| None)
}

/// Loads the model at `path`, decoding its textures with `images`. Files the model refers to (OBJ
/// material libraries and pictures, glTF buffers and images) are read from the model's folder and
/// below it, never above it. The mesh comes back sanitized, with normals, and with tangents when a
/// material has a normal map.
pub fn load_with(
    path: &Path,
    limits: ModelLimits,
    images: &ImageDecoder<'_>,
) -> Result<Mesh, ModelError> {
    let format = ModelFormat::detect(path).ok_or(ModelError::UnknownFormat)?;
    let bytes = read_limited(path, limits.max_file_bytes)?;
    let folder = path.parent().map(Path::to_path_buf);
    let read = |name: &str| -> Option<Vec<u8>> {
        read_beside(folder.as_deref()?, name, limits.max_file_bytes)
    };
    let mut mesh = match format {
        ModelFormat::Obj => {
            let text = String::from_utf8_lossy(&bytes);
            parse_obj_with(
                &text,
                |name| read(name).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
                |name| read(name).and_then(|bytes| images(&bytes)),
                limits,
            )?
        }
        ModelFormat::Stl => parse_stl(&bytes, limits)?,
        ModelFormat::Ply => parse_ply(&bytes, limits)?,
        ModelFormat::Gltf | ModelFormat::Glb => parse_gltf(&bytes, &read, images, limits)?,
    };
    finish(&mut mesh)?;
    Ok(mesh)
}

/// Loads a model from memory. `mtl` resolves OBJ material library names (return `None` to skip).
/// glTF files can only use embedded data here; see [`load_from_bytes_with`].
pub fn load_from_bytes(
    bytes: &[u8],
    format: ModelFormat,
    mut mtl: impl FnMut(&str) -> Option<String>,
    limits: ModelLimits,
) -> Result<Mesh, ModelError> {
    if bytes.len() as u64 > limits.max_file_bytes {
        return Err(ModelError::TooLarge("file size"));
    }
    let mut mesh = match format {
        ModelFormat::Obj => parse_obj(&String::from_utf8_lossy(bytes), &mut mtl, limits)?,
        ModelFormat::Stl => parse_stl(bytes, limits)?,
        ModelFormat::Ply => parse_ply(bytes, limits)?,
        ModelFormat::Gltf | ModelFormat::Glb => parse_gltf(bytes, &|_| None, &|_| None, limits)?,
    };
    finish(&mut mesh)?;
    Ok(mesh)
}

/// Loads a model from memory, with `read` returning the files it refers to by relative name (from
/// an archive, say) and `images` decoding pictures.
pub fn load_from_bytes_with(
    bytes: &[u8],
    format: ModelFormat,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
    images: &ImageDecoder<'_>,
    limits: ModelLimits,
) -> Result<Mesh, ModelError> {
    if bytes.len() as u64 > limits.max_file_bytes {
        return Err(ModelError::TooLarge("file size"));
    }
    let mut mesh = match format {
        ModelFormat::Obj => parse_obj_with(
            &String::from_utf8_lossy(bytes),
            |name| read(name).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            |name| read(name).and_then(|bytes| images(&bytes)),
            limits,
        )?,
        ModelFormat::Stl => parse_stl(bytes, limits)?,
        ModelFormat::Ply => parse_ply(bytes, limits)?,
        ModelFormat::Gltf | ModelFormat::Glb => parse_gltf(bytes, read, images, limits)?,
    };
    finish(&mut mesh)?;
    Ok(mesh)
}

/// Reads `name` (a relative path with `/` or `\\` separators) inside `folder`. Paths that would
/// leave the folder are refused; a path that does not exist falls back to its bare file name, since
/// exporters often write the artist's absolute paths.
fn read_beside(folder: &Path, name: &str, max_bytes: u64) -> Option<Vec<u8>> {
    let name = name.trim().trim_matches('"');
    let parts: Vec<&str> = name
        .split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let file_name = *parts.last()?;
    let relative_ok = !name.starts_with(['/', '\\'])
        && !parts.first().is_some_and(|part| part.contains(':'))
        && !parts.contains(&"..");
    if relative_ok {
        let path = parts
            .iter()
            .fold(folder.to_path_buf(), |path, part| path.join(part));
        if let Ok(bytes) = read_limited(&path, max_bytes) {
            return Some(bytes);
        }
    }
    if file_name == ".." || file_name.contains(':') {
        return None;
    }
    read_limited(&folder.join(file_name), max_bytes).ok()
}

fn finish(mesh: &mut Mesh) -> Result<(), ModelError> {
    mesh.sanitize();
    if mesh.is_empty() {
        return Err(ModelError::Empty);
    }
    mesh.ensure_normals();
    if mesh
        .materials
        .iter()
        .any(|material| material.normal_texture.is_some())
    {
        mesh.ensure_tangents();
    }
    Ok(())
}

fn read_limited(path: &Path, max_bytes: u64) -> Result<Vec<u8>, ModelError> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > max_bytes {
        return Err(ModelError::TooLarge("file size"));
    }
    let mut bytes = Vec::new();
    file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(ModelError::TooLarge("file size"));
    }
    Ok(bytes)
}

fn read_up_to(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    Ok(filled)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Checks a count against a limit before allocating for it.
pub(crate) fn check_limit(count: usize, max: usize, what: &'static str) -> Result<(), ModelError> {
    if count > max {
        Err(ModelError::TooLarge(what))
    } else {
        Ok(())
    }
}

/// Splits a polygon into a triangle fan, the way OBJ and PLY faces are meant to be read.
pub(crate) fn fan(polygon: &[u32], mut emit: impl FnMut([u32; 3])) {
    for index in 1..polygon.len().saturating_sub(1) {
        emit([polygon[0], polygon[index], polygon[index + 1]]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_recognised_by_extension_and_content() {
        assert_eq!(ModelFormat::from_extension("OBJ"), Some(ModelFormat::Obj));
        assert_eq!(ModelFormat::from_extension("png"), None);
        assert_eq!(
            ModelFormat::sniff(b"ply\nformat ascii 1.0\n"),
            Some(ModelFormat::Ply)
        );
        assert_eq!(
            ModelFormat::sniff(b"solid x\n facet normal 0 0 1\n"),
            Some(ModelFormat::Stl)
        );
        assert_eq!(
            ModelFormat::sniff(b"# Blender\nmtllib a.mtl\no Cube\nv 1 1 1\n"),
            Some(ModelFormat::Obj)
        );
        assert_eq!(ModelFormat::sniff(b"\x89PNG\r\n"), None);
        assert_eq!(ModelFormat::sniff(b"hello world"), None);
    }

    #[test]
    fn detection_checks_content_behind_the_extension() {
        let folder = std::env::temp_dir().join(format!("trend-3d-detect-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let fake = folder.join("fake.ply");
        fs::write(&fake, b"not a ply").unwrap();
        assert_eq!(ModelFormat::detect(&fake), None);
        let real = folder.join("real.obj");
        fs::write(&real, b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        assert_eq!(ModelFormat::detect(&real), Some(ModelFormat::Obj));
        let unnamed = folder.join("model.bin");
        fs::write(&unnamed, b"ply\nformat ascii 1.0\nend_header\n").unwrap();
        assert_eq!(ModelFormat::detect(&unnamed), Some(ModelFormat::Ply));
        let mesh = load(&real, ModelLimits::default()).unwrap();
        assert_eq!(mesh.triangles.len(), 1);
        assert_eq!(mesh.corner_normals.len(), 3);
        let small = ModelLimits {
            max_file_bytes: 4,
            ..ModelLimits::default()
        };
        assert!(matches!(load(&real, small), Err(ModelError::TooLarge(_))));
        fs::remove_dir_all(folder).unwrap();
    }
}
