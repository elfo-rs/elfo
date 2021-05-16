use std::{
    fmt::{self, Write as _},
    sync::Arc,
};

use tracing::Level;

use elfo_core::{trace_id::TraceId, ActorGroup, Context, Schema, _priv::ObjectMeta};
use elfo_macros::{message, msg_raw as msg};

use crate::{config::Config, PreparedEvent, Shared};

pub(crate) struct Logger {
    ctx: Context<Config>,
    shared: Arc<Shared>,
}

impl Logger {
    pub(crate) fn new(shared: Arc<Shared>) -> Schema {
        ActorGroup::new()
            .config::<Config>()
            .exec(move |ctx| Logger::ctor(ctx, shared.clone()).main())
    }

    fn ctor(ctx: Context<Config>, shared: Arc<Shared>) -> Self {
        Self { ctx, shared }
    }

    async fn main(mut self) {
        let mut buffer = String::with_capacity(1024);

        // Note that we don't use `elfo::stream::Stream` here intentionally
        // to avoid cyclic dependences (`Context::recv()` logs all messages).
        loop {
            tokio::select! {
                event = self.shared.channel.receive() => {
                    let event = event.expect("channel cannot close");
                    buffer.clear();
                    self.format_event(event, &mut buffer);
                    // TODO: support files.
                    print!("{}", buffer);
                },
                envelope = self.ctx.recv() => {
                    // TODO
                }
            }
        }
    }

    pub(super) fn format_event(&self, event: PreparedEvent, buffer: &mut String) {
        let payload = self
            .shared
            .pool
            .get(event.payload_id)
            .expect("unknown string");
        self.shared.pool.clear(event.payload_id);

        let _ = write!(
            buffer,
            "{timestamp} {level} [{trace_id}] {object} - {payload}",
            timestamp = humantime::format_rfc3339_nanos(event.timestamp),
            level = LevelFormatter(event.level),
            trace_id = TraceIdFormatter(event.trace_id),
            object = ObjectFormatter(&event.object),
            payload = PayloadFormatter(&payload),
        );

        // Add ancestors' fields.
        let mut span_id = event.span_id;
        while let Some(data) = span_id.and_then(|span_id| self.shared.spans.get(&span_id)) {
            span_id = data.parent_id.clone();
            let payload = self
                .shared
                .pool
                .get(data.payload_id)
                .expect("unknown string");
            let _ = write!(buffer, "{}", PayloadFormatter(&payload));
        }

        let _ = writeln!(buffer);
    }
}

struct LevelFormatter(Level);

impl fmt::Display for LevelFormatter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let str = match self.0 {
            Level::ERROR => "ERROR",
            Level::WARN => "WARN ",
            Level::INFO => "INFO ",
            Level::DEBUG => "DEBUG",
            Level::TRACE => "TRACE",
        };

        f.write_str(str)
    }
}

struct TraceIdFormatter(Option<TraceId>);

impl fmt::Display for TraceIdFormatter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(trace_id) = self.0 {
            fmt::Display::fmt(&trace_id, f)
        } else {
            Ok(())
        }
    }
}

struct ObjectFormatter<'a>(&'a Option<Arc<ObjectMeta>>);

impl fmt::Display for ObjectFormatter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(meta) = self.0 {
            if let Some(key) = meta.key.as_ref() {
                f.write_fmt(format_args!("{}.{}", meta.group, key))
            } else {
                f.write_str(&meta.group)
            }
        } else {
            Ok(())
        }
    }
}

struct PayloadFormatter<'a>(&'a str);

impl fmt::Display for PayloadFormatter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: what about `\t`?
        for (idx, chunk) in self.0.split('\n').enumerate() {
            if idx > 0 {
                f.write_str("\\n")?;
            }

            f.write_str(chunk)?;
        }

        Ok(())
    }
}
