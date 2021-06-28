use std::{fmt::Write, hash::Hash, marker::PhantomData, sync::Arc, time::SystemTime};

use tracing::Level;

use elfo_core::{_priv::ObjectMeta, trace_id::TraceId};

pub(crate) trait Formatter<T: ?Sized> {
    fn fmt(dest: &mut String, v: &T);
}

// Rfc3339Weak

pub(crate) struct Rfc3339Weak;

impl Formatter<SystemTime> for Rfc3339Weak {
    fn fmt(out: &mut String, v: &SystemTime) {
        let t_idx = out.len() + 10;
        let _ = write!(out, "{}", humantime::format_rfc3339_nanos(*v));
        // Replace "T" with " ".
        out.replace_range(t_idx..t_idx + 1, " ");
        // Remove trailing "Z".
        out.pop();
    }
}

// Level

impl Formatter<Level> for Level {
    fn fmt(out: &mut String, v: &Level) {
        out.push_str(match *v {
            Level::TRACE => "TRACE",
            Level::DEBUG => "DEBUG",
            Level::INFO => " INFO",
            Level::WARN => " WARN",
            Level::ERROR => "ERROR",
        })
    }
}

// ColoredLevel

pub(crate) struct ColoredLevel;

impl Formatter<Level> for ColoredLevel {
    fn fmt(out: &mut String, v: &Level) {
        out.push_str(match *v {
            Level::TRACE => "\x1b[37mTRACE\x1b[0m",
            Level::DEBUG => "DEBUG",
            Level::INFO => " \x1b[32mINFO\x1b[0m",
            Level::WARN => " \x1b[33mWARN\x1b[0m",
            Level::ERROR => "\x1b[31mERROR\x1b[0m",
        })
    }
}

// TraceId

impl Formatter<TraceId> for TraceId {
    fn fmt(out: &mut String, v: &TraceId) {
        let _ = write!(out, "{}", v);
    }
}

// ObjectMeta

impl Formatter<Arc<ObjectMeta>> for Arc<ObjectMeta> {
    fn fmt(out: &mut String, v: &Arc<ObjectMeta>) {
        if let Some(key) = v.key.as_ref() {
            out.push_str(&v.group);
            out.push('.');
            out.push_str(key);
        } else {
            out.push_str(&v.group);
        }
    }
}

// Payload

pub(crate) struct Payload;

impl Formatter<str> for Payload {
    fn fmt(out: &mut String, v: &str) {
        // TODO: escape \t.
        for (idx, chunk) in v.split('\n').enumerate() {
            if idx > 0 {
                out.push_str("\\n");
            }

            out.push_str(chunk);
        }
    }
}

// ColoredPayload

pub(crate) struct ColoredPayload;

impl Formatter<str> for ColoredPayload {
    fn fmt(out: &mut String, v: &str) {
        // TODO: escape \t.
        for (idx, chunk) in v.split('\n').enumerate() {
            if idx > 0 {
                out.push_str("\\n");
            }

            // <message>\t<key>=<value>\t<key>=<value>
            for section in chunk.split('\t') {
                if let Some((key, value)) = section.split_once('=') {
                    out.push_str("\t\x1b[1m");
                    out.push_str(key);
                    out.push_str("\x1b[22m=");
                    out.push_str(value);
                } else {
                    // It's the message section.
                    out.push_str(section);
                }
            }
        }
    }
}

// EmptyIfNone

pub(crate) struct EmptyIfNone<I>(PhantomData<I>);

impl<T, I: Formatter<T>> Formatter<Option<T>> for EmptyIfNone<I> {
    fn fmt(out: &mut String, v: &Option<T>) {
        if let Some(inner) = v {
            I::fmt(out, inner);
        }
    }
}

// ColoredByHash

/// Makes a color based on the fx hash of the value.
/// Generated colors have constant brightness.
pub(crate) struct ColoredByHash<I>(PhantomData<I>);

impl<T: Hash, I: Formatter<T>> Formatter<T> for ColoredByHash<I> {
    #[allow(clippy::many_single_char_names)]
    fn fmt(out: &mut String, v: &T) {
        let hash = fxhash::hash64(v);

        let y = 128f64;
        let cb = (hash % 256) as u8 as f64;
        let cr = (hash / 256 % 256) as u8 as f64;

        let r = clamp(y + 1.402 * (cr - 128.));
        let g = clamp(y - 0.344136 * (cb - 128.) - 0.714136 * (cr - 128.));
        let b = clamp(y + 1.772 * (cb - 128.));

        // ANSI escape sequence to set 24-bit foreground font color.
        let _ = write!(out, "\x1b[38;2;{};{};{}m", r, g, b);
        I::fmt(out, v);
        out.push_str("\x1b[0m");
    }
}

fn clamp(v: f64) -> u8 {
    v.max(0.).min(255.) as u8
}
