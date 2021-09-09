use metrics_util::Summary;

#[derive(Clone)]
pub(crate) enum Distribution {
    /// A Prometheus summary.
    ///
    /// Computes and exposes value quantiles directly to Prometheus i.e. 50% of
    /// requests were faster than 200ms, and 99% of requests were faster than
    /// 1000ms, etc.
    Summary(Summary, f64),
}

impl Distribution {
    pub(crate) fn new_summary() -> Distribution {
        let summary = Summary::with_defaults();
        Distribution::Summary(summary, 0.0)
    }

    pub(crate) fn record_samples(&mut self, samples: &[f64]) {
        match self {
            Distribution::Summary(hist, sum) => {
                for sample in samples {
                    hist.add(*sample);
                    *sum += *sample;
                }
            }
        }
    }
}
