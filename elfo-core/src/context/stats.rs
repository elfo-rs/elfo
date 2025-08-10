use derive_more::Constructor;
use metrics::{self, Key, Label};

use elfo_utils::time::Instant;

use crate::{envelope::Envelope, message::Message};

pub(super) struct Stats {
    in_handling: Option<InHandling>,
}

#[derive(Constructor)]
struct InHandling {
    labels: &'static [Label],
    start_time: Instant,
}

static STARTUP_LABELS: &[Label] = &[Label::from_static_parts("message", "<Startup>")];
static EMPTY_MAILBOX_LABELS: &[Label] = &[Label::from_static_parts("message", "<EmptyMailbox>")];

static WAITING_TIME_KEY: Key = Key::from_static_name("elfo_message_waiting_time_seconds");

impl Stats {
    pub(super) fn empty() -> Self {
        Self { in_handling: None }
    }

    pub(super) fn startup() -> Self {
        Self {
            in_handling: Some(InHandling::new(STARTUP_LABELS, Instant::now())),
        }
    }

    pub(super) fn on_recv(&mut self) {
        self.emit_handling_time();
    }

    pub(super) fn on_received_envelope(&mut self, envelope: &Envelope) {
        debug_assert!(self.in_handling.is_none());

        let recorder = ward!(metrics::try_recorder());
        let now = Instant::now();

        // Now envelope cannot be forwarded, so use the created time as a start time.
        let value = now.secs_f64_since(envelope.created_time());
        recorder.record_histogram(&WAITING_TIME_KEY, value);

        self.in_handling = Some(InHandling::new(envelope.message().labels(), now));
    }

    pub(super) fn on_empty_mailbox(&mut self) {
        debug_assert!(self.in_handling.is_none());

        self.in_handling = Some(InHandling::new(EMPTY_MAILBOX_LABELS, Instant::now()));
    }

    pub(super) fn on_sent_message(&self, message: &impl Message) {
        let recorder = ward!(metrics::try_recorder());
        let key = Key::from_static_parts("elfo_sent_messages_total", message.labels());
        recorder.increment_counter(&key, 1);
    }

    fn emit_handling_time(&mut self) {
        let in_handling = ward!(self.in_handling.take());
        let recorder = ward!(metrics::try_recorder());

        // TODO: key creation is not optimal because the hash is not cached.
        //       Consider storing keys in a message's vtable.
        let key = Key::from_static_parts("elfo_message_handling_time_seconds", in_handling.labels);

        let value = Instant::now().secs_f64_since(in_handling.start_time);
        recorder.record_histogram(&key, value);
    }
}

impl Drop for Stats {
    fn drop(&mut self) {
        self.emit_handling_time();
    }
}
