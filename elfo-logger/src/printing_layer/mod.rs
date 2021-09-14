use std::{sync::Arc, time::SystemTime};

use metrics::{Key, Label};
use tracing::{span, Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use elfo_core::scope;

use self::visitor::Visitor;
use crate::{PreparedEvent, Shared, SpanData, StringId};

mod visitor;

pub struct PrintingLayer {
    shared: Arc<Shared>,
}

impl PrintingLayer {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    fn prepare(
        &self,
        simplify_message: bool,
        f: impl FnOnce(&mut Visitor<'_>),
    ) -> Option<StringId> {
        self.shared.pool.create_with(|payload| {
            let mut visitor = Visitor::new(&self.shared, payload, simplify_message);
            f(&mut visitor);
        })
    }
}

impl<S: Subscriber> Layer<S> for PrintingLayer {
    fn new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let parent_id = if attrs.is_root() {
            None
        } else {
            let current_span = ctx.current_span();
            attrs.parent().or_else(|| current_span.id()).cloned()
        };
        let payload_id = ward!(self.prepare(false, |visitor| attrs.record(visitor)));
        let span = SpanData::new(parent_id, payload_id);
        self.shared.spans.insert(id.clone(), span);
    }

    fn on_record(&self, id: &span::Id, record: &span::Record<'_>, _: Context<'_, S>) {
        let mut data = ward!(self.shared.spans.get_mut(id));
        let old_payload_id = data.payload_id;
        let old_payload = ward!(self.shared.pool.get(old_payload_id));

        let payload_id = ward!(self.prepare(false, |visitor| {
            visitor.push(&old_payload);
            record.record(visitor);
        }));

        self.shared.pool.clear(old_payload_id);
        data.payload_id = payload_id;
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let current_span = ctx.current_span();
        let payload_id = ward!(self.prepare(true, |visitor| event.record(visitor)));

        let data = scope::try_with(|scope| (scope.meta().clone(), scope.trace_id()));
        let (object, trace_id) = match data {
            Some((meta, trace_id)) => (Some(meta), Some(trace_id)),
            None => (None, None),
        };

        let event = PreparedEvent {
            timestamp: now(),
            trace_id,
            metadata: event.metadata(),
            object,
            span_id: event.parent().or_else(|| current_span.id()).cloned(),
            payload_id,
        };

        let level = *event.metadata.level();
        let is_lost = self.shared.channel.try_send(event).is_err();

        emit_metrics(level, is_lost);
    }

    fn on_close(&self, id: span::Id, _: Context<'_, S>) {
        if let Some((_, data)) = self.shared.spans.remove(&id) {
            self.shared.pool.clear(data.payload_id);
        }
    }
}

fn emit_metrics(level: Level, is_lost: bool) {
    let recorder = ward!(metrics::try_recorder());
    let labels = labels_by_level(level);
    let key = Key::from_static_parts("elfo_events_total", labels);
    recorder.increment_counter(&key, 1);

    if is_lost {
        let labels = labels_by_level(level);
        let key = Key::from_static_parts("elfo_lost_events_total", labels);
        recorder.increment_counter(&key, 1);
    }
}

fn labels_by_level(level: Level) -> &'static [Label] {
    const fn f(value: &'static str) -> Label {
        Label::from_static_parts("level", value)
    }

    const TRACE_LABELS: &[Label] = &[f("Trace")];
    const DEBUG_LABELS: &[Label] = &[f("Debug")];
    const INFO_LABELS: &[Label] = &[f("Info")];
    const WARN_LABELS: &[Label] = &[f("Warn")];
    const ERROR_LABELS: &[Label] = &[f("Error")];

    match level {
        Level::TRACE => TRACE_LABELS,
        Level::DEBUG => DEBUG_LABELS,
        Level::INFO => INFO_LABELS,
        Level::WARN => WARN_LABELS,
        Level::ERROR => ERROR_LABELS,
    }
}

// TODO: use the `quanta` crate.
#[cfg(not(test))]
fn now() -> SystemTime {
    SystemTime::now()
}

#[cfg(test)]
fn now() -> SystemTime {
    humantime::parse_rfc3339("2021-05-17T20:20:20.123456789Z").unwrap()
}
