use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use trend::{
    AnsiPresenter, CellFrame, FitMode, PixelFrameView, PlaybackClock, RenderOptions, RenderRequest,
    Renderer, SamplingFilter, TerminalColorMode, Viewport,
};
#[cfg(feature = "webp")]
use trend_animation::decode_webp as decode_animated_webp;
use trend_animation::{DecodeLimits as AnimationLimits, DecodedAnimation, decode_gif};
#[cfg(feature = "webp")]
use trend_image::{DecodeError as ImageError, decode_webp as decode_static_webp};
use trend_image::{DecodeLimits as ImageLimits, decode_jpeg, decode_png};
use trend_media::{MediaLimits, MediaSource};
use trend_term::{Event, Key, KeyState, Session, SessionOptions};
use trend_video::{Container, DecodeLimits as VideoLimits, VideoDecoder};

/// Takes over the terminal for viewing. The viewer reads single key presses only, so it does not
/// ask for key releases.
fn enter_session() -> io::Result<Session> {
    Session::enter(SessionOptions::new().with_key_releases(false))
}

/// The cells the media may fill: the terminal less the status line's rows, and less the last
/// column, which some consoles wrap on.
fn media_viewport(session: &Session) -> Viewport {
    let terminal = session.info().viewport;
    Viewport {
        width: terminal.width.saturating_sub(1).max(1),
        height: terminal.height.saturating_sub(2).max(1),
    }
}

/// The next key press, waiting up to `wait` for it. Returns early with `None` when the terminal
/// is resized, so the caller redraws at the new size at once. Ctrl+C reads as `Q`.
fn next_key(session: &mut Session, wait: Duration) -> io::Result<Option<Key>> {
    let deadline = Instant::now() + wait;
    loop {
        match session.poll(deadline.saturating_duration_since(Instant::now()))? {
            Some(Event::Key {
                key,
                modifiers,
                state: KeyState::Press | KeyState::Repeat,
            }) => {
                let interrupt = modifiers.control && key == Key::Char('c');
                return Ok(Some(if interrupt { Key::Char('q') } else { key }));
            }
            Some(Event::Resize(_)) | None => return Ok(None),
            Some(_) => {}
        }
    }
}

#[derive(Clone, Copy)]
struct ViewSettings {
    fit: FitMode,
    quality: QualityMode,
    compare_all: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            fit: FitMode::Contain,
            quality: QualityMode::Quality,
            compare_all: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QualityMode {
    Fast,
    Balanced,
    Quality,
}

impl QualityMode {
    const ALL: [Self; 3] = [Self::Fast, Self::Balanced, Self::Quality];

    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().replace([' ', '_'], "-").as_str() {
            "fast" => Some(Self::Fast),
            "balanced" => Some(Self::Balanced),
            "quality" => Some(Self::Quality),
            _ => None,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Fast => "FAST",
            Self::Balanced => "BALANCED",
            Self::Quality => "QUALITY",
        }
    }

    const fn sampling(self) -> SamplingFilter {
        match self {
            Self::Fast => SamplingFilter::Nearest,
            Self::Balanced => SamplingFilter::Bilinear,
            Self::Quality => SamplingFilter::Area,
        }
    }

    fn next(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

const HELP: &str = "TRend Test Viewer

Render images, animations, and video as terminal cells and ANSI output.

Usage:
    trend-test-viewer <MEDIA>
    trend-test-viewer --quality <MODE> <MEDIA>
    trend-test-viewer --compare-all <MEDIA>
    trend-test-viewer --probe <WIDTH>x<HEIGHT> <ITERATIONS> <MEDIA>
    trend-test-viewer --supports <FORMAT>

Options:
    --quality <MODE>     fast | balanced | quality
    --compare-all        Start paused in comparison mode
    --probe              Benchmark deterministic rendering without interactive output
    --supports <FORMAT>  Exit 0 when the compiled viewer supports the extension
    -h, --help           Show this help
    -V, --version        Show the package version

Interactive controls:
    Q or Esc             Exit (Esc applies to video)
    1 / 2 / 3            Select fast, balanced, or quality rendering
    M                    Cycle rendering quality
    C / V / S            Select contain, cover, or stretch fitting
    Space                Pause or resume animation/video
    R                    Restart animation/video
    Left / Right         Seek animation/video

Examples:
    trend-test-viewer image.png
    trend-test-viewer --quality quality animation.gif
    trend-test-viewer --probe 80x24 10 video.mp4";

fn main() {
    if let Err(error) = run() {
        eprintln!("TRend viewer error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let first = arguments
        .next()
        .ok_or("missing media path; run trend-test-viewer --help")?;
    if first == "-h" || first == "--help" {
        println!("{HELP}");
        return Ok(());
    }
    if first == "-V" || first == "--version" {
        println!("trend-test-viewer {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if first == "--supports" {
        let extension = arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .unwrap_or_default();
        return if supports_extension(extension.trim_start_matches('.')) {
            Ok(())
        } else {
            Err("unsupported media format".into())
        };
    }
    if first == "--probe" {
        let viewport = arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .and_then(|value| parse_viewport(&value))
            .ok_or("probe viewport must be WIDTHxHEIGHT")?;
        let iterations = arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|iterations| *iterations > 0)
            .ok_or("probe iterations must be greater than zero")?;
        let path = arguments.next().ok_or("missing probe media file")?;
        return probe_media(&std::path::PathBuf::from(path), viewport, iterations);
    }
    let first = first
        .into_string()
        .map_err(|_| "viewer arguments must be valid Unicode")?;
    let (path, settings) = if first == "--quality" {
        let mode = arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .and_then(|value| QualityMode::parse(&value))
            .ok_or("unknown or missing quality mode")?;
        let path = arguments.next().ok_or("missing media file")?;
        (
            std::path::PathBuf::from(path),
            ViewSettings {
                quality: mode,
                ..ViewSettings::default()
            },
        )
    } else if first == "--compare-all" {
        let path = arguments.next().ok_or("missing media file")?;
        (
            std::path::PathBuf::from(path),
            ViewSettings {
                quality: QualityMode::Fast,
                compare_all: true,
                ..ViewSettings::default()
            },
        )
    } else {
        (std::path::PathBuf::from(first), ViewSettings::default())
    };
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => {
            let bytes = fs::read(&path)?;
            let image = decode_png(&bytes, ImageLimits::default())?;
            view_static(&path, image.view(Duration::ZERO)?, settings)
        }
        "jpg" | "jpeg" => {
            let bytes = fs::read(&path)?;
            let image = decode_jpeg(&bytes, ImageLimits::default())?;
            view_static(&path, image.view(Duration::ZERO)?, settings)
        }
        #[cfg(feature = "webp")]
        "webp" => {
            let bytes = fs::read(&path)?;
            match decode_static_webp(&bytes, ImageLimits::default()) {
                Ok(image) => view_static(&path, image.view(Duration::ZERO)?, settings),
                Err(ImageError::UnsupportedAnimation) => view_animation(
                    &path,
                    decode_animated_webp(&bytes, AnimationLimits::default(), Duration::ZERO)?,
                    settings,
                ),
                Err(error) => Err(error.into()),
            }
        }
        "gif" => {
            let bytes = fs::read(&path)?;
            let animation = decode_gif(&bytes, AnimationLimits::default(), Duration::ZERO)?;
            view_animation(&path, animation, settings)
        }
        _ if supports_video_extension(&extension) => view_video(
            &path,
            VideoDecoder::open(&path, VideoLimits::default())?,
            settings,
        ),
        _ => Err(format!("unsupported media format: .{extension}").into()),
    }
}

fn parse_viewport(value: &str) -> Option<Viewport> {
    let (width, height) = value.split_once(['x', 'X'])?;
    Some(Viewport {
        width: width.parse().ok()?,
        height: height.parse().ok()?,
    })
    .filter(|viewport| viewport.width > 0 && viewport.height > 0)
}

fn probe_media(path: &Path, viewport: Viewport, iterations: usize) -> Result<(), Box<dyn Error>> {
    let mut source = MediaSource::open(path, MediaLimits::default())?;
    let frame = source
        .representative_frame()?
        .ok_or("media did not produce a representative frame")?;
    probe_source(path, frame, viewport, iterations)
}

fn probe_source(
    path: &Path,
    source: PixelFrameView<'_>,
    viewport: Viewport,
    iterations: usize,
) -> Result<(), Box<dyn Error>> {
    println!(
        "probe={} source={}x{} viewport={}x{} iterations={iterations}",
        file_name(path),
        source.width(),
        source.height(),
        viewport.width,
        viewport.height
    );
    println!(
        "mode,median_render_us,min_render_us,ansi_generation_us,ansi_bytes,deterministic,fingerprint,glyphs"
    );
    for quality in QualityMode::ALL {
        let settings = ViewSettings {
            fit: FitMode::Contain,
            quality,
            compare_all: false,
        };
        let mut state = ViewerState::new();
        state.render(source, viewport, settings)?;
        let mut timings = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let started = Instant::now();
            state.render(source, viewport, settings)?;
            timings.push(started.elapsed());
        }
        timings.sort_unstable();
        let median = timings[timings.len() / 2];
        let minimum = timings[0];
        let cells_fingerprint = cell_fingerprint(&state.cells);
        let glyphs = glyph_usage(&state.cells);
        let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
        let present_started = Instant::now();
        let ansi = presenter.present(&state.cells);
        let present_time = present_started.elapsed();
        let ansi_bytes = ansi.len();
        let ansi_fingerprint = byte_fingerprint(ansi);

        let mut repeated = ViewerState::new();
        repeated.render(source, viewport, settings)?;
        let repeated_cells_fingerprint = cell_fingerprint(&repeated.cells);
        let mut repeated_presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
        let repeated_ansi_fingerprint =
            byte_fingerprint(repeated_presenter.present(&repeated.cells));
        let deterministic = cells_fingerprint == repeated_cells_fingerprint
            && ansi_fingerprint == repeated_ansi_fingerprint;
        println!(
            "{},{:.3},{:.3},{:.3},{ansi_bytes},{deterministic},{cells_fingerprint:016x},{glyphs}",
            quality.name(),
            median.as_secs_f64() * 1_000_000.0,
            minimum.as_secs_f64() * 1_000_000.0,
            present_time.as_secs_f64() * 1_000_000.0,
        );
    }
    Ok(())
}

fn cell_fingerprint(frame: &CellFrame) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for cell in frame.cells() {
        for byte in cell
            .glyph
            .to_le_bytes()
            .into_iter()
            .chain([
                cell.foreground.red,
                cell.foreground.green,
                cell.foreground.blue,
            ])
            .chain([
                cell.background.red,
                cell.background.green,
                cell.background.blue,
            ])
        {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn byte_fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn glyph_usage(frame: &CellFrame) -> String {
    let mut counts = BTreeMap::new();
    for cell in frame.cells() {
        *counts
            .entry(char::from_u32(cell.glyph).unwrap_or('�'))
            .or_insert(0_usize) += 1;
    }
    counts
        .into_iter()
        .map(|(glyph, count)| format!("U+{:04X}:{count}", glyph as u32))
        .collect::<Vec<_>>()
        .join("|")
}

fn supports_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif"
    ) || cfg!(feature = "webp") && extension.eq_ignore_ascii_case("webp")
        || supports_video_extension(extension)
}

fn supports_video_extension(extension: &str) -> bool {
    Container::from_extension(extension).is_some_and(|container| {
        video_container_enabled(container)
            && VideoDecoder::capabilities().supports_container(container)
    })
}

fn video_container_enabled(container: Container) -> bool {
    match container {
        Container::Mp4 => true,
        Container::WebM => cfg!(feature = "webm"),
        Container::Mkv => cfg!(feature = "mkv"),
        Container::Avi => cfg!(feature = "avi"),
        Container::Mov => cfg!(feature = "mov"),
    }
}

fn view_static(
    path: &Path,
    source: PixelFrameView<'_>,
    mut settings: ViewSettings,
) -> Result<(), Box<dyn Error>> {
    let mut session = enter_session()?;
    let mut state = ViewerState::new();
    let mut dirty = true;

    loop {
        let viewport = media_viewport(&session);
        if dirty || state.viewport != Some(viewport) {
            state.render(source, viewport, settings)?;
            state.present(
                &mut session,
                &format!(
                    "{} | PNG {}x{} | fit: {} | {} | [1-3] quality [M] next [C/V/S] fit [Q] back",
                    file_name(path),
                    source.width(),
                    source.height(),
                    fit_name(settings.fit),
                    quality_label(settings)
                ),
            )?;
            dirty = false;
        }
        if let Some(key) = next_key(&mut session, Duration::from_millis(30))? {
            dirty = update_settings(key, &mut settings);
            if matches!(key, Key::Char('q')) {
                return Ok(());
            }
        }
    }
}

fn view_animation(
    path: &Path,
    mut animation: DecodedAnimation,
    mut settings: ViewSettings,
) -> Result<(), Box<dyn Error>> {
    let mut session = enter_session()?;
    let started = Instant::now();
    let mut state = ViewerState::new();
    let mut paused = settings.compare_all;
    if paused {
        animation.pause(Duration::ZERO);
    }
    let mut last_frame = usize::MAX;
    let mut dirty = true;

    loop {
        let now = started.elapsed();
        let position = animation.position(now);
        let viewport = media_viewport(&session);
        if dirty || state.viewport != Some(viewport) || last_frame != position.frame_index {
            state.render(
                animation.frame_at(position.frame_index)?,
                viewport,
                settings,
            )?;
            state.present(&mut session, &format!(
                "{} | GIF frame {}/{} loop {} | {} | {} | [Space] pause [R] restart [Left/Right] seek [1-3] quality [M] next [Q] back",
                file_name(path),
                position.frame_index + 1,
                animation.frame_count(),
                position.loop_index + 1,
                if paused { "paused" } else { "playing" },
                quality_label(settings)
            ))?;
            last_frame = position.frame_index;
            dirty = false;
        }

        if let Some(key) = next_key(&mut session, Duration::from_millis(10))? {
            match key {
                Key::Char('q') => return Ok(()),
                Key::Char(' ') => {
                    if paused {
                        animation.play(now);
                    } else {
                        animation.pause(now);
                    }
                    paused = !paused;
                    dirty = true;
                }
                Key::Char('r') => {
                    animation.restart(now);
                    dirty = true;
                }
                Key::Left | Key::Right => {
                    let current = animation.position(now).frame_index;
                    let target = if matches!(key, Key::Left) {
                        current
                            .checked_sub(1)
                            .unwrap_or(animation.frame_count() - 1)
                    } else {
                        (current + 1) % animation.frame_count()
                    };
                    animation.seek_frame(target, now)?;
                    dirty = true;
                }
                other => dirty = update_settings(other, &mut settings),
            }
        }
    }
}

fn view_video(
    path: &Path,
    mut decoder: VideoDecoder,
    mut settings: ViewSettings,
) -> Result<(), Box<dyn Error>> {
    let mut session = enter_session()?;
    let started = Instant::now();
    let mut clock = PlaybackClock::new(Duration::ZERO);
    if settings.compare_all {
        clock.pause(Duration::ZERO);
    }
    let mut state = ViewerState::new();
    let mut paused = settings.compare_all;
    let mut last_timestamp = None;
    let mut dirty = true;

    loop {
        let now = started.elapsed();
        let media_time = clock.media_time(now).min(decoder.info().duration);
        let timestamp = decoder
            .frame_for_time(media_time)?
            .map(|frame| frame.timestamp());
        let viewport = media_viewport(&session);
        if dirty || state.viewport != Some(viewport) || timestamp != last_timestamp {
            if let Some(frame) = decoder.frame_for_time(media_time)? {
                state.render(frame.view()?, viewport, settings)?;
                last_timestamp = Some(frame.timestamp());
            }
            let info = decoder.info();
            state.present(&mut session, &format!(
                "{} | VIDEO {}x{} {} | {}/{} | {} | {} | decoder: {} | audio: {} | dropped: {} buffered: {} | [Space] pause [R] restart [Left/Right] seek [1-3] quality [M] next [Q/Esc] back",
                file_name(path),
                info.width,
                info.height,
                format_frame_rate(info.nominal_frame_rate),
                format_time(media_time),
                format_time(info.duration),
                if paused { "paused" } else { "playing" },
                quality_label(settings),
                info.decoder,
                if info.audio_present { "present" } else { "none" },
                decoder.dropped_frames(),
                decoder.buffered_frames(),
            ))?;
            dirty = false;
        }

        if let Some(key) = next_key(&mut session, Duration::from_millis(5))? {
            match key {
                Key::Char('q') | Key::Esc => return Ok(()),
                Key::Char(' ') => {
                    if paused {
                        clock.play(now);
                    } else {
                        clock.pause(now);
                    }
                    paused = !paused;
                    dirty = true;
                }
                Key::Char('r') => {
                    decoder.restart()?;
                    clock.seek(Duration::ZERO, now);
                    dirty = true;
                }
                Key::Left | Key::Right => {
                    let offset = Duration::from_secs(5);
                    let target = if matches!(key, Key::Left) {
                        media_time.saturating_sub(offset)
                    } else {
                        media_time
                            .saturating_add(offset)
                            .min(decoder.info().duration)
                    };
                    decoder.seek(target)?;
                    clock.seek(target, now);
                    dirty = true;
                }
                other => dirty = update_settings(other, &mut settings),
            }
        }
    }
}

struct ViewerState {
    renderer: Renderer,
    presenter: AnsiPresenter,
    cells: CellFrame,
    viewport: Option<Viewport>,
    generation: u64,
    last_render: Duration,
    /// The frame and its status line, written to the terminal in one piece.
    output: Vec<u8>,
}

impl ViewerState {
    fn new() -> Self {
        Self {
            renderer: Renderer::new(),
            presenter: AnsiPresenter::new(TerminalColorMode::TrueColor),
            cells: CellFrame::default(),
            viewport: None,
            generation: 0,
            last_render: Duration::ZERO,
            output: Vec::new(),
        }
    }

    fn render(
        &mut self,
        source: PixelFrameView<'_>,
        viewport: Viewport,
        settings: ViewSettings,
    ) -> Result<(), Box<dyn Error>> {
        self.generation = self.generation.wrapping_add(1);
        let request = RenderRequest {
            viewport,
            fit: settings.fit,
            generation: self.generation,
        };
        let started = Instant::now();
        self.renderer.render_with_options_into(
            source,
            request,
            RenderOptions {
                sampling: settings.quality.sampling(),
                ..RenderOptions::default()
            },
            &mut self.cells,
        )?;
        self.last_render = started.elapsed();
        self.viewport = Some(viewport);
        Ok(())
    }

    fn present(&mut self, session: &mut Session, status: &str) -> io::Result<()> {
        let ansi = self.presenter.present(&self.cells);
        let ansi_bytes = ansi.len();
        self.output.clear();
        self.output.extend_from_slice(ansi);
        let measurements = format!(
            " | viewport {}x{} | render {:.3} ms | ANSI {} B",
            self.cells.width(),
            self.cells.height(),
            self.last_render.as_secs_f64() * 1_000.0,
            ansi_bytes
        );
        // The rows under the picture hold the status line. Text that ran past them would
        // scroll the whole screen up by a line on every frame.
        let terminal = session.info().viewport;
        let rows = usize::from(terminal.height.saturating_sub(self.cells.height()));
        let capacity = (rows * usize::from(terminal.width)).saturating_sub(1);
        write!(
            self.output,
            "\x1b[{};1H\x1b[0m\x1b[J{}",
            self.cells.height() + 1,
            fit_status(status, &measurements, capacity)
        )?;
        session.write_frame(&self.output)?;
        Ok(())
    }
}

/// The status line cut to `capacity` characters: the measurements are kept whole where they fit,
/// and the description before them (which ends with the key hints) gives way first.
fn fit_status(description: &str, measurements: &str, capacity: usize) -> String {
    let kept = capacity.saturating_sub(measurements.chars().count());
    description
        .chars()
        .take(kept)
        .chain(measurements.chars())
        .take(capacity)
        .collect()
}

fn update_settings(key: Key, settings: &mut ViewSettings) -> bool {
    match key {
        Key::Char('c') => settings.fit = FitMode::Contain,
        Key::Char('v') => settings.fit = FitMode::Cover,
        Key::Char('s') => settings.fit = FitMode::Stretch,
        Key::Char('1') => settings.quality = QualityMode::Fast,
        Key::Char('2') => settings.quality = QualityMode::Balanced,
        Key::Char('3') => settings.quality = QualityMode::Quality,
        Key::Char('m') => settings.quality = settings.quality.next(),
        Key::Char('n') => settings.quality = QualityMode::Fast,
        Key::Char('b') => settings.quality = QualityMode::Balanced,
        Key::Char('a') => settings.quality = QualityMode::Quality,
        _ => return false,
    }
    true
}

fn format_frame_rate(frame_rate: Option<(u32, u32)>) -> String {
    frame_rate.map_or_else(
        || "variable FPS".into(),
        |(numerator, denominator)| {
            format!("{:.2} FPS", f64::from(numerator) / f64::from(denominator))
        },
    )
}

fn format_time(duration: Duration) -> String {
    let milliseconds = duration.as_millis();
    format!(
        "{:02}:{:02}.{:03}",
        milliseconds / 60_000,
        milliseconds / 1_000 % 60,
        milliseconds % 1_000
    )
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("media")
}

const fn fit_name(fit: FitMode) -> &'static str {
    match fit {
        FitMode::Contain => "contain",
        FitMode::Cover => "cover",
        FitMode::Stretch => "stretch",
    }
}

fn quality_label(settings: ViewSettings) -> String {
    let comparison = if settings.compare_all {
        " | COMPARE ALL"
    } else {
        ""
    };
    format!("quality: {}{}", settings.quality.name(), comparison)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_documents_every_top_level_mode() {
        for option in [
            "--quality",
            "--compare-all",
            "--probe",
            "--supports",
            "--help",
            "--version",
        ] {
            assert!(HELP.contains(option));
        }
    }

    #[test]
    fn status_line_never_exceeds_its_rows() {
        let tail = " | render 1.000 ms";
        assert_eq!(
            fit_status("file.gif | [Q] back", tail, 80),
            "file.gif | [Q] back | render 1.000 ms"
        );
        // Too long: the description gives way and the measurements stay whole.
        assert_eq!(
            fit_status("file.gif | [Q] back", tail, 26),
            "file.gif | render 1.000 ms"
        );
        // Shorter than the measurements alone: cut to what fits, never more.
        assert_eq!(fit_status("file.gif", tail, 5), " | re");
        assert_eq!(fit_status("file.gif", tail, 0), "");
        assert_eq!(fit_status("né.gif | hints", " | é", 9).chars().count(), 9);
    }

    #[test]
    fn quality_modes_map_to_expected_renderers() {
        assert_eq!(QualityMode::Fast.sampling(), SamplingFilter::Nearest);
        assert_eq!(QualityMode::Balanced.sampling(), SamplingFilter::Bilinear);
        assert_eq!(QualityMode::Quality.sampling(), SamplingFilter::Area);
        assert!(QualityMode::parse("reliable").is_none());
        assert!(QualityMode::parse("unique-expanded").is_none());
        assert!(QualityMode::parse("pruned").is_none());
    }

    #[test]
    fn comparison_cycle_visits_all_modes() {
        let mut mode = QualityMode::Fast;
        for expected in QualityMode::ALL.into_iter().skip(1) {
            mode = mode.next();
            assert_eq!(mode, expected);
        }
        assert_eq!(mode.next(), QualityMode::Fast);
    }

    #[test]
    fn unsupported_media_remains_unsupported() {
        let capabilities = VideoDecoder::capabilities();
        assert_eq!(
            supports_extension("webm"),
            cfg!(feature = "webm") && capabilities.supports_container(Container::WebM)
        );
        assert_eq!(
            supports_extension("mkv"),
            cfg!(feature = "mkv") && capabilities.supports_container(Container::Mkv)
        );
        assert_eq!(
            supports_extension("avi"),
            cfg!(feature = "avi") && capabilities.supports_container(Container::Avi)
        );
        assert_eq!(
            supports_extension("mov"),
            cfg!(feature = "mov") && capabilities.supports_container(Container::Mov)
        );
        assert_eq!(supports_extension("webp"), cfg!(feature = "webp"));
        for extension in ["png", "jpg", "jpeg", "gif"] {
            assert!(supports_extension(extension));
        }
        assert_eq!(
            supports_extension("mp4"),
            capabilities.supports_container(Container::Mp4)
        );
    }
}
