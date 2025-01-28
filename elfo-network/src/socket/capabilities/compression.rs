use std::fmt;

use crate::config::Preference;

bitflags::bitflags! {
    /// Set of algorithms.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Algorithms: u8 {
        const LZ4 = 1;

        #[cfg(test)] // not implemented yet, but useful for tests
        const ZSTD = 1 << 2;
    }
}

/// Compression capabilities.
// ~
// Layout:
// ```text
//    6 bits   2 bits
// ┌──────────┬─────┐
// │ Reserved │ Lz4 │
// └──────────┴─────┘
// ```
//
// Each mentioned algorithm here occupies two bits for a reason, here's the
// layout of those bits:
// ```text
//     1 bit       1 bit
// ┌───────────┬───────────┐
// │ Preferred │ Supported │
// └───────────┴───────────┘
// ```
//
// 1. Preferred — the compression algorithm is preferred, implies `Supported`.
// 2. Supported — the compression algorithm is supported.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) struct Compression(u8);

impl Compression {
    pub(crate) const fn empty() -> Self {
        Self::new(Algorithms::empty(), Algorithms::empty())
    }

    pub(super) const fn from_bits(v: u8) -> Self {
        Self(v)
    }

    pub(crate) const fn into_bits(self) -> u8 {
        self.0
    }

    pub(crate) const fn new(supported: Algorithms, preferred: Algorithms) -> Self {
        let preferred = preferred.bits();
        // Preferred implies supported.
        let supported = supported.bits() | preferred;

        // 0 1 0 1 | Supported
        // 1 0 1 0 | Preferred
        // -------
        // 1 1 1 1
        Self(supported | (preferred << 1))
    }

    pub(crate) fn toggle(&mut self, algos: Algorithms, pref: Preference) {
        let mut preferred = self.preferred() - algos;
        let mut supported = self.supported() - algos;

        match pref {
            Preference::Preferred => preferred |= algos,
            Preference::Supported => supported |= algos,
            Preference::Disabled => {}
        };

        *self = Self::new(supported, preferred);
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

    pub(crate) const fn supported(self) -> Algorithms {
        // `Preferred` bits would be discarded.
        Algorithms::from_bits_truncate(self.0)
    }

    pub(crate) const fn preferred(self) -> Algorithms {
        // `Supported` bits would be discarded.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle() {
        let mut compression = Compression::empty();
        assert!(!compression.preferred().contains(Algorithms::LZ4));
        assert!(!compression.supported().contains(Algorithms::LZ4));
        assert!(!compression.preferred().contains(Algorithms::ZSTD));
        assert!(!compression.supported().contains(Algorithms::ZSTD));

        compression.toggle(Algorithms::ZSTD, Preference::Preferred);
        assert!(compression.preferred().contains(Algorithms::ZSTD));
        assert!(compression.supported().contains(Algorithms::ZSTD));

        for _ in 0..2 {
            compression.toggle(Algorithms::LZ4, Preference::Preferred);
            assert!(compression.preferred().contains(Algorithms::LZ4));
            assert!(compression.supported().contains(Algorithms::LZ4));

            compression.toggle(Algorithms::LZ4, Preference::Supported);
            assert!(!compression.preferred().contains(Algorithms::LZ4));
            assert!(compression.supported().contains(Algorithms::LZ4));

            compression.toggle(Algorithms::LZ4, Preference::Disabled);
            assert!(!compression.preferred().contains(Algorithms::LZ4));
            assert!(!compression.supported().contains(Algorithms::LZ4));
        }

        assert!(compression.preferred().contains(Algorithms::ZSTD));
        assert!(compression.supported().contains(Algorithms::ZSTD));
    }

    #[test]
    fn intersection() {
        let a = Compression::empty();
        let b = Compression::new(Algorithms::LZ4, Algorithms::LZ4);
        assert_eq!(a.intersection(b), Compression::empty());

        // Some algorithm is preferred by both nodes.
        let a = Compression::new(Algorithms::LZ4, Algorithms::ZSTD | Algorithms::LZ4);
        let b = Compression::new(Algorithms::LZ4, Algorithms::ZSTD);
        let s = a.intersection(b);
        assert_eq!(s.preferred(), Algorithms::ZSTD);
        assert_eq!(s.supported(), Algorithms::LZ4 | Algorithms::ZSTD);

        // A preferred algo is not supported by the other node.
        let a = Compression::new(Algorithms::LZ4, Algorithms::ZSTD);
        let b = Compression::new(Algorithms::LZ4, Algorithms::empty());
        let s = a.intersection(b);
        assert_eq!(s.preferred(), Algorithms::empty());
        assert_eq!(s.supported(), Algorithms::LZ4);

        // Some node prefer an algo supported by the other node.
        let a = Compression::new(Algorithms::LZ4, Algorithms::LZ4);
        let b = Compression::new(Algorithms::LZ4, Algorithms::empty());
        let s = a.intersection(b);
        assert_eq!(s.preferred(), Algorithms::LZ4);
        assert_eq!(s.supported(), Algorithms::LZ4);

        // No common supported algorithms.
        let a = Compression::new(Algorithms::LZ4, Algorithms::empty());
        let b = Compression::new(Algorithms::ZSTD, Algorithms::empty());
        let s = a.intersection(b);
        assert_eq!(s.preferred(), Algorithms::empty());
        assert_eq!(s.supported(), Algorithms::empty());
    }
}
