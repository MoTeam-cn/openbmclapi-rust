//! A byte-denominated gate on how much download data is buffered at once.
//!
//! The sync pass already caps how many files are in flight, but not how large
//! they are, and every download is buffered whole so its checksum can be
//! verified. That product is what runs the process out of memory when a mirror
//! carries multi-hundred-megabyte objects, so the pass also holds a byte budget.

use tokio::sync::{Semaphore, SemaphorePermit};

/// Unit the budget is handed out in.
pub const GRANULARITY: u64 = 1024 * 1024;

/// Limits the total size of the downloads buffered concurrently.
pub struct ByteBudget {
    semaphore: Semaphore,
    permits: u32,
}

impl ByteBudget {
    /// Build a budget of `total_bytes`, rounded up to whole units.
    pub fn new(total_bytes: u64) -> Self {
        let permits = u32::try_from(total_bytes.div_ceil(GRANULARITY).max(1)).unwrap_or(u32::MAX);
        ByteBudget {
            semaphore: Semaphore::new(permits as usize),
            permits,
        }
    }

    /// The budget in bytes, after rounding up to whole units.
    pub fn total_bytes(&self) -> u64 {
        self.permits as u64 * GRANULARITY
    }

    /// Permits a download of `bytes` may hold.
    ///
    /// An object larger than the whole budget takes the whole budget: it has to
    /// be buffered either way, and asking for more would wait forever.
    pub fn permits_for(&self, bytes: u64) -> u32 {
        let wanted = bytes.div_ceil(GRANULARITY).max(1);
        wanted.min(self.permits as u64) as u32
    }

    /// Wait until `bytes` fit, then hold the reservation until the guard drops.
    pub async fn acquire(&self, bytes: u64) -> SemaphorePermit<'_> {
        self.semaphore
            .acquire_many(self.permits_for(bytes))
            .await
            .expect("the budget semaphore is never closed")
    }
}

#[cfg(test)]
#[path = "budget_test.rs"]
mod tests;
