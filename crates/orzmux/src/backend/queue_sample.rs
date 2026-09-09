//! Sampled queue-depth instrumentation for the backend: per-pane chunk
//! peaks and the event / command channel peaks, handed out at most once
//! per interval so a flooding terminal logs one line a second.

use crate::protocol::PaneId;
use std::mem;
use std::time::{Duration, Instant};

/// How many reader chunks wait unparsed in one pane's chunk channel
/// (path A), in units of one `read(2)` result of up to 4 KiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) struct ChunkDepth(pub usize);

/// Tracks per-queue peaks between samples and hands them out at most
/// once per [`Self::SAMPLE_INTERVAL`].
#[derive(Debug)]
pub(crate) struct QueueSampler {
    /// When the last sample was handed out, or when the sampler was
    /// built.
    last_sample: Instant,
    /// The non-zero chunk peaks recorded since the last sample, one
    /// entry per pane in first-recorded order.
    chunks: Vec<(PaneId, ChunkDepth)>,
    /// The event channel (path B) peak since the last sample.
    events: usize,
    /// The command channel (path C) peak since the last sample.
    commands: usize,
}

/// The peaks one sample reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueSample {
    /// Every pane whose chunk peak was non-zero, in first-recorded
    /// order.
    pub chunks: Vec<(PaneId, ChunkDepth)>,
    /// The event channel (path B) peak.
    pub events: usize,
    /// The command channel (path C) peak.
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
            chunks: Vec::new(),
            events: 0,
            commands: 0,
        }
    }

    /// Records one pane's chunk depth, retaining the maximum since the
    /// last sample. A zero depth records nothing.
    pub fn record_pane_depth(&mut self, pane: PaneId, depth: ChunkDepth) {
        if depth == ChunkDepth(0) {
            return;
        }
        match self.chunks.iter_mut().find(|(id, _)| *id == pane) {
            Some((_, peak)) => *peak = (*peak).max(depth),
            None => self.chunks.push((pane, depth)),
        }
    }

    /// Records the event and command channel depths, retaining each
    /// maximum since the last sample.
    pub fn record_channel_depths(&mut self, events: usize, commands: usize) {
        self.events = self.events.max(events);
        self.commands = self.commands.max(commands);
    }

    /// Returns the peaks and resets them once [`Self::SAMPLE_INTERVAL`]
    /// has elapsed since the last sample and any peak is non-zero.
    pub fn sample(&mut self, now: Instant) -> Option<QueueSample> {
        if !self.has_peak() || now < self.last_sample + Self::SAMPLE_INTERVAL {
            return None;
        }
        self.last_sample = now;
        Some(QueueSample {
            chunks: mem::take(&mut self.chunks),
            events: mem::take(&mut self.events),
            commands: mem::take(&mut self.commands),
        })
    }

    /// When an unreported non-zero peak exists, the instant the next
    /// sample is due; `None` otherwise.
    pub fn report_deadline(&self) -> Option<Instant> {
        self.has_peak()
            .then(|| self.last_sample + Self::SAMPLE_INTERVAL)
    }

    fn has_peak(&self) -> bool {
        !self.chunks.is_empty() || self.events > 0 || self.commands > 0
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
        sampler.record_channel_depths(5, 1);
        assert_eq!(sampler.sample(start + Duration::from_millis(500)), None);
        let sample = sampler
            .sample(start + QueueSampler::SAMPLE_INTERVAL)
            .expect("a sample is due");
        assert_eq!(sample.chunks, vec![(PANE, ChunkDepth(2))]);
        assert_eq!((sample.events, sample.commands), (5, 1));
    }

    /// Asserts that the peaks start over after a sample.
    ///
    /// Case: a burst of output fills a queue, the sample reports it,
    /// and the next second is quiet apart from one small chunk.
    #[test]
    fn the_peaks_start_over_after_a_sample() {
        let (mut sampler, start) = sampler();
        sampler.record_pane_depth(PANE, ChunkDepth(40));
        sampler.record_channel_depths(9, 9);
        let first = start + QueueSampler::SAMPLE_INTERVAL;
        sampler.sample(first).expect("a sample is due");
        assert_eq!(sampler.sample(first + QueueSampler::SAMPLE_INTERVAL), None);
        sampler.record_pane_depth(PANE, ChunkDepth(1));
        let second = sampler
            .sample(first + QueueSampler::SAMPLE_INTERVAL)
            .expect("a sample is due");
        assert_eq!(second.chunks, vec![(PANE, ChunkDepth(1))]);
        assert_eq!((second.events, second.commands), (0, 0));
    }

    /// Asserts that a sample is `None` when every recorded peak is
    /// zero, however much time has passed.
    ///
    /// Case: an idle shell sits at its prompt for minutes while the
    /// backend keeps recording empty queues.
    #[test]
    fn a_sample_is_none_when_every_peak_is_zero() {
        let (mut sampler, start) = sampler();
        sampler.record_pane_depth(PANE, ChunkDepth(0));
        sampler.record_channel_depths(0, 0);
        assert_eq!(
            sampler.sample(start + 10 * QueueSampler::SAMPLE_INTERVAL),
            None
        );
    }

    /// Asserts that the report deadline exists only while an unreported
    /// non-zero peak does, and names the end of the current interval.
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
