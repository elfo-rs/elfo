use std::sync::Arc;

use tokio::{
    fs::{File, OpenOptions},
    io::AsyncWriteExt,
};

use elfo_core as elfo;
use elfo_macros::{message, msg_raw as msg};

use elfo::{
    messages::ConfigUpdated,
    signal::{Signal, SignalKind},
    ActorGroup, Context, Schema,
};

use crate::{
    config::{Config, Sink},
    formatters::Formatter,
    theme, PreparedEvent, Shared,
};

pub(crate) struct Logger {
    ctx: Context<Config>,
    shared: Arc<Shared>,
}

/// Reload a log file, usually after rotation.
#[non_exhaustive]
#[message(elfo = elfo_core)]
#[derive(Default)]
pub struct ReopenLogFile {}

impl Logger {
    // TODO: rename it?
    #[allow(clippy::new_ret_no_self)]
    pub(crate) fn new(shared: Arc<Shared>) -> Schema {
        ActorGroup::new()
            .config::<Config>()
            .exec(move |ctx| Logger::ctor(ctx, shared.clone()).main())
    }

    fn ctor(ctx: Context<Config>, shared: Arc<Shared>) -> Self {
        Self { ctx, shared }
    }

    async fn main(self) {
        let mut buffer = String::with_capacity(1024);
        let mut file = open_file(self.ctx.config()).await;
        let mut use_colors = can_use_colors(self.ctx.config());

        let signal = Signal::new(SignalKind::Hangup, ReopenLogFile::default);
        let mut ctx = self.ctx.clone().with(&signal);

        // Note that we don't use `elfo::stream::Stream` here intentionally
        // to avoid cyclic dependences (`Context::recv()` logs all messages).
        loop {
            tokio::select! {
                event = self.shared.channel.receive() => {
                    let event = event.expect("channel cannot close");
                    buffer.clear();

                    if use_colors {
                        self.format_event::<theme::ColoredTheme>(&mut buffer, event);
                    } else {
                        self.format_event::<theme::PlainTheme>(&mut buffer, event);
                    }

                    if let Some(file) = file.as_mut() {
                        // TODO: what about performance here?
                        file.write_all(buffer.as_ref()).await.expect("cannot write to the config file");
                    } else {
                        print!("{}", buffer);
                    }
                },
                envelope = ctx.recv() => {
                    let envelope = ward!(envelope, break);
                    msg!(match envelope {
                        ReopenLogFile | ConfigUpdated => {
                            file = open_file(self.ctx.config()).await;
                            use_colors = can_use_colors(self.ctx.config());
                        },
                    });
                },
            }
        }
    }

    pub(super) fn format_event<T: theme::Theme>(&self, out: &mut String, event: PreparedEvent) {
        let payload = self
            .shared
            .pool
            .get(event.payload_id)
            .expect("unknown string");
        self.shared.pool.clear(event.payload_id);

        // <timestamp> <level> [<trace_id>] <object> - <message>\t<fields>

        T::Timestamp::fmt(out, &event.timestamp);
        out.push(' ');
        T::Level::fmt(out, &event.level);
        out.push_str(" [");
        T::TraceId::fmt(out, &event.trace_id);
        out.push_str("] ");
        T::ObjectMeta::fmt(out, &event.object);
        out.push_str(" - ");
        T::Payload::fmt(out, &payload);

        // Add ancestors' fields.
        let mut span_id = event.span_id;
        while let Some(data) = span_id.and_then(|span_id| self.shared.spans.get(&span_id)) {
            span_id = data.parent_id.clone();
            let payload = self
                .shared
                .pool
                .get(data.payload_id)
                .expect("unknown string");
            out.push_str(&payload);
        }

        out.push('\n');
    }
}

async fn open_file(config: &Config) -> Option<File> {
    if config.sink == Sink::Stdout {
        return None;
    }

    // TODO: rely on deserialize instead.
    let path = config
        .path
        .as_ref()
        .expect("the config path must be provided");

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .expect("cannot open the config file");

    Some(file)
}

fn can_use_colors(config: &Config) -> bool {
    config.sink == Sink::Stdout && atty::is(atty::Stream::Stdout)
}
