use std::fmt;

use serde::{Deserialize, Serialize};

const MAX_FMT_TS_LEN: usize = 64;
const FMT_TS_ARRAY_LEN: usize = MAX_FMT_TS_LEN + 1;

/// Variables for substitution in the path.
#[derive(Debug)]
pub(crate) struct TemplateVariables<'a> {
    pub(crate) class: &'a str,
    pub(crate) ts: i64,
}

/// Template for the dump path.
#[derive(Debug)]
pub struct DumpPath {
    template: String,
    /// Components of the template, contains instructions on
    /// how to render.
    // NOTE: ComponentData::Path cannot precede ComponentData::Path,
    // it's done for efficiency purposes, but it isn't strict invariant.
    components: Vec<Component>,
}

impl<'de> Deserialize<'de> for DumpPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(s).map_err(serde::de::Error::custom)
    }
}

impl Serialize for DumpPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.template.serialize(serializer)
    }
}

impl DumpPath {
    fn expand_variable(var: &Variable, vars: &TemplateVariables<'_>, dest: &mut String) {
        match var {
            Variable::Class => dest.push_str(vars.class),
            Variable::Time { format } => {
                let ts = vars.ts;
                strftime(ts, format, dest);
            }
        }
    }

    pub(crate) fn render_into(&self, variables: TemplateVariables<'_>, to: &mut String) {
        let mut offset = 0;

        for Component { size, data } in self.components.iter() {
            let size = *size as usize;
            match data {
                ComponentData::Variable(var) => {
                    Self::expand_variable(var, &variables, to);
                }
                ComponentData::Path => {
                    to.push_str(&self.template[offset..offset + size]);
                }
            }

            offset += size;
        }
    }
}

impl DumpPath {
    /// Test whether provided strftime format is valid.
    fn test_strftime(format: &cstr::Utf8CString) -> Result<(), String> {
        let mut dest = String::new();
        let success = strftime(100, format, &mut dest);
        if success {
            Ok(())
        } else {
            Err(format!("invalid strftime format: {format:?}"))
        }
    }

    fn parse_variable(var: &str) -> Result<Option<Variable>, String> {
        use Variable as V;

        let var = match var {
            "class" => V::Class,
            _ => {
                if let Some(fmt) = var.strip_prefix("time:") {
                    let format = cstr::Utf8CString::new(fmt);
                    // Test strftime validity to fail early if it is invalid.
                    Self::test_strftime(&format)?;

                    V::Time { format }
                } else {
                    return Ok(None);
                }
            }
        };

        Ok(Some(var))
    }

    fn push_path(of_size: &mut usize, to: &mut Vec<Component>) {
        if *of_size != 0 {
            to.push(Component {
                size: *of_size as u16,
                data: ComponentData::Path,
            });
            *of_size = 0;
        }
    }

    /// Parse template.
    fn parse(s: impl Into<String>) -> Result<Self, String> {
        let template = s.into();
        let mut components = vec![];

        let mut plain_size = 0;
        let mut chunk = template.as_str();

        loop {
            // original = "xxx {yyy} zzz"
            // before = "xxx "
            // after = "yyy} zzz"
            let Some((before, after)) = chunk.split_once('{') else {
                // no more template variables.
                plain_size += chunk.len();
                break;
            };
            plain_size += before.len();

            // raw_variable = "yyy"
            // after = " zzz"
            let Some((raw_variable, after)) = after.split_once('}') else {
                // 1 = {, which is not in "before".
                plain_size += 1;
                chunk = after;
                continue;
            };
            let Some(variable) = Self::parse_variable(raw_variable)? else {
                // "{<variable>}"
                plain_size += 2 + raw_variable.len();
                chunk = after;
                continue;
            };

            Self::push_path(&mut plain_size, &mut components);
            components.push(Component {
                // "{<variable>}"
                size: raw_variable.len() as u16 + 2,
                data: ComponentData::Variable(variable),
            });
            chunk = after;
        }
        Self::push_path(&mut plain_size, &mut components);

        Ok(DumpPath {
            template,
            components,
        })
    }
}

impl fmt::Display for DumpPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.template)
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq, Eq))]
enum ComponentData {
    /// Simple path, no templating.
    Path,

    /// Template variable.
    Variable(Variable),
}

#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq, Eq))]
enum Variable {
    /// Dump class.
    Class,

    /// Time of the dump.
    Time {
        /// strptime string format.
        format: cstr::Utf8CString,
    },
}

#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq, Eq))]
struct Component {
    /// How much this component occupies space in template, in bytes. Each
    /// component contains not the entire span like `(Offset, Size)`, but
    /// only size, since components are arranged according to occurence in
    /// template string, thus offset of each component can be calculated
    /// during rendering, for free, since components are rendered
    /// sequentially in any case.
    size: u16,
    data: ComponentData,
}

/// Convert unix timestamp to [`libc::tm`].
fn ts2tm(ts: i64) -> libc::tm {
    // SAFETY: [`libc::tm`] is `#[repr(C)]` and contains
    // only integral types.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: safe, lol.
    unsafe { libc::localtime_r(std::ptr::addr_of!(ts), std::ptr::addr_of_mut!(tm)) };

    tm
}

fn strftime(ts: i64, format: &cstr::Utf8CString, dest: &mut String) -> bool {
    let tm = ts2tm(ts);
    let mut formatted: [libc::c_char; FMT_TS_ARRAY_LEN] = [0; FMT_TS_ARRAY_LEN];
    // SAFETY: calling function from libc is unsafe, but
    // 1. formatted is valid buffer
    // 2. tm is valid too.
    let len = unsafe {
        libc::strftime(
            std::ptr::addr_of_mut!(formatted).cast::<libc::c_char>(),
            FMT_TS_ARRAY_LEN,
            format.as_ptr(),
            std::ptr::addr_of!(tm),
        )
    };
    if len == 0 {
        return false;
    }

    // SAFETY: comments inside.
    unsafe {
        // 1. mem::transmute wouldn't compile if u8 and libc::c_char differ in size
        // (there are strange circumstances).
        // 2. `libc::c_char` to i8 is i8 -> u8 conversion, thus converting
        // same-sized arrays through transmute is safe (btw transmute checks that
        // condition in compile time)
        let u8_array = std::mem::transmute::<
            [libc::c_char; FMT_TS_ARRAY_LEN],
            [u8; FMT_TS_ARRAY_LEN],
        >(formatted);
        // 3. It's guaranteed that formatted string is in UTF8, since
        // format string is valid UTF8.
        let utf8_str = std::str::from_utf8_unchecked(&u8_array[..len]);
        dest.push_str(utf8_str);
    }

    true
}

// Separate module to ensure encapsulation.
mod cstr {
    use std::{ffi::CString, fmt};

    /// [`CString`] which is guaranteed to be valid utf8.
    #[derive(Clone, PartialEq, Eq)]
    pub(super) struct Utf8CString(CString);

    impl fmt::Debug for Utf8CString {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            <str as fmt::Debug>::fmt(self.as_str(), f)
        }
    }

    impl fmt::Display for Utf8CString {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.as_str())
        }
    }

    impl Utf8CString {
        pub(super) fn as_str(&self) -> &str {
            // SAFETY: provided cstring is valid utf8.
            unsafe { std::str::from_utf8_unchecked(self.0.as_bytes()) }
        }

        pub(super) fn as_ptr(&self) -> *const libc::c_char {
            self.0.as_ptr()
        }

        pub(super) fn new(s: &str) -> Self {
            Self(CString::new(s).expect("nul-byte encountered"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time() -> Variable {
        Variable::Time {
            format: cstr::Utf8CString::new("%d.%m.%Y"),
        }
    }

    #[test]
    fn strftime_wrapper_works() {
        fn case(ts: i64, format: &str, expected: &str) {
            // Silly hack to get rid of timezone-awareness
            // of `libc::strftime`, to make it testable.
            let gmtoff = ts2tm(ts).tm_gmtoff;
            let ts = ts - gmtoff;

            let format = cstr::Utf8CString::new(format);
            let mut dest = String::new();

            assert!(strftime(ts, &format, &mut dest));
            assert_eq!(dest, expected);
        }

        // %s here is not covered, since
        // it would be shifted according to tm_gmtoff.

        // 0 seconds
        case(0, "%S", "00");

        // 1 Minute
        case(60, "%M", "01");

        // 1 Hour
        case(60 * 60, "%H:%M", "01:00");

        // 1 Hour and 10 Minutes
        case(60 * 60 + 10 * 60, "%H:%M", "01:10")
    }

    #[test]
    fn it_renders_correctly() {
        #[track_caller]
        fn case(template: &str, vars: TemplateVariables<'_>, expected: &str) {
            let path = DumpPath::parse(template).unwrap();
            let mut actual = String::new();
            path.render_into(vars, &mut actual);

            assert_eq!(actual, expected);
        }

        // NB: see strftime test above.
        fn ts(time: i64) -> i64 {
            let gmtoff = ts2tm(time).tm_gmtoff;
            time - gmtoff
        }

        // Simple class substitution
        case(
            "/tmp/{class}.dump",
            TemplateVariables {
                class: "class",
                ts: ts(0),
            },
            "/tmp/class.dump",
        );

        // Class and time.
        case(
            "/tmp/dump-{class}-{time:%H:%M}.dump",
            TemplateVariables {
                class: "class",
                // 1 hour 10 minutes
                ts: ts(60 * 60 + 60 * 10),
            },
            "/tmp/dump-class-01:10.dump",
        );
    }

    #[test]
    fn it_parses_correctly() {
        #[track_caller]
        fn case<I>(template: &'static str, expected: I)
        where
            I: IntoIterator<Item = (u16, ComponentData)>,
        {
            let expected: Vec<Component> = expected
                .into_iter()
                .map(|(size, data)| Component { size, data })
                .collect();
            let actual = DumpPath::parse(template).unwrap();

            assert_eq!(expected, actual.components);
        }

        use ComponentData as D;

        // Path without template variables.
        case("/tmp/dump.dump", [(14, D::Path)]);

        // Dump with class.
        case(
            "/tmp/{class}.dump",
            [
                (5, D::Path),
                (7, D::Variable(Variable::Class)),
                (5, D::Path),
            ],
        );

        // {time} precedes {class}.
        case(
            "/tmp/{class}{time:%d.%m.%Y}.dump",
            [
                (5, D::Path),
                (7, D::Variable(Variable::Class)),
                (15, D::Variable(time())),
                (5, D::Path),
            ],
        );

        // Invalid template variables are treated just like path.
        case("/tmp/{lol}{kek}.dump", [(20, D::Path)]);

        // Valid template variable precedes invalid.
        case(
            "/tmp/{lol}{time:%d.%m.%Y}.dump",
            [(10, D::Path), (15, D::Variable(time())), (5, D::Path)],
        );
    }
}
