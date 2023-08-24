use std::alloc::{GlobalAlloc, Layout};

use elfo_core::scope::LinkedBytesTrack;

/// Global allocator providing metrics on allocated memory
///
/// ```
/// # use elfo_telemeter::AllocatorStats;
/// #[global_allocator]
/// static ALLOCATOR: AllocatorStats<std::alloc::System> = AllocatorStats::new(std::alloc::System);
/// ```
///
/// Setting this as the global allocator provides two counters:
/// `elfo_allocated_bytes_total` and `elfo_deallocated_bytes_total`, tracking
/// total allocated and deallocated memory in bytes.
///
/// TODO: `elfo_linked_bytes_total`
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

fn ext_layout(origin: Layout) -> (Layout, usize) {
    Layout::new::<usize>().extend(origin).unwrap() // TODO
}

unsafe impl<A> GlobalAlloc for AllocatorStats<A>
where
    A: GlobalAlloc,
{
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let (layout, offset) = ext_layout(layout);
        let ptr = self.inner.alloc(layout);
        if !ptr.is_null() {
            let linked_track = elfo_core::scope::try_with(|scope| {
                scope.increment_allocated_bytes(layout.size());
                scope.linked_bytes_track(layout.size())
            });

            ptr.cast::<usize>()
                .write(linked_track.map_or(0, LinkedBytesTrack::into_raw));
        }
        ptr.add(offset)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let (layout, offset) = ext_layout(layout);
        let ptr = ptr.sub(offset);

        let p = ptr.cast::<usize>().read();
        if p > 0 {
            LinkedBytesTrack::from_raw(p).destroy(layout.size());
        }

        self.inner.dealloc(ptr, layout);
        elfo_core::scope::try_with(|scope| {
            scope.increment_deallocated_bytes(layout.size());
        });
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let (layout, offset) = ext_layout(layout);
        let ptr = self.inner.alloc_zeroed(layout);
        if !ptr.is_null() {
            let linked_track = elfo_core::scope::try_with(|scope| {
                scope.increment_allocated_bytes(layout.size());
                scope.linked_bytes_track(layout.size())
            });

            ptr.cast::<usize>()
                .write(linked_track.map_or(0, LinkedBytesTrack::into_raw));
        }
        ptr.add(offset)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, mut new_size: usize) -> *mut u8 {
        let (layout, offset) = ext_layout(layout);
        new_size += offset;
        let ptr = ptr.sub(offset);

        let p = ptr.cast::<usize>().read();
        if p > 0 {
            LinkedBytesTrack::from_raw(p).destroy(layout.size());
        }

        let ptr = self.inner.realloc(ptr, layout, new_size);
        if !ptr.is_null() {
            let linked_track = elfo_core::scope::try_with(|scope| {
                scope.increment_deallocated_bytes(layout.size());
                scope.increment_allocated_bytes(new_size);
                scope.linked_bytes_track(new_size)
            });

            ptr.cast::<usize>()
                .write(linked_track.map_or(0, LinkedBytesTrack::into_raw));
        }
        ptr.add(offset)
    }
}
