//! `std::sync::mpsc`-shaped channel — multiple producer, single consumer.
//!
//! Built on top of [`crate::sync::Mutex`] + [`crate::sync::Condvar`] +
//! `VecDeque`. Sender is clonable; cloning shares the same channel.
//! When the last sender is dropped, in-flight messages are still
//! delivered; `recv` then returns `Err(RecvError)` once the queue
//! drains. When the receiver is dropped, subsequent `send` calls
//! return `Err(SendError(value))`.
//!
//! v1 — unbounded queue (no back-pressure), no try_send variant taking
//! a value back. The std API supports a `sync_channel(bound)` variant
//! and rendezvous channels — neither is wired here yet.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core_alloc::collections::VecDeque;
use core_alloc::sync::Arc;

use crate::sync::{Condvar, Mutex};

struct Inner<T> {
    queue: Mutex<VecDeque<T>>,
    cv: Condvar,
    senders: AtomicUsize,
    receiver_alive: AtomicBool,
}

/// The sender side. Cloneable.
pub struct Sender<T> {
    inner: Arc<Inner<T>>,
}

/// The receiver side. NOT cloneable (mpsc = multiple-producer SINGLE-consumer).
pub struct Receiver<T> {
    inner: Arc<Inner<T>>,
}

/// Returned by [`Sender::send`] when the receiver has been dropped — the
/// inner value is returned to the caller.
#[derive(Debug)]
pub struct SendError<T>(pub T);

/// Returned by [`Receiver::recv`] when all senders have been dropped and
/// the queue is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecvError;

/// Returned by [`Receiver::try_recv`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryRecvError {
    Empty,
    Disconnected,
}

/// Create a new mpsc channel — `(Sender, Receiver)`.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Inner {
        queue: Mutex::new(VecDeque::new()),
        cv: Condvar::new(),
        senders: AtomicUsize::new(1),
        receiver_alive: AtomicBool::new(true),
    });
    (
        Sender { inner: inner.clone() },
        Receiver { inner },
    )
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.inner.senders.fetch_add(1, Ordering::Relaxed);
        Self { inner: self.inner.clone() }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        // Decrement; if we were the last sender, wake a parked receiver so it
        // can see the channel as Disconnected.
        if self.inner.senders.fetch_sub(1, Ordering::Release) == 1 {
            self.inner.cv.notify_all();
        }
    }
}

impl<T> Sender<T> {
    /// Push `v` onto the channel. Returns `Err(SendError(v))` if the
    /// receiver has been dropped.
    pub fn send(&self, v: T) -> Result<(), SendError<T>> {
        if !self.inner.receiver_alive.load(Ordering::Acquire) {
            return Err(SendError(v));
        }
        {
            let mut q = self.inner.queue.lock();
            q.push_back(v);
        }
        self.inner.cv.notify_one();
        Ok(())
    }
}

impl<T> Receiver<T> {
    /// Block until a value is available. Returns `Err(RecvError)` only
    /// once every sender has been dropped AND the queue is empty.
    pub fn recv(&self) -> Result<T, RecvError> {
        let mut q = self.inner.queue.lock();
        loop {
            if let Some(v) = q.pop_front() {
                return Ok(v);
            }
            // Empty queue. If all senders are gone, we're done.
            if self.inner.senders.load(Ordering::Acquire) == 0 {
                return Err(RecvError);
            }
            // Park on the condvar until a sender pushes or drops.
            q = self.inner.cv.wait(q);
        }
    }

    /// Non-blocking pop. Distinguishes "empty for now" from "channel closed."
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut q = self.inner.queue.lock();
        if let Some(v) = q.pop_front() {
            return Ok(v);
        }
        if self.inner.senders.load(Ordering::Acquire) == 0 {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        // Mark dead; any subsequent send() returns SendError.
        self.inner.receiver_alive.store(false, Ordering::Release);
    }
}
