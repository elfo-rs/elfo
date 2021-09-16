use std::{sync::Arc, time::SystemTime};

use tracing::{span, Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use elfo_core::scope;

use self::visitor::Visitor;
use crate::{stats, PreparedEvent, Shared, SpanData, StringId};

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
        let level = *event.metadata().level();
        let payload_id = ward!(self.prepare(true, |visitor| event.record(visitor)), {
            stats::counter_per_level("elfo_lost_events_total", level);
            return;
        });

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

        let is_lost = self.shared.channel.try_send(event).is_err();
        if is_lost {
            self.shared.pool.clear(payload_id);
            stats::counter_per_level("elfo_lost_events_total", level);
        } else {
            stats::counter_per_level("elfo_emitted_events_total", level);
        }
    }

    fn on_close(&self, id: span::Id, _: Context<'_, S>) {
        if let Some((_, data)) = self.shared.spans.remove(&id) {
            self.shared.pool.clear(data.payload_id);
        }
    }
}

#[cfg(not(test))]
fn now() -> SystemTime {
    SystemTime::now()
}

#[cfg(test)]
fn now() -> SystemTime {
    humantime::parse_rfc3339("2021-05-17T20:20:20.123456789Z").unwrap()
}
