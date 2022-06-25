extern crate alloc;

use core::{mem::forget, ops::Deref};

use alloc::boxed::Box;

pub struct SpinLock<const N: usize>;
impl<const N: usize> SpinLock<N> {
    // Safety: Multiple SpinLocks with the same N are safe,
    // albeit inefficient.
    pub const fn new() -> Self {
        if N > 31 {
            panic!("N must be <= 31");
        }
        SpinLock
    }

    pub fn lock(&self) {
        let sio = unsafe { &*rp2040_pac::SIO::ptr() };
        let spinlock = sio.spinlock[N];
        while spinlock.read().bits() == 0 {
            cortex_m::asm::nop(); // spinloop wheeeee
        }
    }

    pub unsafe fn unlock(&self) {
        let sio = unsafe { &*rp2040_pac::SIO::ptr() };
        let spinlock = sio.spinlock[N];
        spinlock.write(|w| unsafe { w.bits(0xDEADBEEF) }); // Anything will do, but 0xDEADBEEF is cool.
    }
}

pub struct Mutex<T, const N: usize> {
    lock: SpinLock<N>,
    data: T,
}

struct MutexGuard<'a, T, const N: usize> {
    lock: &'a SpinLock<N>,
    data: &'a T,
}

impl<T, const N: usize> Mutex<T, N> {
    pub const fn new(data: T) -> Self {
        Mutex {
            lock: SpinLock::new(),
            data,
        }
    }
    pub fn lock(&self) -> MutexGuard<T, N> {
        self.lock.lock();
        MutexGuard {
            lock: &self.lock,
            data: &self.data,
        }
    }
}

impl<'a, T, const N: usize> Drop for MutexGuard<'a, T, N> {
    fn drop(&mut self) {
        // Safety: We're holding the lock, so we're allowed to unlock it.
        unsafe { self.lock.unlock() };
    }
}

impl<'a, T, const N: usize> Deref for MutexGuard<'a, T, N> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

struct ArcInner<T, const N: usize> {
    data: T,
    ref_count: Mutex<usize, N>,
}

pub struct Arc<T, const N: usize> {
    inner: *const ArcInner<T, N>,
}

impl<T, const N: usize> Arc<T, N> {
    pub const fn new(data: T) -> Self {
        Arc {
            inner: Box::leak(Box::new(ArcInner {
                data,
                ref_count: Mutex::new(1),
            })),
        }
    }
    pub fn to_raw(self) -> *const () {
        let ret = self.inner as *const ();
        forget(self); // Do NOT decrement the refcount; from_raw will not increment it.
        ret
    }
    // Safety: The caller must ensure that the pointer is valid.
    // Making 2 or more Arc<T>s from the same pointer is unsafe because
    // the ref_count is not modified when this function is called.
    pub unsafe fn from_raw(ptr: *const ()) -> Self {
        Arc {
            inner: ptr as *const ArcInner<T, N>,
        }
    }
}

impl<T, const N: usize> Clone for Arc<T, N> {
    fn clone(&self) -> Self {
        // Safety: self.inner can never be invalid, because it's only destroyed when the ref_count is 0.
        // And we're only ever called when the ref_count is > 0 because we have a reference to self.
        // A reference to self means a ref_count > 0 because each clone increments the ref_count
        // and each drop decrements it.
        // So we're safe.
        let mut ref_count = unsafe { *self.inner }.ref_count.lock();
        *ref_count += 1;
        Arc { inner: self.inner }
    }
}

impl<T, const N: usize> Drop for Arc<T, N> {
    fn drop(&mut self) {
        // Safety: self.inner can never be invalid, because it's only destroyed when the ref_count is 0.
        // And we're only ever called when the ref_count is > 0 because we have a reference to self.
        // A reference to self means a ref_count > 0 because each clone increments the ref_count
        // and each drop decrements it.
        // So we're safe.
        let mut ref_count = unsafe { &mut *self.inner }.ref_count.lock();
        *ref_count -= 1;
        if *ref_count == 0 {
            // Safety: ref_count is now 0, that means we're the last reference to self.inner.
            // So we can safely drop it.
            unsafe { drop(Box::from_raw(self.inner as *mut ArcInner<T, N>)) }
        }
    }
}

impl<T, const N: usize> Deref for Arc<T, N> {
    type Target = T;
    fn deref(&self) -> &T {
        // Safety: self.inner can never be invalid, because it's only destroyed when the ref_count is 0.
        // And we're only ever called when the ref_count is > 0 because we have a reference to self.
        // A reference to self means a ref_count > 0 because each clone increments the ref_count
        // and each drop decrements it.
        // So we're safe.
        &unsafe { &*self.inner }.data
    }
}

unsafe impl<T, const N: usize> Send for Arc<T, N> where T: Send + Sync {}
unsafe impl<T, const N: usize> Sync for Arc<T, N> where T: Send + Sync {}
