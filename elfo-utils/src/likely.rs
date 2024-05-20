//! `likely()` and `unlikely()` hints.
//! `core::intrinsics::{likely, unlikely}` are unstable fo now.
//! On stable we can use `#[cold]` to get the same effect.

#[inline]
#[cold]
fn cold() {}

/// Hints the compiler that this branch is likely to be taken.
#[inline(always)]
pub fn likely(b: bool) -> bool {
    if !b {
        cold();
    }
    b
}

/// Hints the compiler that this branch is unlikely to be taken.
#[inline(always)]
pub fn unlikely(b: bool) -> bool {
    if b {
        cold();
    }
    b
}
