//! Sampled queue-depth instrumentation for the backend: per-pane chunk
//! peaks and the event / command channel peaks, handed out at most once
//! per interval so a flooding terminal logs one line a second.

use crate::protocol::PaneId;
use std::mem;
use std::time::{Duration, Instant};

/// How many reader chunks wait unparsed in one pane's chunk channel
/// (path A), in units of one `read(2)` result of up to 4 KiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ChunkDepth(pub usize);

/// Tracks per-queue peaks between samples and hands them out at most
/// once per [`Self::SAMPLE_INTERVAL`].
#[derive(Debug)]
pub(crate) struct QueueSampler {
    /// When the last sample was handed out, or when the sampler was
    /// built.
    last_sample: Instant,
    /// The peaks recorded since the last sample.
    peaks: QueueSample,
}

/// The peaks one sample reports: every depth above
/// [`QueueSampler::BACKLOG_FLOOR`] recorded since the previous sample.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct QueueSample {
    /// Every pane whose chunk peak was recorded, in first-recorded
    /// order.
    pub chunks: Vec<(PaneId, ChunkDepth)>,
    /// The event channel (path B) peak, or zero when none was recorded.
    pub events: usize,
    /// The command channel (path C) peak, or zero when none was
    /// recorded.
    pub commands: usize,
}

impl QueueSampler {
    /// The shortest time between two handed-out samples.
    pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

    /// A sampler with no peaks whose first sample is due one interval
    /// after `now`.
    pub fn new(now: Instant) -> Self {
        Self {
            last_sample: now,
            peaks: QueueSample::default(),
        }
    }

    /// Records one pane's chunk depth, retaining the maximum since the
    /// last sample. A depth at or below [`Self::BACKLOG_FLOOR`] records
    /// nothing.
    pub fn record_pane_depth(&mut self, pane: PaneId, depth: ChunkDepth) {
        if depth.0 <= Self::BACKLOG_FLOOR {
            return;
        }
        match self.peaks.chunks.iter_mut().find(|(id, _)| *id == pane) {
            Some((_, peak)) => *peak = (*peak).max(depth),
            None => self.peaks.chunks.push((pane, depth)),
        }
    }

    /// Records the event and command channel depths, retaining each
    /// maximum since the last sample. A depth at or below
    /// [`Self::BACKLOG_FLOOR`] records nothing.
    pub fn record_channel_depths(&mut self, events: usize, commands: usize) {
        self.peaks.events = self.peaks.events.max(Self::above_floor(events));
        self.peaks.commands = self.peaks.commands.max(Self::above_floor(commands));
    }

    /// Returns the peaks and resets them once the
    /// [report deadline](Self::report_deadline) has passed.
    ///
    /// The interval is anchored on the previous hand-out, so the first
    /// peak after more than an interval of quiet is handed out on the
    /// wake that recorded it; each later sample of a burst covers one
    /// full interval.
    pub fn sample(&mut self, now: Instant) -> Option<QueueSample> {
        let due = self.report_deadline()?;
        if now < due {
            return None;
        }
        self.last_sample = now;
        Some(mem::take(&mut self.peaks))
    }

    /// When an unreported peak exists, the instant the next sample is
    /// due; `None` otherwise.
    pub fn report_deadline(&self) -> Option<Instant> {
        (!self.peaks.is_empty()).then(|| self.last_sample + Self::SAMPLE_INTERVAL)
    }

    /// The depth a wake implies on its own: the `Select` returns only
    /// once the woken queue holds one item, and one emitted frame waits
    /// in the event channel until the GUI's next update. A depth at or
    /// below the floor is not a peak and records nothing, so an
    /// interactive terminal neither logs nor adds a wake.
    const BACKLOG_FLOOR: usize = 1;

    fn above_floor(depth: usize) -> usize {
        if depth > Self::BACKLOG_FLOOR {
            depth
        } else {
            0
        }
    }
}

impl QueueSample {
    /// Whether no peak was recorded.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty() && self.events == 0 && self.commands == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANE: PaneId = PaneId(1);

    /// A sampler built at `start`, plus `start` itself so tests advance
    /// time arithmetically instead of sleeping.
    fn sampler() -> (QueueSampler, Instant) {
        let start = Instant::now();
        (QueueSampler::new(start), start)
    }

    /// Asserts that two depth recordings for one pane within an interval
    /// keep the higher one.
    ///
    /// Case: a `cat` of a large file queues 40 chunks on one wake and 3
    /// on the next, both before the sample comes due.
    #[test]
    fn two_recordings_within_one_interval_keep_the_higher_depth() {
        let (mut sampler, start) = sampler();
        sampler.record_pane_depth(PANE, ChunkDepth(40));
        sampler.record_pane_depth(PANE, ChunkDepth(3));
        let sample = sampler
            .sample(start + QueueSampler::SAMPLE_INTERVAL)
            .expect("a sample is due");
        assert_eq!(sample.chunks, vec![(PANE, ChunkDepth(40))]);
    }

    /// Asserts that a sample is `None` before the interval elapses and
    /// carries the peaks once it has.
    ///
    /// Case: the backend asks for a sample on every wake while output
    /// streams, and must log only once a second.
    #[test]
    fn a_sample_is_none_before_the_interval_and_some_after_it() {
        let (mut sampler, start) = sampler();
        sampler.record_pane_depth(PANE, ChunkDepth(2));
        sampler.record_channel_depths(5, 2);
        assert_eq!(sampler.sample(start + Duration::from_millis(500)), None);
        let sample = sampler
            .sample(start + QueueSampler::SAMPLE_INTERVAL)
            .expect("a sample is due");
        assert_eq!(sample.chunks, vec![(PANE, ChunkDepth(2))]);
        assert_eq!((sample.events, sample.commands), (5, 2));
    }

    /// Asserts that the peaks start over after a sample.
    ///
    /// Case: a burst of output fills a queue, the sample reports it,
    /// and the next second is quiet apart from two chunks on one wake.
    #[test]
    fn the_peaks_start_over_after_a_sample() {
        let (mut sampler, start) = sampler();
        sampler.record_pane_depth(PANE, ChunkDepth(40));
        sampler.record_channel_depths(9, 9);
        let first = start + QueueSampler::SAMPLE_INTERVAL;
        sampler.sample(first).expect("a sample is due");
        assert_eq!(sampler.sample(first + QueueSampler::SAMPLE_INTERVAL), None);
        sampler.record_pane_depth(PANE, ChunkDepth(2));
        let second = sampler
            .sample(first + QueueSampler::SAMPLE_INTERVAL)
            .expect("a sample is due");
        assert_eq!(second.chunks, vec![(PANE, ChunkDepth(2))]);
        assert_eq!((second.events, second.commands), (0, 0));
    }

    /// Asserts that a sample is `None` when every recorded depth is at
    /// or below the backlog floor, however much time has passed.
    ///
    /// Case: a user types at a shell prompt for minutes, and each
    /// keystroke leaves the backend one chunk to wake for and one echo
    /// frame the GUI has yet to drain.
    #[test]
    fn a_sample_is_none_when_every_depth_is_at_or_below_the_floor() {
        let (mut sampler, start) = sampler();
        sampler.record_pane_depth(PANE, ChunkDepth(0));
        sampler.record_pane_depth(PANE, ChunkDepth(1));
        sampler.record_channel_depths(0, 0);
        sampler.record_channel_depths(1, 1);
        assert_eq!(sampler.report_deadline(), None);
        assert_eq!(
            sampler.sample(start + 10 * QueueSampler::SAMPLE_INTERVAL),
            None
        );
    }

    /// Asserts that the report deadline exists only while an unreported
    /// peak does, and names the end of the current interval.
    ///
    /// Case: the backend goes idle right after one wake recorded a
    /// depth, and must wake once more to log it.
    #[test]
    fn the_report_deadline_exists_only_while_a_peak_is_unreported() {
        let (mut sampler, start) = sampler();
        assert_eq!(sampler.report_deadline(), None);
        sampler.record_channel_depths(0, 3);
        assert_eq!(
            sampler.report_deadline(),
            Some(start + QueueSampler::SAMPLE_INTERVAL)
        );
        sampler
            .sample(start + QueueSampler::SAMPLE_INTERVAL)
            .expect("a sample is due");
        assert_eq!(sampler.report_deadline(), None);
    }

    /// Asserts that the event and command peaks are tracked
    /// independently of each other.
    ///
    /// Case: a layout burst fills the event channel on one wake while a
    /// paste fills the command channel on another.
    #[test]
    fn channel_peaks_are_tracked_independently() {
        let (mut sampler, start) = sampler();
        sampler.record_channel_depths(5, 2);
        sampler.record_channel_depths(1, 7);
        let sample = sampler
            .sample(start + QueueSampler::SAMPLE_INTERVAL)
            .expect("a sample is due");
        assert_eq!((sample.events, sample.commands), (5, 7));
        assert!(sample.chunks.is_empty());
    }
}
