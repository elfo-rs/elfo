use ahash::AHashMap;
use metrics::Label;

use self::openmetrics::OpenMetricsRenderer;
use crate::{
    config::{Config, Quantile},
    protocol::{Description, Snapshot},
};

mod openmetrics;

#[derive(Default)]
pub(crate) struct Renderer {
    quantiles: Vec<(Quantile, Label)>,
    global_labels: Vec<Label>,
    openmetrics: OpenMetricsRenderer,
}

struct RenderOptions<'a> {
    quantiles: &'a [(Quantile, Label)],
    descriptions: &'a AHashMap<String, Description>,
    global_labels: &'a [Label],
}

impl Renderer {
    pub(crate) fn configure(&mut self, config: &Config) {
        self.quantiles = config
            .quantiles
            .iter()
            .map(|&q| {
                let label = Label::new("quantile", (*q).to_string());
                (q, label)
            })
            .collect();

        self.global_labels = config
            .global_labels
            .iter()
            .cloned()
            .map(|(key, value)| Label::new(key, value))
            .collect();
    }

    pub(crate) fn render(
        &mut self,
        snapshot: &Snapshot,
        descriptions: &AHashMap<String, Description>,
    ) -> String {
        let options = RenderOptions {
            quantiles: &self.quantiles,
            descriptions,
            global_labels: &self.global_labels,
        };

        self.openmetrics.render(snapshot, options)
    }
}
