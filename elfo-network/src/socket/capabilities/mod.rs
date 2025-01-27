use std::fmt;

use self::compression::Compression;

pub(crate) mod compression;

/// Things supported by the node.
///
/// ### Layout
///
/// ```text
///         24 bits              8 bits
/// ┌─────────────────────┬──────────────────┐
/// │     Compression     │     Reserved     │
/// └─────────────────────┴──────────────────┘
/// ```
///
/// 1. [`Compression`] - compression capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Capabilities(u32);

impl Capabilities {
    pub(crate) const fn new(compression: Compression) -> Self {
        let compression = compression.bits();
        let joined = compression << 8;

        Self(joined)
    }

    pub(crate) const fn from_bits_truncate(bits: u32) -> Self {
        let compression = Compression::from_bits_truncate(bits >> 8);
        Self::new(compression)
    }

    pub(crate) fn intersection(self, rhs: Self) -> Self {
        let compr = self.compression().intersection(rhs.compression());
        Self::new(compr)
    }
}

impl Capabilities {
    pub(crate) const fn compression(self) -> Compression {
        Compression::from_bits_unchecked(self.0 >> 8)
    }

    pub(crate) const fn bits(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Capabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "caps(compression={})", self.compression())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use self::compression::Algorithms;
    use super::*;

    #[test]
    fn format_is_compatible_with_020alpha17() {
        let caps = Capabilities::new(Compression::new(Algorithms::LZ4, Algorithms::empty()));
        let lz4_bit = caps.bits() & (1 << 8);

        assert_eq!(lz4_bit, 1 << 8);
    }

    #[test]
    fn compression_encoded_right_way() {
        #[track_caller]
        fn case(create: (Algorithms, Algorithms), expect: (Algorithms, Algorithms)) {
            let caps = Capabilities::new(Compression::new(create.0, create.1));
            let compr = caps.compression();

            assert_eq!(compr.supported(), expect.0);
            assert_eq!(compr.preferred(), expect.1);

            // Just in case we should decode same caps.

            let bits = caps.bits();
            let same_caps = Capabilities::from_bits_truncate(bits);

            assert_eq!(caps, same_caps);
        }

        // Supported does not implies preferred.
        case(
            (Algorithms::LZ4, Algorithms::empty()),
            (Algorithms::LZ4, Algorithms::empty()),
        );

        // Preferred implies supported.
        case(
            (Algorithms::empty(), Algorithms::LZ4),
            (Algorithms::LZ4, Algorithms::LZ4),
        );

        // Nothing ever happens.
        case(
            (Algorithms::empty(), Algorithms::empty()),
            (Algorithms::empty(), Algorithms::empty()),
        );
    }

    proptest! {
        #[test]
        fn intersection_is_commutative(lhs in prop::num::u32::ANY, rhs in prop::num::u32::ANY) {
            let lhs = Capabilities::from_bits_truncate(lhs);
            let rhs = Capabilities::from_bits_truncate(rhs);
            prop_assert_eq!(lhs.intersection(rhs), rhs.intersection(lhs));
        }
    }
}
