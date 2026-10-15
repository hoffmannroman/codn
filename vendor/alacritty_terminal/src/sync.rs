//! Synchronization types.
//!
//! Most importantly, a fair mutex is included.

use std::time::{Duration, Instant};

use parking_lot::{Mutex, MutexGuard};

/// A fair mutex.
///
/// Uses an extra lock to ensure that if one thread is waiting that it will get
/// the lock before a single thread can re-lock it.
pub struct FairMutex<T> {
    /// Data.
    data: Mutex<T>,
    /// Next-to-access.
    next: Mutex<()>,
}

impl<T> FairMutex<T> {
    /// Create a new fair mutex.
    pub fn new(data: T) -> FairMutex<T> {
        FairMutex { data: Mutex::new(data), next: Mutex::new(()) }
    }

    /// Acquire a lease to reserve the mutex lock.
    ///
    /// This will prevent others from acquiring a terminal lock, but block if anyone else is
    /// already holding a lease.
    pub fn lease(&self) -> MutexGuard<'_, ()> {
        self.next.lock()
    }

    /// Lock the mutex.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        // Must bind to a temporary or the lock will be freed before going
        // into data.lock().
        let _next = self.next.lock();
        self.data.lock()
    }

    /// Lock the mutex, unless that takes longer than `timeout`.
    pub fn try_lock_for(&self, timeout: Duration) -> Option<MutexGuard<'_, T>> {
        let deadline = Instant::now() + timeout;
        let _next = self.next.try_lock_until(deadline)?;
        self.data.try_lock_until(deadline)
    }

    /// Unfairly lock the mutex.
    pub fn lock_unfair(&self) -> MutexGuard<'_, T> {
        self.data.lock()
    }

    /// Unfairly try to lock the mutex.
    pub fn try_lock_unfair(&self) -> Option<MutexGuard<'_, T>> {
        self.data.try_lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_with_a_limit_gives_up_on_a_lease_and_on_the_data() {
        let mutex = FairMutex::new(0);
        let wait = Duration::from_millis(20);
        *mutex.try_lock_for(wait).unwrap() = 1;
        // The reader holds its lease for a whole chunk of output.
        let lease = mutex.lease();
        let begun = Instant::now();
        assert!(mutex.try_lock_for(wait).is_none());
        assert!(begun.elapsed() >= wait);
        drop(lease);
        // And the data, which it locks without the lease as well.
        let data = mutex.lock_unfair();
        assert!(mutex.try_lock_for(wait).is_none());
        drop(data);
        assert_eq!(*mutex.try_lock_for(wait).unwrap(), 1);
    }
}
