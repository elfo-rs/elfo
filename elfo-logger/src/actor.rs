use std::{sync::Arc, time::Duration};

use metrics::increment_counter;
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncWriteExt,
};
use tracing::Metadata;

use elfo_core::{
    message,
    messages::{ConfigUpdated, Terminate},
    msg,
    signal::{Signal, SignalKind},
    ActorGroup, Blueprint, Context, RestartParams, RestartPolicy, TerminationPolicy,
};

use crate::{
    config::{Config, Sink},
    filtering_layer::FilteringLayer,
    formatters::Formatter,
    theme, PreparedEvent, Shared,
};

pub(crate) struct Logger {
    ctx: Context<Config>,
    shared: Arc<Shared>,
    filtering_layer: FilteringLayer,
    buffer: String,
}

/// Reload a log file, usually after rotation.
#[message]
#[derive(Default)]
#[non_exhaustive]
pub struct ReopenLogFile {}

impl Logger {
    // TODO: rename it?
    #[allow(clippy::new_ret_no_self)]
    pub(crate) fn blueprint(shared: Arc<Shared>, filtering_layer: FilteringLayer) -> Blueprint {
        ActorGroup::new()
            .config::<Config>()
            .termination_policy(TerminationPolicy::manually())
            .restart_policy(RestartPolicy::on_failure(RestartParams::new(
                Duration::from_secs(5),
                Duration::from_secs(30),
            )))
            .stop_order(105)
            .exec(move |ctx| Logger::new(ctx, shared.clone(), filtering_layer.clone()).main())
    }

    fn new(ctx: Context<Config>, shared: Arc<Shared>, filtering_layer: FilteringLayer) -> Self {
        filtering_layer.configure(&ctx.config().targets);
        Self {
            ctx,
            shared,
            filtering_layer,
            buffer: String::with_capacity(1024),
        }
    }

    async fn main(mut self) {
        let mut file = open_file(self.ctx.config()).await;
        let mut use_colors = can_use_colors(self.ctx.config());

        self.ctx.attach(Signal::new(
            SignalKind::UnixHangup,
            ReopenLogFile::default(),
        ));

        // Note that we don't use `elfo::stream::Stream` here intentionally
        // to avoid cyclic dependences (`Context::recv()` logs all messages).
        loop {
            tokio::select! {
                event = self.shared.channel.receive() => {
                    let event = ward!(event, break);

                    self.buffer.clear();

                    if use_colors {
                        self.format_event::<theme::ColoredTheme>(event);
                    } else {
                        self.format_event::<theme::PlainTheme>(event);
                    }

                    if let Some(file) = file.as_mut() {
                        // TODO: what about performance here?
                        file.write_all(self.buffer.as_ref()).await.expect("cannot write to the config file");
                    } else {
                        print!("{}", self.buffer);
                    }

                    increment_counter!("elfo_written_events_total");
                },
                envelope = self.ctx.recv() => {
                    let envelope = ward!(envelope, break);
                    msg!(match envelope {
                        ReopenLogFile => {
                            file = open_file(self.ctx.config()).await;
                            use_colors = can_use_colors(self.ctx.config());
                        },
                        ConfigUpdated => {
                            file = open_file(self.ctx.config()).await;
                            use_colors = can_use_colors(self.ctx.config());
                            self.filtering_layer.configure(&self.ctx.config().targets);
                        },
                        Terminate => {
                            // Close the channel and wait for the rest of the events.
                            self.shared.channel.close();
                        },
                    });
                },
            }
        }

        if let Some(mut file) = file {
            file.flush().await.expect("cannot flush the log file");
            file.sync_all().await.expect("cannot sync the log file");
        }
    }

    pub(super) fn format_event<T: theme::Theme>(&mut self, event: PreparedEvent) {
        let out = &mut self.buffer;
        let config = self.ctx.config();

        let payload = self
            .shared
            .pool
            .get(event.payload_id)
            .expect("unknown string");
        self.shared.pool.clear(event.payload_id);

        // <timestamp> <level> [<trace_id>] <object> - <message>\t<fields>

        T::Timestamp::fmt(out, &event.timestamp);
        out.push(' ');
        T::Level::fmt(out, event.metadata.level());
        out.push_str(" [");
        T::TraceId::fmt(out, &event.trace_id);
        out.push_str("] ");
        T::ActorMeta::fmt(out, &event.object);
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

            T::Payload::fmt(out, &payload);
        }

        if config.format.with_location {
            if let Some(location) = extract_location(event.metadata) {
                out.push('\t');
                T::Location::fmt(out, &location);
            }
        }

        if config.format.with_module {
            if let Some(module) = event.metadata.module_path() {
                out.push('\t');
                T::Module::fmt(out, module);
            }
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

fn extract_location(metadata: &Metadata<'static>) -> Option<(&'static str, u32)> {
    metadata
        .file()
        .map(|file| (file, metadata.line().unwrap_or_default()))
}
