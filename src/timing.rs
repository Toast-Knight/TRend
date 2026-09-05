use core::fmt;
use core::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameDecision {
    Wait(Duration),
    Present,
    Drop,
}

impl FrameDecision {
    pub fn for_timestamp(
        media_time: Duration,
        frame_timestamp: Duration,
        late_threshold: Duration,
    ) -> Self {
        if frame_timestamp > media_time {
            Self::Wait(frame_timestamp - media_time)
        } else if media_time - frame_timestamp > late_threshold {
            Self::Drop
        } else {
            Self::Present
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaybackClock {
    wall_anchor: Duration,
    media_anchor: Duration,
    paused: bool,
}

impl PlaybackClock {
    pub const fn new(wall_now: Duration) -> Self {
        Self {
            wall_anchor: wall_now,
            media_anchor: Duration::ZERO,
            paused: false,
        }
    }

    pub fn media_time(&self, wall_now: Duration) -> Duration {
        if self.paused {
            self.media_anchor
        } else {
            self.media_anchor
                .saturating_add(wall_now.saturating_sub(self.wall_anchor))
        }
    }

    pub fn pause(&mut self, wall_now: Duration) {
        if !self.paused {
            self.media_anchor = self.media_time(wall_now);
            self.wall_anchor = wall_now;
            self.paused = true;
        }
    }

    pub fn play(&mut self, wall_now: Duration) {
        if self.paused {
            self.wall_anchor = wall_now;
            self.paused = false;
        }
    }

    pub fn seek(&mut self, media_time: Duration, wall_now: Duration) {
        self.media_anchor = media_time;
        self.wall_anchor = wall_now;
    }

    pub fn restart(&mut self, wall_now: Duration) {
        self.seek(Duration::ZERO, wall_now);
    }

    pub const fn is_paused(&self) -> bool {
        self.paused
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimelineError {
    NoFrames,
    ZeroDuration,
    DurationOverflow,
    FrameOutOfRange,
}

impl fmt::Display for TimelineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NoFrames => "animation must contain at least one frame",
            Self::ZeroDuration => "animation frame duration must be non-zero",
            Self::DurationOverflow => "animation duration overflowed",
            Self::FrameOutOfRange => "animation frame index is out of range",
        })
    }
}

impl std::error::Error for TimelineError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationPosition {
    pub frame_index: usize,
    pub loop_index: u64,
    pub ended: bool,
}

#[derive(Clone, Debug)]
pub struct AnimationTimeline {
    clock: PlaybackClock,
    frame_ends: Vec<Duration>,
    duration: Duration,
    total_plays: Option<u64>,
}

impl AnimationTimeline {
    pub fn new<I>(
        wall_now: Duration,
        frame_durations: I,
        total_plays: Option<u64>,
    ) -> Result<Self, TimelineError>
    where
        I: IntoIterator<Item = Duration>,
    {
        if total_plays == Some(0) {
            return Err(TimelineError::NoFrames);
        }
        let mut frame_ends = Vec::new();
        let mut duration = Duration::ZERO;
        for frame_duration in frame_durations {
            if frame_duration.is_zero() {
                return Err(TimelineError::ZeroDuration);
            }
            duration = duration
                .checked_add(frame_duration)
                .ok_or(TimelineError::DurationOverflow)?;
            frame_ends.push(duration);
        }
        if frame_ends.is_empty() {
            return Err(TimelineError::NoFrames);
        }

        Ok(Self {
            clock: PlaybackClock::new(wall_now),
            frame_ends,
            duration,
            total_plays,
        })
    }

    pub fn position(&self, wall_now: Duration) -> AnimationPosition {
        let media_time = self.clock.media_time(wall_now);
        let duration_nanos = self.duration.as_nanos();
        let loop_index_u128 = media_time.as_nanos() / duration_nanos;
        let loop_index = u64::try_from(loop_index_u128).unwrap_or(u64::MAX);
        let ended = self.total_plays.is_some_and(|plays| loop_index >= plays);
        if ended {
            return AnimationPosition {
                frame_index: self.frame_ends.len() - 1,
                loop_index: self.total_plays.unwrap_or(1).saturating_sub(1),
                ended: true,
            };
        }

        let in_loop_nanos = media_time.as_nanos() % duration_nanos;
        let frame_index = self
            .frame_ends
            .partition_point(|end| end.as_nanos() <= in_loop_nanos);
        AnimationPosition {
            frame_index,
            loop_index,
            ended: false,
        }
    }

    pub fn pause(&mut self, wall_now: Duration) {
        self.clock.pause(wall_now);
    }

    pub fn play(&mut self, wall_now: Duration) {
        self.clock.play(wall_now);
    }

    pub fn restart(&mut self, wall_now: Duration) {
        self.clock.seek(Duration::ZERO, wall_now);
    }

    pub fn seek_time(&mut self, media_time: Duration, wall_now: Duration) {
        self.clock.seek(media_time, wall_now);
    }

    pub fn seek_frame(
        &mut self,
        frame_index: usize,
        wall_now: Duration,
    ) -> Result<(), TimelineError> {
        if frame_index >= self.frame_ends.len() {
            return Err(TimelineError::FrameOutOfRange);
        }
        let frame_start = frame_index
            .checked_sub(1)
            .map_or(Duration::ZERO, |previous| self.frame_ends[previous]);
        self.clock.seek(frame_start, wall_now);
        Ok(())
    }

    pub const fn frame_count(&self) -> usize {
        self.frame_ends.len()
    }

    pub const fn duration(&self) -> Duration {
        self.duration
    }

    pub const fn total_plays(&self) -> Option<u64> {
        self.total_plays
    }

    pub const fn is_paused(&self) -> bool {
        self.clock.is_paused()
    }
}

#[cfg(test)]
mod tests {
    use core::iter;

    use super::*;

    #[test]
    fn late_frames_drop_instead_of_accumulating_latency() {
        assert_eq!(
            FrameDecision::for_timestamp(
                Duration::from_millis(500),
                Duration::from_millis(300),
                Duration::from_millis(100)
            ),
            FrameDecision::Drop
        );
        assert_eq!(
            FrameDecision::for_timestamp(
                Duration::from_millis(500),
                Duration::from_millis(450),
                Duration::from_millis(100)
            ),
            FrameDecision::Present
        );
    }

    #[test]
    fn pause_resume_and_seek_rebase_without_losing_position() {
        let mut clock = PlaybackClock::new(Duration::from_secs(10));
        clock.pause(Duration::from_secs(12));
        assert_eq!(
            clock.media_time(Duration::from_secs(20)),
            Duration::from_secs(2)
        );

        clock.play(Duration::from_secs(20));
        assert_eq!(
            clock.media_time(Duration::from_secs(21)),
            Duration::from_secs(3)
        );

        clock.seek(Duration::from_secs(30), Duration::from_secs(21));
        assert_eq!(
            clock.media_time(Duration::from_secs(22)),
            Duration::from_secs(31)
        );

        clock.restart(Duration::from_secs(23));
        assert_eq!(
            clock.media_time(Duration::from_secs(24)),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn animation_timeline_selects_variable_duration_frames() {
        let timeline = AnimationTimeline::new(
            Duration::ZERO,
            [
                Duration::from_millis(10),
                Duration::from_millis(100),
                Duration::from_millis(50),
            ],
            None,
        )
        .unwrap();

        assert_eq!(timeline.position(Duration::from_millis(9)).frame_index, 0);
        assert_eq!(timeline.position(Duration::from_millis(10)).frame_index, 1);
        assert_eq!(timeline.position(Duration::from_millis(109)).frame_index, 1);
        assert_eq!(timeline.position(Duration::from_millis(110)).frame_index, 2);
        assert_eq!(timeline.position(Duration::from_millis(160)).frame_index, 0);
        assert_eq!(timeline.position(Duration::from_millis(160)).loop_index, 1);
    }

    #[test]
    fn finite_animation_stops_on_last_frame() {
        let timeline = AnimationTimeline::new(
            Duration::ZERO,
            iter::repeat_n(Duration::from_millis(20), 2),
            Some(2),
        )
        .unwrap();

        assert_eq!(
            timeline.position(Duration::from_millis(80)),
            AnimationPosition {
                frame_index: 1,
                loop_index: 1,
                ended: true,
            }
        );
    }

    #[test]
    fn animation_pause_seek_resume_and_restart_are_host_driven() {
        let mut timeline = AnimationTimeline::new(
            Duration::from_secs(10),
            [Duration::from_millis(50), Duration::from_millis(50)],
            None,
        )
        .unwrap();
        timeline.pause(Duration::from_millis(10_075));
        assert_eq!(timeline.position(Duration::from_secs(20)).frame_index, 1);

        timeline.seek_frame(0, Duration::from_secs(20)).unwrap();
        assert_eq!(timeline.position(Duration::from_secs(30)).frame_index, 0);
        timeline.play(Duration::from_secs(30));
        assert_eq!(
            timeline.position(Duration::from_millis(30_050)).frame_index,
            1
        );

        timeline.restart(Duration::from_secs(31));
        assert_eq!(timeline.position(Duration::from_secs(31)).frame_index, 0);
    }
}
