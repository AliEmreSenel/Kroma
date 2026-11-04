//! Return unused CPU heap pages after media initialization and retirement.

use std::sync::atomic::{AtomicBool, Ordering};

static RECLAIM_REQUESTED: AtomicBool = AtomicBool::new(true);

/// Schedule reclamation after temporary or retired media buffers are freed.
pub(crate) fn request_reclaim() {
    RECLAIM_REQUESTED.store(true, Ordering::Relaxed);
}

/// Coalesce requests on the render thread, outside the per-frame decode path.
pub(crate) fn reclaim_if_requested() {
    if !RECLAIM_REQUESTED.swap(false, Ordering::Relaxed) {
        return;
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: glibc's thread-safe allocator API releases only unused pages.
        // Live Rust, FFmpeg and driver allocations remain owned and valid.
        unsafe { malloc_trim(0) };
    }
}
