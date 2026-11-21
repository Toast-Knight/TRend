//! Optional, dependency-free software 3D for TRend.
//!
//! TRend's core turns pixels into terminal cells and stays free of 3D. This crate is one way to
//! make those pixels from a 3D model, entirely on the CPU and with no dependencies besides `trend`:
//!
//! - [`load`] reads OBJ (with MTL colours), STL (text and binary) and PLY (text and binary) into a
//!   [`Mesh`], bounded by [`ModelLimits`];
//! - [`OrbitCamera`] frames and orbits a model;
//! - [`Renderer`] rasterizes it with a depth buffer, perspective, near-plane clipping, smooth or flat
//!   lighting and optional edges, straight into a [`trend::PixelViewMut`] at the size and pixel
//!   aspect a [`trend::RasterSpec`] asks for, so models keep their proportions with every encoder.
//!
//! ```no_run
//! use trend::{CellAspect, HalfBlock, Surface, TerminalCapabilities, Viewport};
//! use trend_3d::{ModelLimits, OrbitCamera, RenderStyle, Renderer, UpAxis};
//!
//! let mesh = trend_3d::load("bunny.obj".as_ref(), ModelLimits::default())?;
//! let camera = OrbitCamera::framing_mesh(&mesh, UpAxis::Y).unwrap();
//! let mut renderer = Renderer::with_threads(4);
//! let mut surface = Surface::new(HalfBlock, Viewport { width: 80, height: 24 },
//!     TerminalCapabilities::detect())?;
//! surface.draw(|pixels, spec| {
//!     renderer.render(&mesh, &camera, &RenderStyle::default(), pixels, spec.pixel_aspect());
//! })?;
//! let _ = CellAspect::default();
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Applications that already have a GPU renderer don't need this crate: they hand TRend their
//! frames directly. See `docs/readback-recipes.md` in the TRend repository.

#![forbid(unsafe_code)]

mod camera;
mod color;
mod environment;
mod load;
mod math;
mod mesh;
mod raster;

pub use camera::{OrbitCamera, UpAxis, ViewBasis};
pub use environment::{Environment, EnvironmentPreset};
pub use load::{
    ImageDecoder, ModelError, ModelFormat, ModelLimits, load, load_from_bytes,
    load_from_bytes_with, load_with, parse_gltf, parse_obj, parse_obj_with, parse_ply, parse_stl,
};
pub use math::Vec3;
pub use mesh::{Bounds, DEFAULT_CREASE_DEGREES, Material, Mesh, MeshStats, Texture};
pub use raster::{Background, Fog, FrameStats, Light, RenderMode, RenderStyle, Renderer, Shading};
