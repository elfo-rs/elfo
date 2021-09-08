use metrics::{self, Key, Label};
use quanta::Instant;

use crate::{envelope::Envelope, message::Message};

#[derive(Default)]
pub(super) struct Stats {
    in_handling: Option<InHandling>,
}

struct InHandling {
    labels: &'static [Label],
    start_time: quanta::Instant,
}

impl Stats {
    pub(super) fn on_new_message(&mut self, envelope: &Envelope) {
        let _recorder = ward!(metrics::try_recorder());
        self.in_handling = Some(InHandling {
            labels: envelope.message().labels(),
            start_time: Instant::now(),
        });
    }

    pub(super) fn message_handling_time_seconds(&mut self) {
        let recorder = ward!(metrics::try_recorder());
        let in_handling = ward!(self.in_handling.take());
        let key = Key::from_static_parts("message_handling_time_seconds", in_handling.labels);
        let value = (Instant::now() - in_handling.start_time).as_secs_f64();
        recorder.record_histogram(&key, value);
    }

    pub(super) fn sent_messages_total<M: Message>(&self) {
        let recorder = ward!(metrics::try_recorder());
        let key = Key::from_static_parts("sent_messages_total", M::LABELS);
        recorder.increment_counter(&key, 1);
    }
}
