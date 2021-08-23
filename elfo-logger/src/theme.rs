use std::{sync::Arc, time::SystemTime};

use tracing::Level;

use elfo_core::{_priv::ObjectMeta, trace_id::TraceId};

use crate::formatters::*;

pub(crate) trait Theme {
    type Timestamp: Formatter<SystemTime>;
    type Level: Formatter<Level>;
    type TraceId: Formatter<Option<TraceId>>;
    type ObjectMeta: Formatter<Option<Arc<ObjectMeta>>>;
    type Payload: Formatter<str>;
    type Location: Formatter<(&'static str, u32)>;
    type Module: Formatter<str>;
}

pub(crate) struct PlainTheme;

impl Theme for PlainTheme {
    type Level = Level;
    type Location = Location;
    type Module = Module;
    type ObjectMeta = EmptyIfNone<Arc<ObjectMeta>>;
    type Payload = Payload;
    type Timestamp = Rfc3339Weak;
    type TraceId = EmptyIfNone<TraceId>;
}

pub(crate) struct ColoredTheme;

impl Theme for ColoredTheme {
    type Level = ColoredLevel;
    type Location = ColoredLocation;
    type Module = ColoredModule;
    type ObjectMeta = EmptyIfNone<ColoredByHash<Arc<ObjectMeta>>>;
    type Payload = ColoredPayload;
    type Timestamp = Rfc3339Weak;
    type TraceId = EmptyIfNone<ColoredByHash<TraceId>>;
}
