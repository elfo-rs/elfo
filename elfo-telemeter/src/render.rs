use fxhash::FxHashMap;
use metrics::Label;
use metrics_util::{parse_quantiles, Quantile};

use crate::{config::Config, protocol::Snapshot};

mod prometheus;

#[derive(Default)]
pub(crate) struct Renderer {
    quantiles: Vec<(Quantile, Label)>,
    global_labels: Vec<Label>,
}

struct RenderOptions<'a> {
    quantiles: &'a [(Quantile, Label)],
    descriptions: &'a FxHashMap<String, &'static str>,
    global_labels: &'a [Label],
}

impl Renderer {
    pub(crate) fn configure(&mut self, config: &Config) {
        self.quantiles = parse_quantiles(&config.quantiles)
            .into_iter()
            .map(|q| {
                let label = Label::new("quantile", q.value().to_string());
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
        &self,
        snapshot: &Snapshot,
        descriptions: &FxHashMap<String, &'static str>,
    ) -> String {
        let options = RenderOptions {
            quantiles: &self.quantiles,
            descriptions,
            global_labels: &self.global_labels,
        };

        prometheus::render(snapshot, options)
    }
}
