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
    line_buffer::LineBuffer,
    line_transaction::{FailOnUnfit, Line as _, LineFactory, TruncateOnUnfit},
    theme, PreparedEvent, Shared,
};

pub(crate) struct Logger {
    ctx: Context<Config>,
    shared: Arc<Shared>,
    filtering_layer: FilteringLayer,

    buffer: LineBuffer,
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
        let buffer = LineBuffer::with_capacity(1024, {
            let cfg = ctx.config();
            cfg.max_line_size.0 as _
        });

        Self {
            ctx,
            shared,
            filtering_layer,
            buffer,
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

                    self.format_event(use_colors, event);

                    if let Some(file) = file.as_mut() {
                        // TODO: what about performance here?
                        file.write_all(self.buffer.as_str().as_bytes()).await.expect("cannot write to the config file");
                    } else {
                        print!("{}", self.buffer.as_str());
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
                            self.buffer.configure(self.ctx.config().max_line_size.0 as _);
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

    fn format_event(&mut self, use_colors: bool, event: PreparedEvent) {
        // boolean operator || is short-circuit
        let successful = if use_colors {
            self.do_format_event::<theme::ColoredTheme, FailOnUnfit>(&event)
                || self.do_format_event::<theme::ColoredTheme, TruncateOnUnfit>(&event)
        } else {
            self.do_format_event::<theme::PlainTheme, FailOnUnfit>(&event)
                || self.do_format_event::<theme::PlainTheme, TruncateOnUnfit>(&event)
        };

        if successful {
            self.shared.pool.clear(event.payload_id);
        } else {
            unreachable!("truncation must succeed")
        }
    }

    fn do_format_event<T: theme::Theme, F: LineFactory>(&mut self, event: &PreparedEvent) -> bool {
        let config = self.ctx.config();
        let mut line = F::create_line(&mut self.buffer);

        let payload = self
            .shared
            .pool
            .get(event.payload_id)
            .expect("unknown string");

        // <timestamp> <level> [<trace_id>] <object> - <message>\t<fields>

        T::Timestamp::fmt(line.meta_mut(), &event.timestamp);
        line.meta_mut().push(' ');
        T::Level::fmt(line.meta_mut(), event.metadata.level());
        line.meta_mut().push_str(" [");
        T::TraceId::fmt(line.meta_mut(), &event.trace_id);
        line.meta_mut().push_str("] ");
        T::ActorMeta::fmt(line.payload_mut(), &event.object);
        line.payload_mut().push_str(" - ");
        T::Payload::fmt(line.payload_mut(), &payload);

        // Add ancestors' fields.
        let mut span_id = event.span_id.clone();

        {
            let payload_buffer = line.payload_mut();
            while let Some(data) = span_id
                .as_ref()
                .and_then(|span_id| self.shared.spans.get(span_id))
            {
                span_id.clone_from(&data.parent_id);

                let payload = self
                    .shared
                    .pool
                    .get(data.payload_id)
                    .expect("unknown string");

                T::Payload::fmt(payload_buffer, &payload);
            }
        }

        if config.format.with_location {
            if let Some(location) = extract_location(event.metadata) {
                let fields_buffer = line.fields_mut();
                fields_buffer.push('\t');
                T::Location::fmt(line.fields_mut(), &location);
            }
        }

        if config.format.with_module {
            if let Some(module) = event.metadata.module_path() {
                let fields_buffer = line.fields_mut();
                fields_buffer.push('\t');
                T::Module::fmt(fields_buffer, module);
            }
        }

        line.try_commit()
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
