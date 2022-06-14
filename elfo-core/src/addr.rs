use std::fmt;

// TODO: improve `Debug` and `Display` instances.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Addr(usize);

impl fmt::Display for Addr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: print a version.
        write!(f, "{}v0", self.0)
    }
}

impl Addr {
    pub const NULL: Addr = Addr(usize::MAX);

    pub fn from_bits(bits: usize) -> Self {
        Addr(bits)
    }

    pub fn into_bits(self) -> usize {
        self.0
    }
}
