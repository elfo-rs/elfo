use std::time::Duration;

use metrics::gauge;

pub(crate) struct Rtt {
    ema: Option<f64>,
    alpha: f64,
}

impl Rtt {
    pub(crate) fn new(samples: usize) -> Self {
        // https://en.wikipedia.org/wiki/Moving_average#Relationship_between_SMA_and_EMA
        let alpha = 2.0 / (samples + 1) as f64;

        Self { ema: None, alpha }
    }

    pub(crate) fn push(&mut self, rtt: Duration) {
        let rtt = rtt.as_secs_f64();

        let ema = if let Some(ema) = self.ema {
            ema * (1.0 - self.alpha) + rtt * self.alpha
        } else {
            rtt
        };

        gauge!("elfo_network_rtt_seconds", ema);

        self.ema = Some(ema);
    }
}

impl Drop for Rtt {
    fn drop(&mut self) {
        if self.ema.is_some() {
            gauge!("elfo_network_rtt_seconds", f64::NAN);
        }
    }
}
