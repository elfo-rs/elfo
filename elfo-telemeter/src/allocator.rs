use std::alloc::{GlobalAlloc, Layout};

use metrics::counter;

const ALLOCATED: &str = "elfo_allocated_memory";
const DEALLOCATED: &str = "elfo_deallocated_memory";

/// Global allocator providing metrics on allocated memory
///
/// ```
/// # use elfo_telemeter::AllocatorStats;
/// #[global_allocator]
/// static ALLOCATOR: AllocatorStats<std::alloc::System> = AllocatorStats::new(std::alloc::System);
/// ```
///
/// Setting this as the global allocator provides two counters:
/// `elfo_allocated_memory` and `elfo_deallocated_memory`, tracking total
/// allocated and deallocated memory in bytes.
#[stability::unstable]
pub struct AllocatorStats<A> {
    inner: A,
}

impl<A> AllocatorStats<A> {
    /// Wrap a global allocator, instrumenting it with metrics
    pub const fn new(inner: A) -> Self {
        Self { inner }
    }
}

unsafe impl<A> GlobalAlloc for AllocatorStats<A>
where
    A: GlobalAlloc,
{
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = self.inner.alloc(layout);
        if !ptr.is_null() {
            counter!(ALLOCATED, layout.size() as u64);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.inner.dealloc(ptr, layout);
        counter!(DEALLOCATED, layout.size() as u64);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = self.inner.alloc_zeroed(layout);
        if !ptr.is_null() {
            counter!(ALLOCATED, layout.size() as u64);
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let ptr = self.inner.realloc(ptr, layout, new_size);
        if !ptr.is_null() {
            counter!(DEALLOCATED, layout.size() as u64);
            counter!(ALLOCATED, new_size as u64);
        }
        ptr
    }
}
