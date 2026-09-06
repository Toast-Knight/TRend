#![deny(unsafe_code)]

pub mod cache;
pub mod cell;
pub mod color;
pub mod dither;
pub mod experimental;
#[allow(unsafe_code)]
pub mod ffi;
pub mod framebuffer;
pub mod geometry;
pub mod queue;
pub mod renderer;
pub mod resize;
pub mod scale;
pub mod terminal;
pub mod timing;

pub use cache::{CacheInsert, CellFrameCache};
pub use cell::{Cell, CellFrame, Color};
pub use framebuffer::{FrameError, PixelFormat, PixelFrameView};
pub use geometry::{Alignment, FitMode, GeometryError, RenderGeometry, RenderTransform, Viewport};
pub use queue::BoundedQueue;
pub use renderer::{RenderError, RenderOptions, RenderRequest, RenderStatus, Renderer};
pub use resize::ResizeCoalescer;
pub use scale::SamplingFilter;
pub use terminal::{
    AnsiPresenter, PositionedRenderOutput, PositionedRenderRequest, TerminalCapabilities,
    TerminalColorMode, TerminalEnvironment,
};
pub use timing::{
    AnimationPosition, AnimationTimeline, FrameDecision, PlaybackClock, TimelineError,
};
