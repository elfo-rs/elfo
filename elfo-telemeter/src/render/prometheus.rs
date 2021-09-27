//! A lot of code here is copy-pasted from `metrics-exporter-prometheus`.
use std::{
    fmt::{Display, Write},
    iter,
};

use metrics::Label;

use super::RenderOptions;
use crate::{distribution::Distribution, storage::Snapshot};

pub(super) fn render(snapshot: Snapshot, options: RenderOptions<'_>) -> String {
    let Snapshot {
        mut counters,
        mut distributions,
        mut gauges,
    } = snapshot;

    let mut output = String::new();

    for (name, mut by_labels) in counters.drain() {
        if let Some(desc) = options.descriptions.get(name.as_str()) {
            write_help_line(&mut output, name.as_str(), desc);
        }

        write_type_line(&mut output, name.as_str(), "counter");
        for (labels, value) in by_labels.drain() {
            let all_labels = options.global_labels.iter().chain(labels.iter());
            write_metric_line(&mut output, &name, None, all_labels, value);
        }
        output.push('\n');
    }

    for (name, mut by_labels) in gauges.drain() {
        if let Some(desc) = options.descriptions.get(name.as_str()) {
            write_help_line(&mut output, name.as_str(), desc);
        }

        write_type_line(&mut output, name.as_str(), "gauge");
        for (labels, value) in by_labels.drain() {
            let all_labels = options.global_labels.iter().chain(labels.iter());
            write_metric_line(&mut output, &name, None, all_labels, value);
        }
        output.push('\n');
    }

    for (name, mut by_labels) in distributions.drain() {
        if let Some(desc) = options.descriptions.get(name.as_str()) {
            write_help_line(&mut output, name.as_str(), desc);
        }

        write_type_line(&mut output, name.as_str(), "summary");
        for (labels, distribution) in by_labels.drain() {
            let all_labels = options.global_labels.iter().chain(labels.iter());

            let (sum, count, min, max) = match distribution {
                Distribution::Summary(summary, sum) => {
                    for (quantile, label) in options.quantiles {
                        let value = summary.quantile(quantile.value()).unwrap_or(0.0);
                        let all_labels = all_labels.clone().chain(iter::once(label));
                        write_metric_line(&mut output, &name, None, all_labels, value);
                    }

                    (sum, summary.count() as u64, summary.min(), summary.max())
                }
            };

            write_metric_line(&mut output, &name, Some("sum"), all_labels.clone(), sum);
            write_metric_line(&mut output, &name, Some("count"), all_labels.clone(), count);
            write_metric_line(&mut output, &name, Some("min"), all_labels.clone(), min);
            write_metric_line(&mut output, &name, Some("max"), all_labels.clone(), max);
        }

        output.push('\n');
    }

    output
}

fn write_help_line(buffer: &mut String, name: &str, desc: &str) {
    buffer.push_str("# HELP ");
    buffer.push_str(name);
    buffer.push(' ');
    buffer.push_str(desc);
    buffer.push('\n');
}

fn write_type_line(buffer: &mut String, name: &str, metric_type: &str) {
    buffer.push_str("# TYPE ");
    buffer.push_str(name);
    buffer.push(' ');
    buffer.push_str(metric_type);
    buffer.push('\n');
}

fn write_metric_line<'a, V: Display>(
    buffer: &mut String,
    name: &str,
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
    let label_value = label
        .value()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");

    buffer.push_str(label.key());
    buffer.push_str("=\"");
    buffer.push_str(&label_value);
    buffer.push('"');
}
