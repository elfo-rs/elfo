use std::fmt;

use serde::{Deserialize, Serialize};

/// Variables for substitution in the path.
#[derive(Debug)]
pub(crate) struct TemplateVariables<'a> {
    pub(crate) class: &'a str,
    pub(crate) ts: u64,
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
        Ok(Self::parse(s))
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
    fn expand_variable(var: Variable, vars: &TemplateVariables<'_>, dest: &mut String) {
        match var {
            Variable::Class => dest.push_str(vars.class),
            Variable::Time => {
                dest.push_str(&vars.ts.to_string());
            }
        }
    }

    pub(crate) fn render_into(&self, variables: TemplateVariables<'_>, to: &mut String) {
        let mut offset = 0;

        for Component { size, data } in self.components.iter().copied() {
            let size = size as usize;
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
    fn parse_variable(var: &str) -> Option<Variable> {
        use Variable as V;

        Some(match var {
            "time" => V::Time,
            "class" => V::Class,
            _ => return None,
        })
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
    fn parse(s: impl Into<String>) -> DumpPath {
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
            let Some(variable) = Self::parse_variable(raw_variable) else {
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

        DumpPath {
            template,
            components,
        }
    }
}

impl fmt::Display for DumpPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.template)
    }
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(PartialEq, Eq))]
enum ComponentData {
    /// Simple path, no templating.
    Path,

    /// Template variable.
    Variable(Variable),
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(PartialEq, Eq))]
enum Variable {
    /// Dump class.
    Class,

    /// Time of the dump.
    Time,
}

#[derive(Debug, Clone, Copy)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_renders_successfully() {
        #[track_caller]
        fn case(template: &str, vars: TemplateVariables<'_>, expected: &str) {
            let path = DumpPath::parse(template);
            let mut actual = String::new();
            path.render_into(vars, &mut actual);

            assert_eq!(actual, expected);
        }

        // Simple class substitution
        case(
            "/tmp/{class}.dump",
            TemplateVariables {
                class: "class",
                ts: 0,
            },
            "/tmp/class.dump",
        );

        // Class and time.
        case(
            "/tmp/dump-{class}-{time}.dump",
            TemplateVariables {
                class: "class",
                ts: 1337100,
            },
            "/tmp/dump-system.network-1337100.dump",
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
            let actual = DumpPath::parse(template);

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
            "/tmp/{class}{time}.dump",
            [
                (5, D::Path),
                (7, D::Variable(Variable::Class)),
                (6, D::Variable(Variable::Time)),
                (5, D::Path),
            ],
        );

        // Invalid template variables are treated just like path.
        case("/tmp/{lol}{kek}.dump", [(20, D::Path)]);

        // Valid template variable precedes invalid.
        case(
            "/tmp/{lol}{time}.dump",
            [
                (10, D::Path),
                (6, D::Variable(Variable::Time)),
                (5, D::Path),
            ],
        );
    }
}
