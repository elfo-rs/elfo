use std::sync::Arc;

use tracing::Level;

use elfo_core::{tracing::TraceId, ActorMeta};
use elfo_utils::time::SystemTime;

use crate::formatters::*;

pub(crate) trait Theme {
    type Timestamp: Formatter<SystemTime>;
    type Level: Formatter<Level>;
    type TraceId: Formatter<Option<TraceId>>;
    type ActorMeta: Formatter<Option<Arc<ActorMeta>>>;
    type Payload: Formatter<str>;
    type Location: Formatter<(&'static str, u32)>;
    type Module: Formatter<str>;
    type ResetStyle: Formatter<()>;
}

pub(crate) struct PlainTheme;

impl Theme for PlainTheme {
    type ActorMeta = EmptyIfNone<Arc<ActorMeta>>;
    type Level = Level;
    type Location = Location;
    type Module = Module;
    type Payload = Payload;
    type ResetStyle = DoNothing;
    type Timestamp = Rfc3339Weak;
    type TraceId = EmptyIfNone<TraceId>;
}

pub(crate) struct ColoredTheme;

impl Theme for ColoredTheme {
    type ActorMeta = EmptyIfNone<ColoredByHash<Arc<ActorMeta>>>;
    type Level = ColoredLevel;
    type Location = ColoredLocation;
    type Module = ColoredModule;
    type Payload = ColoredPayload;
    type ResetStyle = ResetStyle;
    type Timestamp = Rfc3339Weak;
    type TraceId = EmptyIfNone<ColoredByHash<TraceId>>;
}
