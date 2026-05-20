//! PR-E2a: validates that the new observability instrumentation fires
//! under realistic bridge scenarios. Uses metrics-util DebuggingRecorder
//! installed per-test via metrics::set_default_local_recorder, then
//! snapshots Counter and Histogram values by name + labels.

use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use std::collections::HashMap;

/// Creates a fresh `DebuggingRecorder` + `Snapshotter` pair. The caller
/// keeps the recorder alive on its own stack and installs it via
/// `metrics::set_default_local_recorder(&recorder)`. The recorder must
/// outlive the `_guard` returned by `set_default_local_recorder`, which
/// is why the install is done inline by the test rather than abstracted
/// into a helper that returns the guard by value.
fn new_recorder() -> (DebuggingRecorder, Snapshotter) {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    (recorder, snapshotter)
}

/// Returns the counter value for `name` + `labels` (subset match), or None.
fn counter_value(
    snapshotter: &Snapshotter,
    name: &str,
    labels: &[(&str, &str)],
) -> Option<u64> {
    snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .find_map(|(key, _unit, _desc, value)| {
            if key.key().name() != name {
                return None;
            }
            let key_labels: HashMap<&str, &str> =
                key.key().labels().map(|l| (l.key(), l.value())).collect();
            for (k, v) in labels {
                if key_labels.get(k) != Some(v) {
                    return None;
                }
            }
            match value {
                DebugValue::Counter(c) => Some(c),
                _ => None,
            }
        })
}

/// Returns the histogram sample count for `name` + `labels`, or None.
#[expect(dead_code, reason = "consumed by tests added in later commits in this PR")]
fn histogram_count(
    snapshotter: &Snapshotter,
    name: &str,
    labels: &[(&str, &str)],
) -> Option<usize> {
    snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .find_map(|(key, _unit, _desc, value)| {
            if key.key().name() != name {
                return None;
            }
            let key_labels: HashMap<&str, &str> =
                key.key().labels().map(|l| (l.key(), l.value())).collect();
            for (k, v) in labels {
                if key_labels.get(k) != Some(v) {
                    return None;
                }
            }
            match value {
                DebugValue::Histogram(samples) => Some(samples.len()),
                _ => None,
            }
        })
}

#[tokio::test]
async fn install_recorder_helper_works() {
    let (recorder, snapshotter) = new_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);
    metrics::counter!("__test_smoke").increment(1);
    assert_eq!(counter_value(&snapshotter, "__test_smoke", &[]), Some(1));
}
