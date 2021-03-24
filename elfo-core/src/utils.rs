use derive_more::Deref;

#[derive(Debug, Clone, Copy, Default, Hash, PartialEq, Eq, Deref)]
// Spatial prefetcher is now pulling two lines at a time, so we use `align(128)`.
#[cfg_attr(any(target_arch = "x86_64", target_arch = "aarch64"), repr(align(128)))]
#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    repr(align(64))
)]
pub(crate) struct CachePadded<T>(pub(crate) T);
