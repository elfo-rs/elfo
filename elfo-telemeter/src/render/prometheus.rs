//! Highly inspired by `metrics-exporter-prometheus`.
use std::{
    borrow::Cow,
    collections::BTreeMap,
    fmt::{Display, Write},
    iter,
};

use cow_utils::CowUtils;
use fxhash::FxHashSet;
use metrics::{Key, Label};
use metrics_util::MetricKind;

use super::RenderOptions;
use crate::protocol::{Distribution, Metrics, Snapshot};

#[derive(Default)]
pub(super) struct PrometheusRenderer {
    prev_size: usize,
    // The renderer renders new counters with `0` for the first time.
    // See https://www.section.io/blog/beware-prometheus-counters-that-do-not-begin-at-zero/.
    // We store only hashes of `MetricMeta` because `insert()` API is bad for compound values.
    known_counters: FxHashSet<u64>,
}

impl PrometheusRenderer {
    pub(super) fn render(&mut self, snapshot: &Snapshot, options: RenderOptions<'_>) -> String {
        let mut output = String::with_capacity(self.prev_size * 5 / 4);
        render(&mut output, snapshot, options, &mut self.known_counters);
        self.prev_size = output.len();
        output
    }
}

fn render(
    buffer: &mut String,
    snapshot: &Snapshot,
    options: RenderOptions<'_>,
    known_counters: &mut FxHashSet<u64>,
) {
    for ((kind, original_name), by_labels) in group_by_name(snapshot) {
        let name = &*sanitize_name(original_name);

        if let Some(desc) = options.descriptions.get(original_name) {
            write_help_line(buffer, name, desc);
        }

        write_type_line(buffer, name, kind);

        for (meta, value) in by_labels {
            let actor_group_label = Label::new("actor_group", meta.actor_group.to_string());
            let actor_key_label = meta
                .actor_key
                .map(|k| Label::new("actor_key", k.to_string()));

            let labels = options
                .global_labels
                .iter()
                .chain(iter::once(&actor_group_label))
                .chain(actor_key_label.as_ref())
                .chain(meta.key.labels());

            match value {
                MetricValue::Counter(value) => {
                    let value = if known_counters.insert(fxhash::hash64(&meta)) {
                        0
                    } else {
                        value
                    };
                    write_metric_line(buffer, name, None, labels.clone(), value);
                }
                MetricValue::Gauge(value) => {
                    write_metric_line(buffer, name, None, labels.clone(), value);
                }
                MetricValue::Distribution(distribution) => {
                    for (quantile, label) in options.quantiles {
                        let value = distribution.quantile(quantile.value()).unwrap_or(0.0);
                        let all_labels = labels.clone().chain(iter::once(label));
                        write_metric_line(buffer, name, None, all_labels, value);
                    }

                    let min = distribution.min();
                    let max = distribution.max();

                    let (sum, count) = if known_counters.insert(fxhash::hash64(&meta)) {
                        (0., 0)
                    } else {
                        (distribution.sum(), distribution.count())
                    };

                    // TODO: write types for this.
                    write_metric_line(buffer, name, Some("sum"), labels.clone(), sum);
                    write_metric_line(buffer, name, Some("count"), labels.clone(), count);
                    write_metric_line(buffer, name, Some("min"), labels.clone(), min);
                    write_metric_line(buffer, name, Some("max"), labels.clone(), max);
                }
            }
        }

        buffer.push('\n');
    }
}

type GroupedData<'a> = BTreeMap<(MetricKind, &'a str), BTreeMap<MetricMeta<'a>, MetricValue<'a>>>;

#[derive(Hash, PartialEq, Eq, PartialOrd, Ord)]
struct MetricMeta<'a> {
    actor_group: &'a str,
    actor_key: Option<&'a str>,
    key: &'a Key,
}

enum MetricValue<'a> {
    Counter(u64),
    Gauge(f64),
    Distribution(&'a Distribution),
}

fn group_by_name(snapshot: &Snapshot) -> GroupedData<'_> {
    let mut data: GroupedData<'_> = BTreeMap::new();

    for (group, per_group) in &snapshot.per_group {
        for (key, value, kind) in iter_metrics(per_group) {
            data.entry((kind, key.name())).or_default().insert(
                MetricMeta {
                    actor_group: group,
                    actor_key: None,
                    key,
                },
                value,
            );
        }
    }

    for (actor_meta, per_actor) in &snapshot.per_actor {
        for (key, value, kind) in iter_metrics(per_actor) {
            data.entry((kind, key.name())).or_default().insert(
                MetricMeta {
                    actor_group: &actor_meta.group,
                    actor_key: Some(&actor_meta.key),
                    key,
                },
                value,
            );
        }
    }

    data
}

fn iter_metrics(metrics: &Metrics) -> impl Iterator<Item = (&Key, MetricValue<'_>, MetricKind)> {
    let c = metrics
        .counters
        .iter()
        .map(|(k, v)| (k, MetricValue::Counter(*v), MetricKind::Counter));
    let g = metrics
        .gauges
        .iter()
        .map(|(k, v)| (k, MetricValue::Gauge(*v), MetricKind::Gauge));
    let d = metrics
        .distributions
        .iter()
        .map(|(k, v)| (k, MetricValue::Distribution(v), MetricKind::Histogram));

    c.chain(g).chain(d)
}

fn write_help_line(buffer: &mut String, name: &str, desc: &str) {
    buffer.push_str("# HELP ");
    buffer.push_str(name);
    buffer.push(' ');
    buffer.push_str(desc);
    buffer.push('\n');
}

fn write_type_line(buffer: &mut String, name: &str, kind: MetricKind) {
    buffer.push_str("# TYPE ");
    buffer.push_str(name);
    buffer.push(' ');
    buffer.push_str(match kind {
        MetricKind::Counter => "counter",
        MetricKind::Gauge => "gauge",
        MetricKind::Histogram => "summary",
    });
    buffer.push('\n');
}

fn write_metric_line<'a, V: Display>(
    buffer: &mut String,
    name: &'a str,
    suffix: Option<&'static str>,
    mut labels: impl Iterator<Item = &'a Label>,
    value: V,
) {
    buffer.push_str(name);
    if let Some(suffix) = suffix {
        buffer.push('_');
        buffer.push_str(suffix)
    }

    if let Some(first_label) = labels.next() {
        buffer.push('{');
        write_label(buffer, first_label);

        for label in labels {
            buffer.push(',');
            write_label(buffer, label);
        }

        buffer.push('}');
    }

    buffer.push(' ');
    let _ = write!(buffer, "{}", value);
    buffer.push('\n');
}

fn write_label(buffer: &mut String, label: &Label) {
    buffer.push_str(&sanitize_label_key(label.key()));
    buffer.push_str("=\"");
    buffer.push_str(&sanitize_label_value(label.value()));
    buffer.push('"');
}

fn sanitize_name(name: &str) -> Cow<'_, str> {
    // [a-zA-Z_:][a-zA-Z0-9_:]*
    let forbidden = |c: char| !(c.is_alphanumeric() || c == '_' || c == ':');
    name.cow_replace(forbidden, "_")
}

fn sanitize_label_key(key: &str) -> Cow<'_, str> {
    // [a-zA-Z_][a-zA-Z0-9_]*
    let forbidden = |c: char| !(c.is_alphanumeric() || c == '_');
    key.cow_replace(forbidden, "_")
}

fn sanitize_label_value(value: &str) -> Cow<'_, str> {
    if value.contains(|c: char| c == '\\' || c == '"' || c == '\n') {
        value.into()
    } else {
        value
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .into()
    }
}
