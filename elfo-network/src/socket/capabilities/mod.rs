use self::compression::Compression;

pub(crate) mod compression;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Capabilities(u32);

impl Capabilities {
    pub(crate) const fn new(compression: Compression) -> Self {
        let compression = compression.bits() as u32;
        let joined = compression << 8;

        Self(joined)
    }

    pub(crate) const fn from_bits_truncate(bits: u32) -> Self {
        let compression = Compression::from_bits_truncate(bits >> 8);
        Self::new(compression)
    }

    pub(crate) const fn intersection(self, rhs: Self) -> Self {
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
