# trend-3d

Optional, dependency-free software 3D for TRend. The `trend` core stays free of 3D: this crate is one
way for an application to make pixels from a 3D model, which TRend then turns into terminal cells like
any other picture.

- **Loading:** glTF 2.0 (`.gltf` with its `.bin` and pictures, or embedded data; `.glb`), OBJ (with
  MTL materials: colours, opacity, `map_Kd`, `Ke`/`map_Ke`, `map_Bump`/`norm`, `Pr`/`map_Pr`,
  `Pm`/`map_Pm`; texture coordinates; `v x y z r g b` vertex colours), STL (text and binary) and PLY
  (text, binary little- and big-endian; vertex normals, vertex and face colours). Every loader is
  bounded by `ModelLimits`, checks every index and returns an error rather than panicking on
  malformed files. Formats are detected by extension and checked by content. Files a model refers
  to are read from its folder and below, never above it.
- **No image codecs:** pictures are decoded by the application through an `ImageDecoder`
  (`load_with`, `load_from_bytes_with`), so this crate needs nothing but `trend`. Without one, models
  load with their colours and material factors.
- **glTF:** the default scene's node tree with TRS and matrix transforms (mirroring transforms flip
  the winding), triangle lists, strips and fans, indexed or not, all accessor component types
  (normalized or not, strided), positions, normals, tangents, texture coordinates and vertex
  colours, metallic-roughness materials with base colour, normal, emissive, metallic-roughness and
  occlusion textures, `MASK`/`BLEND` alpha, `KHR_materials_emissive_strength` and
  `KHR_materials_unlit`. Skins and animations are ignored (rest pose); Draco and sparse accessors
  are not supported. A small strict JSON parser is built in.
- **Meshes:** indexed triangles with per-corner normals and tangents, so hard edges and smooth
  surfaces both survive. Missing normals are computed with a crease angle (angle-weighted); missing
  tangents are derived from texture coordinates when a material has a normal map. STL corners are
  welded.
- **Camera:** `OrbitCamera` frames a model by its bounding sphere and orbits, zooms and pans; Y-up or
  Z-up (STL from CAD is usually Z-up). `OrbitCamera::first_person` places the eye directly, for
  games and walk-throughs.
- **Materials:** physically based: base colour and texture, metallic, roughness (with a glTF
  metallic-roughness texture), tangent-space normal maps (OpenGL or DirectX green), emissive colour
  and texture (may exceed 1), ambient occlusion, alpha test cut-outs, dithered alpha blending (no
  sorting needed), and unlit surfaces. Textures are perspective-correct and repeating, bilinear
  or crisp (`Material::pixelated`, for pixel-art and block games).
- **Environments:** `Environment` is an image-based light and sky box: built-in presets (Studio,
  Sky, Sunset, Night, Neon, Overcast), Radiance `.hdr` panoramas (`from_hdr`), or any decoded
  equirectangular picture (`from_texture`). It lights the model (diffuse irradiance) and shows in
  shiny surfaces (reflections blurred by roughness); it can be turned and scaled, and drawn as the
  background, sharp or blurred.
- **Rendering:** `Renderer` draws straight into a `trend::PixelViewMut` at the size and pixel aspect
  of a `RasterSpec`, so models keep their proportions with every encoder. Perspective, near-plane
  clipping, a visibility buffer, then deferred per-pixel shading in linear light: Lambert diffuse,
  normalized Blinn-Phong specular with Schlick Fresnel, the environment, and a key light whose colour,
  strength and direction are set in `Light`; ACES tone mapping and exposure. Optional distance
  `Fog`, to a colour or into the background behind each pixel. Backgrounds: transparent,
  solid, gradient or the environment. Three modes: shaded, shaded with edges, and hidden-line
  wireframe. Triangle setup and shading are split over threads; triangles that miss every pixel
  centre are dropped early.

```rust
// `decode` turns PNG/JPEG bytes into a `Texture` (with the `image` crate, say), or returns None.
let mesh = trend_3d::load_with(path, ModelLimits::default(), &decode)?;
let style = RenderStyle {
    environment: Some(Arc::new(Environment::preset(EnvironmentPreset::Sunset))),
    background: Background::Environment { blur: 0.3 },
    ..RenderStyle::default()
};
let camera = OrbitCamera::framing_mesh(&mesh, UpAxis::Y).unwrap();
let mut renderer = Renderer::with_threads(4);
surface.draw(|pixels, spec| {
    renderer.render(&mesh, &camera, &style, pixels, spec.pixel_aspect());
})?;
```

## Example

```text
cargo run --release -p trend-3d --example model_view -- MODEL.obj|stl|ply
    [--encoder half|quadrant|sextant|braille] [--size COLSxROWS] [--frames N] [--once]
    [--mode shaded|edges|wire] [--flat] [--z-up]
```

Without a model it spins a built-in torus.

```text
cargo run --release -p trend-3d --example render_image -- MODEL|--spheres OUT.ppm
    [--size WxH] [--environment NAME|none] [--sky] [--yaw RADIANS] [--exposure X]
```

Renders one picture to a PPM file; `--spheres` draws rows of metal and plastic balls from glossy to
rough, and a glowing one, to compare environments. (Without an image decoder it shows textured
models in their material colours.)

## Measured

On a 2-core cloud VM, a 1,000,000-triangle PLY:

| Step | Time |
| --- | --- |
| Load (parse and compute normals) | 268 ms |
| Frame at 200x60 cells, half block (200x120 px) | 45 ms |
| Frame at 200x60 cells, sextant (400x180 px) | 86 ms |

Small models (a few thousand triangles) render in 0.3 to 4 ms per frame.

## Not included

Shadows, skinning and animation, morph targets, Draco-compressed glTF, `KHR_texture_transform`
(read but not applied), second texture coordinate sets, and image decoding (by design: the
application supplies it).
Applications with a GPU renderer don't need this crate: they hand TRend their frames directly
(see `docs/readback-recipes.md`).
