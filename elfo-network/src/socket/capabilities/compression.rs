// Layouts are specified from highest to lowest bits.

use std::fmt;

use crate::config::Preference;

bitflags::bitflags! {
    /// Set of algorithms. 24 bits.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Algorithms: u32 {
        const LZ4 = 1;
        // NB: Shift by 2: `const ZSTD = 1 << 2;`.
    }
}

/// Compression capabilities.
///
/// Layout:
/// ```text
///       22        2
/// ┌────────────┬─────┐
/// │  Reserved  │ Lz4 │
/// └────────────┴─────┘
/// ```
///
/// Each mentioned algorithm here occupies two bits for a reason, here's the
/// layout of those bits:
/// ```text
///       1           1
/// ┌───────────┬───────────┐
/// │ Preferred │ Supported │
/// └───────────┴───────────┘
/// ```
///
/// 1. Preferred - the compression algorithm is preferred, implies `Supported`.
/// 2. Supported - the compression algorithm is supported.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Compression(u32);

impl Compression {
    pub(crate) const fn empty() -> Self {
        Self::new(Algorithms::empty(), Algorithms::empty())
    }

    pub(super) const fn from_bits_unchecked(v: u32) -> Self {
        Self(v)
    }

    pub(crate) const fn from_bits_truncate(v: u32) -> Self {
        let supported = Algorithms::from_bits_truncate(v);
        let preferred = Algorithms::from_bits_truncate(v >> 1);

        Self::new(supported, preferred)
    }

    pub(crate) const fn new(supported: Algorithms, preferred: Algorithms) -> Self {
        let preferred = preferred.bits();
        // Preferred implies supported.
        let supported = supported.bits() | preferred;

        // 0 1 0 1 | Supported
        // 1 0 1 0 | Preferred
        // -------
        // 1 1 1 1
        let joined = supported | (preferred << 1);

        Self(joined)
    }

    pub(crate) fn toggle(&mut self, algos: Algorithms, pref: Preference) {
        let preferred = self.preferred();
        let supported = self.supported();

        *self = match pref {
            Preference::Preferred => Self::new(supported, preferred | algos),
            Preference::Supported => Self::new(supported | algos, preferred),
            Preference::Disabled => *self,
        };
    }

    pub(crate) fn intersection(self, rhs: Self) -> Self {
        let supported = self.supported() & rhs.supported();
        let both_prefer = self.preferred() & rhs.preferred();
        let some_prefer = (self.preferred() | rhs.preferred()) & supported;

        // If both nodes prefer the same algorithms, use this set.
        // Otherwise, use the set preferred by at least one node.
        let preferred = if !both_prefer.is_empty() {
            both_prefer
        } else {
            some_prefer
        };

        Self::new(supported, preferred)
    }
}

impl Compression {
    pub(crate) const fn bits(self) -> u32 {
        self.0
    }

    pub(crate) const fn supported(self) -> Algorithms {
        // `preferred` bits would be discarded.
        Algorithms::from_bits_truncate(self.0)
    }

    pub(crate) const fn preferred(self) -> Algorithms {
        // `supported` bits would be discarded.
        Algorithms::from_bits_truncate(self.0 >> 1)
    }
}

impl fmt::Display for Compression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut empty = true;

        // Print only preferred algorithms, because they are required to
        // actually enable compression. For resolved final capabilities,
        // only one of the preferred algorithms should be selected.
        // Thus, it will be printed as a single value.
        for (name, _) in self.preferred().iter_names() {
            if !empty {
                f.write_str(", ")?;
            }
            write!(f, "{name}")?;
            empty = false;
        }

        if empty {
            f.write_str("None")?;
        }

        Ok(())
    }
}
