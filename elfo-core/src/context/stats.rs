use derive_more::Constructor;
use metrics::{self, Key, Label};

use elfo_utils::{CachePadded, time::Instant};

use crate::{envelope::Envelope, message::Message};

pub(super) struct Stats {
    in_handling: Option<InHandling>,
}

#[derive(Constructor)]
struct InHandling {
    key: &'static Key,
    start_time: Instant,
}

const STARTUP_LABELS: &[Label] = &[Label::from_static_parts("message", "<Startup>")];
const EMPTY_MAILBOX_LABELS: &[Label] = &[Label::from_static_parts("message", "<EmptyMailbox>")];

struct Keys {
    startup: Key,
    empty_mailbox: Key,
    waiting_time: Key,
}

static KEYS: CachePadded<Keys> = CachePadded::new(Keys {
    startup: Key::from_static_parts("elfo_message_handling_time_seconds", STARTUP_LABELS),
    empty_mailbox: Key::from_static_parts(
        "elfo_message_handling_time_seconds",
        EMPTY_MAILBOX_LABELS,
    ),
    waiting_time: Key::from_static_name("elfo_message_waiting_time_seconds"),
});

impl Stats {
    pub(super) fn empty() -> Self {
        Self { in_handling: None }
    }

    pub(super) fn startup() -> Self {
        Self {
            in_handling: Some(InHandling::new(&KEYS.startup, Instant::now())),
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
        recorder.record_histogram(&KEYS.waiting_time, value);

        self.in_handling = Some(InHandling::new(
            envelope.message()._vtable().handling_time_key(),
            now,
        ));
    }

    pub(super) fn on_empty_mailbox(&mut self) {
        debug_assert!(self.in_handling.is_none());

        self.in_handling = Some(InHandling::new(&KEYS.empty_mailbox, Instant::now()));
    }

    pub(super) fn on_sent_message(&self, message: &impl Message) {
        let recorder = ward!(metrics::try_recorder());
        recorder.increment_counter(message._vtable().sent_messages_key(), 1);
    }

    fn emit_handling_time(&mut self) {
        let in_handling = ward!(self.in_handling.take());
        let recorder = ward!(metrics::try_recorder());

        let value = in_handling.start_time.elapsed_secs_f64();
        recorder.record_histogram(in_handling.key, value);
    }
}

impl Drop for Stats {
    fn drop(&mut self) {
        self.emit_handling_time();
    }
}
