use metrics::{Key, Label};
use tracing::Level;

fn labels_by_level(level: Level) -> &'static [Label] {
    const fn f(value: &'static str) -> Label {
        Label::from_static_parts("level", value)
    }

    const TRACE_LABELS: &[Label] = &[f("Trace")];
    const DEBUG_LABELS: &[Label] = &[f("Debug")];
    const INFO_LABELS: &[Label] = &[f("Info")];
    const WARN_LABELS: &[Label] = &[f("Warn")];
    const ERROR_LABELS: &[Label] = &[f("Error")];

    match level {
        Level::TRACE => TRACE_LABELS,
        Level::DEBUG => DEBUG_LABELS,
        Level::INFO => INFO_LABELS,
        Level::WARN => WARN_LABELS,
        Level::ERROR => ERROR_LABELS,
    }
}

pub(crate) fn emitted_events_total(level: Level) {
    let recorder = ward!(metrics::try_recorder());
    let labels = labels_by_level(level);
    let key = Key::from_static_parts("elfo_emitted_events_total", labels);
    recorder.increment_counter(&key, 1);
}

pub(crate) fn lost_events_total(level: Level) {
    let recorder = ward!(metrics::try_recorder());
    let labels = labels_by_level(level);
    let key = Key::from_static_parts("elfo_lost_events_total", labels);
    recorder.increment_counter(&key, 1);
}
