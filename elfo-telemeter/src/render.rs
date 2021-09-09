//! A lot of code here is copy-pasted from `metrics-exporter-prometheus`.

use std::fmt::Display;

use fxhash::FxHashMap;
use metrics_util::Quantile;

use crate::{distribution::Distribution, storage::Snapshot};

pub(crate) struct RenderOptions<'a> {
    pub(crate) quantiles: &'a [Quantile],
    pub(crate) descriptions: &'a FxHashMap<String, &'static str>,
}

pub(crate) fn render(snapshot: Snapshot, options: RenderOptions<'_>) -> String {
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
            write_metric_line::<&str, u64>(&mut output, &name, None, &labels, None, value);
        }
        output.push('\n');
    }

    for (name, mut by_labels) in gauges.drain() {
        if let Some(desc) = options.descriptions.get(name.as_str()) {
            write_help_line(&mut output, name.as_str(), desc);
        }

        write_type_line(&mut output, name.as_str(), "gauge");
        for (labels, value) in by_labels.drain() {
            write_metric_line::<&str, f64>(&mut output, &name, None, &labels, None, value);
        }
        output.push('\n');
    }

    for (name, mut by_labels) in distributions.drain() {
        if let Some(desc) = options.descriptions.get(name.as_str()) {
            write_help_line(&mut output, name.as_str(), desc);
        }

        write_type_line(&mut output, name.as_str(), "summary");
        for (labels, distribution) in by_labels.drain() {
            let (sum, count, min, max) = match distribution {
                Distribution::Summary(summary, sum) => {
                    for quantile in options.quantiles {
                        let value = summary.quantile(quantile.value()).unwrap_or(0.0);
                        write_metric_line(
                            &mut output,
                            &name,
                            None,
                            &labels,
                            Some(("quantile", quantile.value())),
                            value,
                        );
                    }

                    (sum, summary.count() as u64, summary.min(), summary.max())
                }
            };

            write_metric_line::<&str, f64>(&mut output, &name, Some("sum"), &labels, None, sum);
            write_metric_line::<&str, u64>(&mut output, &name, Some("count"), &labels, None, count);
            write_metric_line::<&str, f64>(&mut output, &name, Some("min"), &labels, None, min);
            write_metric_line::<&str, f64>(&mut output, &name, Some("max"), &labels, None, max);
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

fn write_metric_line<L: Display, V: Display>(
    buffer: &mut String,
    name: &str,
    suffix: Option<&'static str>,
    labels: &[String],
    additional_label: Option<(&'static str, L)>,
    value: V,
) {
    buffer.push_str(name);
    if let Some(suffix) = suffix {
        buffer.push('_');
        buffer.push_str(suffix)
    }

    if !labels.is_empty() || additional_label.is_some() {
        buffer.push('{');

        let mut first = true;
        for label in labels {
            if first {
                first = false;
            } else {
                buffer.push(',');
            }
            buffer.push_str(label);
        }

        if let Some((name, value)) = additional_label {
            if !first {
                buffer.push(',');
            }
            buffer.push_str(name);
            buffer.push_str("=\"");
            buffer.push_str(value.to_string().as_str());
            buffer.push('"');
        }

        buffer.push('}');
    }

    buffer.push(' ');
    buffer.push_str(value.to_string().as_str());
    buffer.push('\n');
}
