// Layouts are specified from highest to lowest bits.

use crate::config::Preference;

/// Layout:
/// ```text
///     Bits
///   6    2
/// +---+-----+
/// | R | Lz4 |
/// +---+-----+
/// ```
///
/// `R` - reserved, any other mean specific compression algorithm. Layout
/// for specific compression algorithm:
/// ```text
///    Bits
///   1   1
/// +---+---+
/// | S | P |
/// +---+---+
/// ```
///
/// 1. `S` - the compression algorithm is supported.
/// 2. `P` - the compression algorithm is preferred, implies `S`.
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
        let supported = Algorithms::from_bits_truncate(v >> 1);
        let preferred = Algorithms::from_bits_truncate(v);

        Self::new(supported, preferred)
    }

    pub(crate) const fn new(supported: Algorithms, preferred: Algorithms) -> Self {
        let preferred = preferred.bits();
        // Preferred implies supported.
        let supported = supported.bits() | preferred;

        // 0 1 0 1 | Preferred
        // 1 0 1 0 | Supported
        // -------
        // 1 1 1 1
        let joined = (supported << 1) | preferred;

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

    pub(crate) const fn intersection(self, rhs: Self) -> Self {
        let we_prefer = self.preferred();
        let we_support = self.supported();

        let they_prefer = rhs.preferred();
        let they_support = rhs.supported();

        // Let's see what we both support.
        let both_support = we_support.intersection(they_support);
        // And if we both prefer something.
        let both_prefer = we_prefer.intersection(they_prefer);

        let preferred = if both_prefer.is_empty() {
            // if we prefer something that is supported by us and
            // the remote node, then it's a deal.
            we_prefer.intersection(both_support)
        } else {
            // We both prefer something!
            both_prefer
        };

        Self::new(both_support, preferred)
    }
}

impl Compression {
    pub(crate) const fn bits(self) -> u32 {
        self.0
    }

    pub(crate) const fn supported(self) -> Algorithms {
        // `preferred` bits would be discarded.
        Algorithms::from_bits_truncate(self.0 >> 1)
    }

    pub(crate) const fn preferred(self) -> Algorithms {
        // `supported` bits would be discarded.
        Algorithms::from_bits_truncate(self.0)
    }
}

bitflags::bitflags! {
    // Actually, 24 bits.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Algorithms: u32 {
        const LZ4 = 1;
        // NB: Shift by 2: `const ZSTD = 1 << 2;`.
    }
}
