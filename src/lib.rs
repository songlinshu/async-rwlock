//! An async reader-writer lock.
//!
//! The locking stragegy is fair: neither readers nor writers will be starved, assuming the task
//! executor is also fair.
//!
//! # Examples
//!
//! ```
//! # futures_lite::future::block_on(async {
//! use async_rwlock::RwLock;
//!
//! let lock = RwLock::new(5);
//!
//! // Multiple read locks can be held at a time.
//! let r1 = lock.read().await;
//! let r2 = lock.read().await;
//! assert_eq!(*r1, 5);
//! assert_eq!(*r2, 5);
//! drop((r1, r2));
//!
//! // Only one write lock can be held at a time.
//! let mut w = lock.write().await;
//! *w += 1;
//! assert_eq!(*w, 6);
//! # })
//! ```

#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]

use std::cell::UnsafeCell;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_mutex::{Mutex, MutexGuard};
use event_listener::Event;

const WRITER_BIT: usize = 1;
const ONE_READER: usize = 2;

/// An async reader-writer lock.
///
/// This type of lock allows multiple readers or one writer at any point in time.
///
/// # Examples
///
/// ```
/// # futures_lite::future::block_on(async {
/// use async_rwlock::RwLock;
///
/// let lock = RwLock::new(5);
///
/// // Multiple read locks can be held at a time.
/// let r1 = lock.read().await;
/// let r2 = lock.read().await;
/// assert_eq!(*r1, 5);
/// assert_eq!(*r2, 5);
/// drop((r1, r2));
///
/// // Only one write locks can be held at a time.
/// let mut w = lock.write().await;
/// *w += 1;
/// assert_eq!(*w, 6);
/// # })
/// ```
pub struct RwLock<T> {
    /// Acquired by the writer.
    mutex: Mutex<()>,

    /// Event triggered when the last reader is dropped.
    no_readers: Event,

    /// Event triggered when the writer is dropped.
    no_writer: Event,

    /// Current state of the lock.
    ///
    /// The least significant bit (`WRITER_BIT`) is set to 1 when a writer is holding the lock or
    /// trying to acquire it.
    ///
    /// The upper bits contain the number of currently active readers. Each active reader
    /// increments the state by `ONE_READER`.
    state: AtomicUsize,

    /// The inner value.
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    /// Creates a new reader-writer lock.
    ///
    /// # Examples
    ///
    /// ```
    /// use async_rwlock::RwLock;
    ///
    /// let lock = RwLock::new(0);
    /// ```
    pub fn new(t: T) -> RwLock<T> {
        RwLock {
            mutex: Mutex::new(()),
            no_readers: Event::new(),
            no_writer: Event::new(),
            state: AtomicUsize::new(0),
            value: UnsafeCell::new(t),
        }
    }

    /// Attempts to acquire a read lock.
    ///
    /// If a read lock could not be acquired at this time, then [`None`] is returned. Otherwise, a
    /// guard is returned that releases the lock when dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// # futures_lite::future::block_on(async {
    /// use async_rwlock::RwLock;
    ///
    /// let lock = RwLock::new(1);
    ///
    /// let reader = lock.read().await;
    /// assert_eq!(*reader, 1);
    ///
    /// assert!(lock.try_read().is_some());
    /// # })
    /// ```
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        let mut state = self.state.load(Ordering::Acquire);

        loop {
            // If there's a writer holding the lock or attempting to acquire it, we cannot acquire
            // a read lock here.
            if state & WRITER_BIT != 0 {
                return None;
            }

            // Make sure the number of readers doesn't overflow.
            if state > std::isize::MAX as usize {
                process::abort();
            }

            // Increment the number of readers.
            match self.state.compare_exchange(
                state,
                state + ONE_READER,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(RwLockReadGuard(self)),
                Err(s) => state = s,
            }
        }
    }

    /// Acquires a read lock.
    ///
    /// Returns a guard that releases the lock when dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// # futures_lite::future::block_on(async {
    /// use async_rwlock::RwLock;
    ///
    /// let lock = RwLock::new(1);
    ///
    /// let reader = lock.read().await;
    /// assert_eq!(*reader, 1);
    ///
    /// assert!(lock.try_read().is_some());
    /// # })
    /// ```
    pub async fn read(&self) -> RwLockReadGuard<'_, T> {
        let mut state = self.state.load(Ordering::Acquire);

        loop {
            if state & WRITER_BIT == 0 {
                // Make sure the number of readers doesn't overflow.
                if state > std::isize::MAX as usize {
                    process::abort();
                }

                // If nobody is holding a write lock or attempting to acquire it, increment the
                // number of readers.
                match self.state.compare_exchange(
                    state,
                    state + ONE_READER,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return RwLockReadGuard(self),
                    Err(s) => state = s,
                }
            } else {
                // Start listening for "no writer" events.
                let listener = self.no_writer.listen();

                // Check again if there's a writer.
                if self.state.load(Ordering::SeqCst) & WRITER_BIT != 0 {
                    // Wait until the writer is dropped.
                    listener.await;
                    // Notify the next reader waiting in line.
                    self.no_writer.notify(1);
                }

                // Reload the state.
                state = self.state.load(Ordering::Acquire);
            }
        }
    }

    /// Attempts to acquire a write lock.
    ///
    /// If a write lock could not be acquired at this time, then [`None`] is returned. Otherwise, a
    /// guard is returned that releases the lock when dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// # futures_lite::future::block_on(async {
    /// use async_rwlock::RwLock;
    ///
    /// let lock = RwLock::new(1);
    ///
    /// assert!(lock.try_write().is_some());
    /// let reader = lock.read().await;
    /// assert!(lock.try_write().is_none());
    /// # })
    /// ```
    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        // First try grabbing the mutex.
        let lock = self.mutex.try_lock()?;

        // If there are no readers, grab the write lock.
        if self.state.compare_and_swap(0, WRITER_BIT, Ordering::AcqRel) == 0 {
            Some(RwLockWriteGuard(self, lock))
        } else {
            None
        }
    }

    /// Acquires a write lock.
    ///
    /// Returns a guard that releases the lock when dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// # futures_lite::future::block_on(async {
    /// use async_rwlock::RwLock;
    ///
    /// let lock = RwLock::new(1);
    ///
    /// let writer = lock.write().await;
    /// assert!(lock.try_read().is_none());
    /// # })
    /// ```
    pub async fn write(&self) -> RwLockWriteGuard<'_, T> {
        // First grab the mutex.
        let lock = self.mutex.lock().await;

        // Set `WRITER_BIT` and create a guard that unsets it in case this future is canceled.
        self.state.fetch_or(WRITER_BIT, Ordering::SeqCst);
        let guard = RwLockWriteGuard(self, lock);

        // If there are readers, we need to wait for them to finish.
        while self.state.load(Ordering::SeqCst) != WRITER_BIT {
            // Start listening for "no readers" events.
            let listener = self.no_readers.listen();

            // Check again if there are readers.
            if self.state.load(Ordering::Acquire) != WRITER_BIT {
                // Wait for the readers to finish.
                listener.await;
            }
        }

        guard
    }

    /// Returns a mutable reference to the inner value.
    ///
    /// Since this call borrows the lock mutably, no actual locking takes place. The mutable borrow
    /// statically guarantees no locks exist.
    ///
    /// # Examples
    ///
    /// ```
    /// # futures_lite::future::block_on(async {
    /// use async_rwlock::RwLock;
    ///
    /// let mut lock = RwLock::new(1);
    ///
    /// *lock.get_mut() = 2;
    /// assert_eq!(*lock.read().await, 2);
    /// # })
    /// ```
    pub fn get_mut(&mut self) -> &mut T {
        unsafe { &mut *self.value.get() }
    }

    /// Unwraps the lock and returns the inner value.
    ///
    /// # Examples
    ///
    /// ```
    /// use async_rwlock::RwLock;
    ///
    /// let lock = RwLock::new(5);
    /// assert_eq!(lock.into_inner(), 5);
    /// ```
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Locked;
        impl fmt::Debug for Locked {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("<locked>")
            }
        }

        match self.try_read() {
            None => f.debug_struct("RwLock").field("value", &Locked).finish(),
            Some(guard) => f.debug_struct("RwLock").field("value", &&*guard).finish(),
        }
    }
}

impl<T> From<T> for RwLock<T> {
    fn from(val: T) -> RwLock<T> {
        RwLock::new(val)
    }
}

impl<T: Default> Default for RwLock<T> {
    fn default() -> RwLock<T> {
        RwLock::new(Default::default())
    }
}

/// A guard that releases the read lock when dropped.
pub struct RwLockReadGuard<'a, T>(&'a RwLock<T>);

unsafe impl<T: Sync> Send for RwLockReadGuard<'_, T> {}
unsafe impl<T: Sync> Sync for RwLockReadGuard<'_, T> {}

impl<T> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        // Decrement the number of readers.
        if self.0.state.fetch_sub(ONE_READER, Ordering::SeqCst) & !WRITER_BIT == ONE_READER {
            // If this was the last reader, trigger the "no readers" event.
            self.0.no_readers.notify(1);
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for RwLockReadGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: fmt::Display> fmt::Display for RwLockReadGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.0.value.get() }
    }
}

/// A guard that releases the write lock when dropped.
pub struct RwLockWriteGuard<'a, T>(&'a RwLock<T>, MutexGuard<'a, ()>);

unsafe impl<T: Send> Send for RwLockWriteGuard<'_, T> {}
unsafe impl<T: Sync> Sync for RwLockWriteGuard<'_, T> {}

impl<T> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        // Unset `WRITER_BIT`.
        self.0.state.fetch_and(!WRITER_BIT, Ordering::SeqCst);
        // If this was the last reader, trigger the "no writer" event.
        self.0.no_writer.notify(1);
    }
}

impl<T: fmt::Debug> fmt::Debug for RwLockWriteGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: fmt::Display> fmt::Display for RwLockWriteGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.0.value.get() }
    }
}

impl<T> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.0.value.get() }
    }
}
