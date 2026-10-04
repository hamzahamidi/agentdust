use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

pub struct Meter;

unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds the `GlobalAlloc` contract, which `System` shares.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(live, Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        // SAFETY: `ptr` and `layout` come from a matching `alloc` on `System`, as the caller guarantees.
        unsafe { System.dealloc(ptr, layout) }
    }
}

pub fn measure<T>(work: impl FnOnce() -> T) -> (T, usize) {
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    let value = work();
    (value, PEAK.load(Relaxed).saturating_sub(base))
}
