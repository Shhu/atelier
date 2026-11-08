//! The allocator: memory freed goes back to the system.
//!
//! On Linux (glibc), freed blocks under the mmap threshold stay in the
//! process's arenas, and that threshold grows with the first big buffer
//! freed, up to 32 MiB: layers, masks and reductions of a few megapixels
//! would then never leave once freed. Windows' and macOS' allocators give
//! large blocks back by themselves.

/// Above this size, a block is mapped on its own and unmapped once freed.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const MMAP_THRESHOLD: i32 = 4 << 20;

/// At start, before the first big buffer: a fixed threshold (it also
/// turns off the growing one). Whether the allocator took it.
pub fn tune() -> bool {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: mallopt only changes the allocator's settings.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, MMAP_THRESHOLD) == 1
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    true
}

/// Much was just freed (a document closed): the arenas' free pages go
/// back to the system, on a worker thread (it walks the whole heap).
pub fn release_later() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    std::thread::spawn(|| {
        // SAFETY: malloc_trim only returns free pages.
        unsafe {
            libc::malloc_trim(0);
        }
    });
}

#[cfg(test)]
mod tests {
    /// glibc takes the fixed threshold (RSS itself can't be measured
    /// reliably here: the other tests allocate at the same time).
    #[test]
    fn the_allocator_takes_the_threshold() {
        assert!(super::tune());
    }
}
