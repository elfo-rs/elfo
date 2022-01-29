use std::cell::Cell;

use serde::{Serialize, Serializer};

// TODO: move to `scope::serde_mode`
thread_local! {
    static IN_DUMPING: Cell<bool> = Cell::new(false);
}

pub(crate) fn set_in_dumping(flag: bool) {
    IN_DUMPING.with(|cell| cell.set(flag));
}

/// Dumps the field as `<hidden>`.
pub fn hide<T: Serialize, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
    if IN_DUMPING.with(Cell::get) {
        serializer.serialize_str("<hidden>")
    } else {
        value.serialize(serializer)
    }
}

#[test]
fn it_works() {
    #[derive(Serialize)]
    struct S {
        #[serde(serialize_with = "crate::dumping::hide")]
        f: u32,
    }

    let value = S { f: 42 };

    assert_eq!(serde_json::to_string(&value).unwrap(), r#"{"f":42}"#);

    set_in_dumping(true);
    assert_eq!(
        serde_json::to_string(&value).unwrap(),
        r#"{"f":"<hidden>"}"#
    );
    set_in_dumping(false);

    assert_eq!(serde_json::to_string(&value).unwrap(), r#"{"f":42}"#);
}
