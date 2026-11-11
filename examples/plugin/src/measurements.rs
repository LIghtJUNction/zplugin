//! Allocation instrumentation for the reproducible benchmark fixture.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

struct MeasuredAllocator;
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);

// This example also serves as the benchmark fixture. Instrumenting both sides
// counts plugin allocations that the host's global allocator cannot observe.
unsafe impl GlobalAlloc for MeasuredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Forward the allocator's original layout to System.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: The caller returns the original pointer and layout.
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;

#[unsafe(no_mangle)]
pub extern "C" fn demo_allocations() -> u64 {
    ALLOCATIONS.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn demo_allocated_bytes() -> u64 {
    ALLOCATED_BYTES.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn demo_live_bytes() -> u64 {
    LIVE_BYTES.load(Ordering::Relaxed)
}
