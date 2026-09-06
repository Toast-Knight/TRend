use std::error::Error;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use trend::{
    AnsiPresenter, CellFrame, FitMode, PixelFrameView, PlaybackClock, PositionedRenderRequest,
    RenderOptions, RenderRequest, Renderer, SamplingFilter, TerminalCapabilities,
    TerminalColorMode, Viewport,
};
use trend_media::{
    MediaKind, MediaLimits, MediaSource, PreviewRange, PreviewSession, SeekAccuracy,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutputCapabilities {
    color: TerminalColorMode,
    unicode: bool,
}

struct FrameCommand {
    viewport: Viewport,
    row: u16,
    column: u16,
    sampling: SamplingFilter,
    capabilities: OutputCapabilities,
    elapsed: Duration,
    presentation: Presentation,
}

struct FrameAtCommand {
    viewport: Viewport,
    row: u16,
    column: u16,
    sampling: SamplingFilter,
    capabilities: OutputCapabilities,
    timestamp: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Presentation {
    Representative,
    Clip(PreviewRange),
}

struct FrameMetrics {
    decode_us: u128,
    render_us: u128,
    retained_bytes: usize,
    decoded_frames: u64,
    dropped_frames: u64,
    buffered_frames: usize,
}

const HELP: &str = "TRend Python TUI Bridge

Decode media and return positioned ANSI frames over a line-oriented subprocess protocol.

Usage:
    trend-python-tui-bridge <MEDIA>

Options:
    -h, --help       Show this help
    -V, --version    Show the package version

Protocol commands:
    META             Read media metadata
    FRAME ...        Render a representative or timed preview frame
    FRAME_AT ...     Render at an exact requested media timestamp
    SEEK ...         Seek the media decoder
    CLOCK ...        Update the host-driven playback clock
    RESET            Reset incremental ANSI presenter state
    QUIT             Exit cleanly

See docs/bridge-protocol.md for field definitions and response framing.";

fn main() {
    if let Err(error) = run() {
        eprintln!("trend Python TUI bridge: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let argument = std::env::args_os()
        .nth(1)
        .ok_or("missing media path; run trend-python-tui-bridge --help")?;
    if argument == "-h" || argument == "--help" {
        println!("{HELP}");
        return Ok(());
    }
    if argument == "-V" || argument == "--version" {
        println!("trend-python-tui-bridge {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let path = argument;
    let source = MediaSource::open(Path::new(&path), MediaLimits::default())?;
    let mut session = PreviewSession::new(source, Duration::ZERO);
    let mut active_range = None;
    let renderer = Renderer::new();
    let mut frame = CellFrame::default();
    let mut generation = 0_u64;
    let mut active_capabilities = None;
    let mut presenter = AnsiPresenter::with_capabilities(TerminalCapabilities::CONSERVATIVE);
    let mut playback_clock = PlaybackClock::new(Duration::ZERO);
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line == "QUIT" {
            break;
        }
        if line == "META" {
            write_metadata(&mut stdout, &session)?;
            continue;
        }
        if line == "RESET" {
            presenter = AnsiPresenter::with_capabilities(TerminalCapabilities::CONSERVATIVE);
            active_capabilities = None;
            writeln!(stdout, "RESET_OK")?;
            stdout.flush()?;
            continue;
        }
        if line.starts_with("CLOCK ") {
            match apply_clock_command(&line, &mut playback_clock) {
                Ok(()) => {
                    let fields: Vec<_> = line.split_ascii_whitespace().collect();
                    let wall_now = parse_duration_ns(fields.last().unwrap_or(&""), "wall_ns")?;
                    writeln!(
                        stdout,
                        "CLOCK_OK {} {}",
                        playback_clock.media_time(wall_now).as_nanos(),
                        u8::from(playback_clock.is_paused()),
                    )?;
                    stdout.flush()?;
                }
                Err(error) => {
                    writeln!(stdout, "ERR {error}")?;
                    stdout.flush()?;
                }
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("SEEK ") {
            match parse_duration_ns(value, "timestamp_ns") {
                Ok(timestamp) => {
                    let started = Instant::now();
                    let result = session.seek_media_time(timestamp)?;
                    let seek_us = started.elapsed().as_micros();
                    writeln!(
                        stdout,
                        "SEEK_OK {} {} {} {} {}",
                        result.requested_timestamp.as_nanos(),
                        optional_duration_ns(result.first_frame_timestamp),
                        seek_accuracy_name(result.accuracy),
                        seek_us,
                        session.source().seek_discarded_frames(),
                    )?;
                    stdout.flush()?;
                }
                Err(error) => {
                    writeln!(stdout, "ERR {error}")?;
                    stdout.flush()?;
                }
            }
            continue;
        }
        if line.starts_with("FRAME_AT ") {
            match parse_frame_at_command(&line) {
                Ok(command) => {
                    if active_capabilities != Some(command.capabilities) {
                        presenter = presenter_for(command.capabilities);
                        active_capabilities = Some(command.capabilities);
                    }
                    generation = generation.wrapping_add(1);
                    write_frame_at(
                        &mut stdout,
                        &mut session,
                        &renderer,
                        &mut frame,
                        &mut presenter,
                        &command,
                        generation,
                    )?;
                }
                Err(error) => {
                    writeln!(stdout, "ERR {error}")?;
                    stdout.flush()?;
                }
            }
            continue;
        }
        let command = match parse_frame_command(&line) {
            Ok(command) => command,
            Err(error) => {
                writeln!(stdout, "ERR {error}")?;
                stdout.flush()?;
                continue;
            }
        };
        if active_capabilities != Some(command.capabilities) {
            presenter = presenter_for(command.capabilities);
            active_capabilities = Some(command.capabilities);
        }
        generation = generation.wrapping_add(1);
        let result = render_media(
            &mut session,
            &mut active_range,
            &renderer,
            &mut frame,
            &command,
            generation,
        );
        match result {
            Ok(Some(metrics)) => {
                let ansi_started = Instant::now();
                let bytes = presenter.present_at(&frame, command.row, command.column);
                let ansi_us = ansi_started.elapsed().as_micros();
                writeln!(
                    stdout,
                    "OK {} {} {} {} {} {} {} {}",
                    bytes.len(),
                    metrics.decode_us,
                    metrics.render_us,
                    ansi_us,
                    metrics.retained_bytes,
                    metrics.decoded_frames,
                    metrics.dropped_frames,
                    metrics.buffered_frames
                )?;
                stdout.write_all(bytes)?;
                stdout.flush()?;
            }
            Ok(None) => {
                writeln!(stdout, "WAIT")?;
                stdout.flush()?;
            }
            Err(error) => {
                writeln!(stdout, "ERR {}", single_line(&error.to_string()))?;
                stdout.flush()?;
            }
        }
    }
    Ok(())
}

fn presenter_for(capabilities: OutputCapabilities) -> AnsiPresenter {
    AnsiPresenter::with_capabilities(TerminalCapabilities {
        color: capabilities.color,
        unicode: capabilities.unicode,
    })
}

fn write_metadata(output: &mut impl Write, session: &PreviewSession) -> io::Result<()> {
    let info = session.info();
    let (fps_numerator, fps_denominator) = info.nominal_frame_rate.unwrap_or((0, 0));
    writeln!(
        output,
        "META_OK {} {} {} {} {} {} {} {}",
        match info.kind {
            MediaKind::Image => "image",
            MediaKind::Animation => "animation",
            MediaKind::Video => "video",
        },
        info.width,
        info.height,
        optional_duration_ns(info.duration),
        fps_numerator,
        fps_denominator,
        u8::from(info.audio_present),
        seek_accuracy_name(info.seek_accuracy),
    )?;
    output.flush()
}

fn write_frame_at(
    output: &mut impl Write,
    session: &mut PreviewSession,
    renderer: &Renderer,
    cells: &mut CellFrame,
    presenter: &mut AnsiPresenter,
    command: &FrameAtCommand,
    generation: u64,
) -> Result<(), Box<dyn Error>> {
    let acquisition_started = Instant::now();
    let Some(media_frame) = session.frame_at_media_time(command.timestamp)? else {
        writeln!(output, "WAIT")?;
        output.flush()?;
        return Ok(());
    };
    let acquisition_us = acquisition_started.elapsed().as_micros();
    let requested_ns = media_frame.requested_timestamp.as_nanos();
    let presentation_ns = media_frame.presentation_timestamp.as_nanos();
    let duration_ns = optional_duration_ns(media_frame.duration);
    let positioned = presenter.render_at(
        renderer,
        media_frame.view,
        PositionedRenderRequest {
            render: RenderRequest {
                viewport: command.viewport,
                fit: FitMode::Contain,
                generation,
            },
            origin_row: command.row,
            origin_column: command.column,
        },
        RenderOptions {
            sampling: command.sampling,
            ..RenderOptions::default()
        },
        cells,
    )?;
    let render_us = positioned.render_time.as_micros();
    let ansi_us = positioned.ansi_time.as_micros();
    let bytes = positioned.bytes;
    writeln!(
        output,
        "FRAME_OK {} {} {} {} {} {} {} {} {} {} {} {}",
        bytes.len(),
        requested_ns,
        presentation_ns,
        duration_ns,
        acquisition_us,
        render_us,
        ansi_us,
        session.source().retained_bytes(),
        session.source().decoded_frames(),
        session.source().dropped_frames(),
        session.source().buffered_frames(),
        session.source().seek_discarded_frames(),
    )?;
    output.write_all(bytes)?;
    output.flush()?;
    Ok(())
}

fn optional_duration_ns(value: Option<Duration>) -> i128 {
    value
        .and_then(|duration| i128::try_from(duration.as_nanos()).ok())
        .unwrap_or(-1)
}

fn seek_accuracy_name(value: Option<SeekAccuracy>) -> &'static str {
    match value {
        Some(SeekAccuracy::KeyframeThenForward) => "keyframe_then_forward",
        None => "not_applicable",
    }
}

fn apply_clock_command(line: &str, clock: &mut PlaybackClock) -> Result<(), String> {
    let fields: Vec<_> = line.split_ascii_whitespace().collect();
    match fields.as_slice() {
        ["CLOCK", "POSITION", wall] => {
            parse_duration_ns(wall, "wall_ns")?;
        }
        ["CLOCK", "PAUSE", wall] => clock.pause(parse_duration_ns(wall, "wall_ns")?),
        ["CLOCK", "PLAY", wall] => clock.play(parse_duration_ns(wall, "wall_ns")?),
        ["CLOCK", "SEEK", media, wall] => clock.seek(
            parse_duration_ns(media, "media_ns")?,
            parse_duration_ns(wall, "wall_ns")?,
        ),
        ["CLOCK", "RESTART", wall] => clock.restart(parse_duration_ns(wall, "wall_ns")?),
        _ => {
            return Err(
                "expected CLOCK POSITION|PAUSE|PLAY wall_ns, CLOCK SEEK media_ns wall_ns, or CLOCK RESTART wall_ns".into(),
            );
        }
    }
    Ok(())
}

fn render_media(
    session: &mut PreviewSession,
    active_range: &mut Option<PreviewRange>,
    renderer: &Renderer,
    output: &mut CellFrame,
    command: &FrameCommand,
    generation: u64,
) -> Result<Option<FrameMetrics>, Box<dyn Error>> {
    let decode_started = Instant::now();
    let frame = match command.presentation {
        Presentation::Representative => session.representative_frame()?,
        Presentation::Clip(range) => {
            if *active_range != Some(range) {
                session.start(range, command.elapsed)?;
                *active_range = Some(range);
            }
            session.frame_for_time(command.elapsed)?
        }
    };
    let Some(frame) = frame else {
        return Ok(None);
    };
    let decode_us = decode_started.elapsed().as_micros();
    let render_started = Instant::now();
    render_view(
        renderer,
        output,
        frame,
        command.viewport,
        command.sampling,
        generation,
    )?;
    let render_us = render_started.elapsed().as_micros();
    Ok(Some(FrameMetrics {
        decode_us,
        render_us,
        retained_bytes: session.source().retained_bytes(),
        decoded_frames: session.source().decoded_frames(),
        dropped_frames: session.source().dropped_frames(),
        buffered_frames: session.source().buffered_frames(),
    }))
}

fn render_view(
    renderer: &Renderer,
    output: &mut CellFrame,
    source: PixelFrameView<'_>,
    viewport: Viewport,
    sampling: SamplingFilter,
    generation: u64,
) -> Result<bool, Box<dyn Error>> {
    renderer.render_with_options_into(
        source,
        RenderRequest {
            viewport,
            fit: FitMode::Contain,
            generation,
        },
        RenderOptions {
            sampling,
            ..RenderOptions::default()
        },
        output,
    )?;
    Ok(true)
}

fn parse_frame_command(line: &str) -> Result<FrameCommand, String> {
    let fields: Vec<_> = line.split_ascii_whitespace().collect();
    if fields.len() != 13 || fields[0] != "FRAME" {
        return Err(
            "expected FRAME width height row column quality color unicode elapsed_ms presentation start_ms duration_ms loop".into(),
        );
    }
    let parse_u16 = |index: usize, name: &str| {
        fields[index]
            .parse::<u16>()
            .map_err(|_| format!("invalid {name}"))
    };
    let width = parse_u16(1, "width")?;
    let height = parse_u16(2, "height")?;
    if width == 0 || height == 0 {
        return Err("viewport must be nonzero".into());
    }
    let sampling = match fields[5] {
        "fast" => SamplingFilter::Nearest,
        "balanced" => SamplingFilter::Bilinear,
        "quality" => SamplingFilter::Area,
        _ => return Err("quality must be fast, balanced, or quality".into()),
    };
    let color = match fields[6] {
        "truecolor" => TerminalColorMode::TrueColor,
        "256" => TerminalColorMode::Ansi256,
        "16" => TerminalColorMode::Ansi16,
        "mono" => TerminalColorMode::Monochrome,
        _ => return Err("color must be truecolor, 256, 16, or mono".into()),
    };
    let unicode = match fields[7] {
        "unicode" => true,
        "ascii" => false,
        _ => return Err("glyph mode must be unicode or ascii".into()),
    };
    let elapsed_ms = fields[8]
        .parse::<u64>()
        .map_err(|_| "invalid elapsed_ms".to_string())?;
    let presentation = match fields[9] {
        "representative" => Presentation::Representative,
        "clip" => Presentation::Clip(PreviewRange {
            start: Duration::from_millis(
                fields[10]
                    .parse::<u64>()
                    .map_err(|_| "invalid start_ms".to_string())?,
            ),
            duration: Duration::from_millis(
                fields[11]
                    .parse::<u64>()
                    .map_err(|_| "invalid duration_ms".to_string())?,
            ),
            looping: match fields[12] {
                "loop" => true,
                "once" => false,
                _ => return Err("loop mode must be loop or once".into()),
            },
        }),
        _ => return Err("presentation must be representative or clip".into()),
    };
    Ok(FrameCommand {
        viewport: Viewport { width, height },
        row: parse_u16(3, "row")?.max(1),
        column: parse_u16(4, "column")?.max(1),
        sampling,
        capabilities: OutputCapabilities { color, unicode },
        elapsed: Duration::from_millis(elapsed_ms),
        presentation,
    })
}

fn parse_frame_at_command(line: &str) -> Result<FrameAtCommand, String> {
    let fields: Vec<_> = line.split_ascii_whitespace().collect();
    if fields.len() != 9 || fields[0] != "FRAME_AT" {
        return Err(
            "expected FRAME_AT width height row column quality color unicode timestamp_ns".into(),
        );
    }
    let frame = parse_frame_command(&format!(
        "FRAME {} {} {} {} {} {} {} 0 representative 0 0 once",
        fields[1], fields[2], fields[3], fields[4], fields[5], fields[6], fields[7]
    ))?;
    Ok(FrameAtCommand {
        viewport: frame.viewport,
        row: frame.row,
        column: frame.column,
        sampling: frame.sampling,
        capabilities: frame.capabilities,
        timestamp: parse_duration_ns(fields[8], "timestamp_ns")?,
    })
}

fn parse_duration_ns(value: &str, name: &str) -> Result<Duration, String> {
    let nanoseconds = value
        .parse::<u64>()
        .map_err(|_| format!("invalid {name}"))?;
    Ok(Duration::from_nanos(nanoseconds))
}

fn single_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_names_the_protocol_and_discovery_options() {
        for command in [
            "META", "FRAME", "FRAME_AT", "SEEK", "CLOCK", "RESET", "QUIT",
        ] {
            assert!(HELP.contains(command));
        }
        assert!(HELP.contains("--help"));
        assert!(HELP.contains("--version"));
    }

    #[test]
    fn parses_explicit_application_configuration() {
        let command =
            parse_frame_command("FRAME 40 12 6 24 quality 256 ascii 1500 clip 250 3000 loop")
                .unwrap();
        assert_eq!(
            command.viewport,
            Viewport {
                width: 40,
                height: 12
            }
        );
        assert_eq!(command.row, 6);
        assert_eq!(command.column, 24);
        assert_eq!(command.sampling, SamplingFilter::Area);
        assert_eq!(command.capabilities.color, TerminalColorMode::Ansi256);
        assert!(!command.capabilities.unicode);
        assert_eq!(command.elapsed, Duration::from_millis(1_500));
        assert_eq!(
            command.presentation,
            Presentation::Clip(PreviewRange {
                start: Duration::from_millis(250),
                duration: Duration::from_secs(3),
                looping: true,
            })
        );
    }

    #[test]
    fn rejects_zero_sized_viewport() {
        assert!(
            parse_frame_command("FRAME 0 12 1 1 fast mono ascii 0 representative 0 0 once")
                .is_err()
        );
    }

    #[test]
    fn parses_exact_timestamp_frame_request() {
        let command =
            parse_frame_at_command("FRAME_AT 60 20 5 20 balanced truecolor unicode 83250000000")
                .unwrap();
        assert_eq!(command.timestamp, Duration::from_millis(83_250));
        assert_eq!(
            command.viewport,
            Viewport {
                width: 60,
                height: 20
            }
        );
        assert_eq!((command.row, command.column), (5, 20));
    }

    #[test]
    fn bridge_clock_is_host_driven() {
        let mut clock = PlaybackClock::new(Duration::ZERO);
        apply_clock_command("CLOCK SEEK 5000000000 10000000000", &mut clock).unwrap();
        apply_clock_command("CLOCK PAUSE 12000000000", &mut clock).unwrap();
        assert_eq!(
            clock.media_time(Duration::from_secs(100)),
            Duration::from_secs(7)
        );
        apply_clock_command("CLOCK PLAY 20000000000", &mut clock).unwrap();
        assert_eq!(
            clock.media_time(Duration::from_secs(21)),
            Duration::from_secs(8)
        );
    }
}
