use std::{
    error::Error,
    fmt::{self, Write},
};

use sharded_slab::Pool;
use tracing::field::{Field, Visit};

use crate::Shared;

const MAX_ERROR_SOURCES: u8 = 5;

pub(super) struct Visitor<'a> {
    pool: &'a Pool<String>,
    output: &'a mut String,
    simplify_message: bool,
}

impl<'a> Visitor<'a> {
    pub(super) fn new(shared: &'a Shared, output: &'a mut String, simplify_message: bool) -> Self {
        Self {
            pool: &shared.pool,
            output,
            simplify_message,
        }
    }

    pub(super) fn push(&mut self, str: &str) {
        self.output.push_str(str);
    }
}

impl Visit for Visitor<'_> {
    fn record_i64(&mut self, field: &Field, value: i64) {
        let _ = write!(self.output, "\t{}={}", field.name(), value);
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        let _ = write!(self.output, "\t{}={}", field.name(), value);
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        let _ = write!(self.output, "\t{}={}", field.name(), value);
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        let name = field.name();

        // These fields are handled in the actor.
        if name == "actor_group" || name == "actor_key" {
            return;
        }

        if name == "message" && self.simplify_message {
            self.simplify_message = false;
            self.output.insert_str(0, value);
        } else {
            let _ = write!(self.output, "\t{}={}", name, value);
        }
    }

    fn record_error(&mut self, field: &Field, mut value: &(dyn Error + 'static)) {
        for i in 0..=MAX_ERROR_SOURCES {
            let prev_len = self.output.len();
            let suffix = Repeat(".source", i);

            if write!(self.output, "\t{}{}={}", field.name(), suffix, value).is_err() {
                self.output.truncate(prev_len);
            }

            if let Some(source) = value.source() {
                value = source;
            } else {
                break;
            }
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let prev_len = self.output.len();

        let result = match field.name() {
            "message" if self.simplify_message && self.output.is_empty() => {
                self.simplify_message = false;
                write!(self.output, "{:?}", value)
            }
            "message" if self.simplify_message => {
                self.simplify_message = false;
                let mut result = Ok(());

                if let Some(id) = self.pool.create_with(|tmp| {
                    result = write!(tmp, "{:?}", value);
                    self.output.insert_str(0, &tmp);
                }) {
                    self.pool.clear(id);
                }

                result
            }
            _ => write!(self.output, "\t{}={:?}", field.name(), value),
        };

        if result.is_err() {
            self.output.truncate(prev_len);
        }
    }
}

struct Repeat<'a>(&'a str, u8);

impl fmt::Display for Repeat<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for _ in 0..self.1 {
            f.write_str(self.0)?;
        }

        Ok(())
    }
}
