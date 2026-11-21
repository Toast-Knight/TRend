#![deny(unsafe_code)]

pub mod cache;
pub mod cell;
pub mod color;
pub mod compose;
pub mod dither;
pub mod encode;
pub mod experimental;
#[allow(unsafe_code)]
pub mod ffi;
pub mod framebuffer;
pub mod geometry;
pub mod pixel_buffer;
pub mod queue;
pub mod raster;
pub mod renderer;
pub mod resize;
pub mod scale;
pub mod surface;
pub mod terminal;
pub mod text;
pub mod timing;

pub use cache::{CacheInsert, CellFrameCache};
pub use cell::{Cell, CellFrame, CellRowBands, CellView, CellViewMut, Color};
pub use encode::{
    Braille, CellEncoder, ColorUse, Density, DensityRamp, EncodeError, EncoderRequirements,
    GlyphRepertoire, HalfBlock, Quadrant, Sextant,
};
pub use framebuffer::{
    AlphaMode, FrameError, PixelFormat, PixelFrameView, PixelLayout, PixelRowBands, PixelViewMut,
    RowOrder,
};
pub use geometry::{
    Alignment, CellAspect, CellRect, FitMode, GeometryError, PixelAspect, PixelRect, RasterSpec,
    RegionError, RenderGeometry, RenderTransform, SubcellGrid, Viewport,
};
pub use pixel_buffer::PixelBuffer;
pub use queue::BoundedQueue;
pub use raster::{ImageSource, SourceMapping};
pub use renderer::{RenderError, RenderOptions, RenderRequest, RenderStatus, Renderer};
pub use resize::ResizeCoalescer;
pub use scale::SamplingFilter;
pub use surface::Surface;
pub use terminal::{
    AnsiPresenter, OutputOptions, PositionedRenderOutput, PositionedRenderRequest, PresentStats,
    TerminalCapabilities, TerminalColorMode, TerminalEnvironment,
};
pub use timing::{
    AnimationPosition, AnimationTimeline, FrameDecision, PlaybackClock, TimelineError,
};
