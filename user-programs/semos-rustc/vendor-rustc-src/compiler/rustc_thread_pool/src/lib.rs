//! Rustc thread-pool — SemOS single-threaded shim.
//!
//! M27 §1.4 + R4 B4. Upstream `rustc_thread_pool` is a vendored fork of
//! `rayon-core` (work-stealing scheduler, registry, sleep machinery,
//! latches, scope/spawn primitives, TLV). For SemOS we drop the
//! parallelism entirely; the public API is preserved as a sequential
//! shim so callers in `rustc_data_structures::sync` and
//! `rustc_interface::util` link unchanged.
//!
//! Behavior summary:
//! - `join(a, b)`        → runs `a()` then `b()`.
//! - `scope(|s| ...)`    → calls the closure with a synchronous Scope;
//!                         `s.spawn(|_| ...)` runs immediately.
//! - `spawn(f)`          → runs `f()` immediately on the current thread.
//! - `broadcast(f)`      → returns `vec![f(BroadcastContext{index:0,count:1})]`.
//! - `ThreadPool::install(f)` → just `f()`.
//! - `ThreadPoolBuilder::build_global()` → succeeds the first time,
//!                         fails with `GlobalPoolAlreadyInitialized` on
//!                         subsequent calls (matches upstream contract).
//! - Deadlock handler:   never invoked (single thread can't deadlock on
//!                         itself via work-stealing).
//! - `tlv::TLV`:         exposes `with`/`set`/`get` backed by a single
//!                         static `Cell<*const ()>` (no per-thread state
//!                         is needed in single-threaded mode).
//! - `WorkerLocal<T>`:   has 1 slot; `Deref` always yields that slot.
//!
//! Limitations vs upstream: there is no actual parallelism, so any
//! caller that depends on observing two closures running concurrently
//! will see them serialized. This is acceptable per M27 §1.4.

#![cfg_attr(test, allow(unused_crate_dependencies))]

use std::any::Any;
use std::cell::Cell;
use std::error::Error;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{fmt, io};

pub mod tlv {
    //! Single-threaded `TLV` shim. Upstream stores a `*const ()` in
    //! TLS to thread query state through `rayon::scope`. With one
    //! thread we just use a static `Cell` behind a `with` accessor.

    use std::cell::Cell;

    // M27 §1.4: upstream uses `thread_local!`. SemOS Ring-3 is
    // single-threaded per process, so a plain static (wrapped behind
    // the same accessor name) is sufficient.
    thread_local!(pub static TLV: Cell<*const ()> = const { Cell::new(core::ptr::null()) });

    #[derive(Copy, Clone)]
    pub(crate) struct Tlv(pub(crate) *const ());

    impl Tlv {
        #[inline]
        pub(crate) fn null() -> Self {
            Self(core::ptr::null())
        }
    }

    unsafe impl Sync for Tlv {}
    unsafe impl Send for Tlv {}

    #[inline]
    pub(crate) fn set(value: Tlv) {
        TLV.with(|tlv| tlv.set(value.0));
    }

    #[inline]
    pub(crate) fn get() -> Tlv {
        TLV.with(|tlv| Tlv(tlv.get()))
    }
}

// -----------------------------------------------------------------------------
// Public types
// -----------------------------------------------------------------------------

pub fn max_num_threads() -> usize {
    1
}

pub fn current_num_threads() -> usize {
    1
}

pub fn current_thread_index() -> Option<usize> {
    Some(0)
}

pub fn current_thread_has_pending_tasks() -> Option<bool> {
    Some(false)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Yield {
    #[default]
    Idle,
    Executed,
}

pub fn yield_now() -> Option<Yield> {
    Some(Yield::Idle)
}

pub fn yield_local() -> Option<Yield> {
    Some(Yield::Idle)
}

/// Single-threaded shim Registry — holds no state.
#[derive(Clone, Debug, Default)]
pub struct Registry;

impl Registry {
    pub fn current() -> std::sync::Arc<Registry> {
        std::sync::Arc::new(Registry)
    }

    pub fn current_num_threads() -> usize {
        1
    }

    pub fn num_threads(&self) -> usize {
        1
    }

    pub fn id(&self) -> usize {
        0
    }
}

/// Stub of upstream's `ThreadBuilder`. Carries no state.
#[derive(Debug, Default)]
pub struct ThreadBuilder {
    index: usize,
    name: Option<String>,
    stack_size: Option<usize>,
}

impl ThreadBuilder {
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    pub fn stack_size(&self) -> Option<usize> {
        self.stack_size
    }
    /// In the upstream rayon-core fork this runs the worker loop. Here
    /// it is a no-op so callers that pass a `wrapper` to
    /// `build_scoped` still type-check.
    pub fn run(self) {}
}

/// Marker for "this thread is blocked on a query cycle." In the shim
/// it is a no-op.
pub fn mark_blocked() {}
pub fn mark_unblocked(_registry: &Registry) {}

// -----------------------------------------------------------------------------
// Scope / join / spawn
// -----------------------------------------------------------------------------

/// Single-threaded `Scope`. `spawn`'d closures run immediately.
pub struct Scope<'scope> {
    _marker: PhantomData<&'scope mut &'scope ()>,
}

impl<'scope> Scope<'scope> {
    pub fn spawn<F>(&self, f: F)
    where
        F: FnOnce(&Scope<'scope>) + Send + 'scope,
    {
        f(self);
    }
}

/// Same as `Scope`. The FIFO ordering is moot with one worker.
pub struct ScopeFifo<'scope> {
    _marker: PhantomData<&'scope mut &'scope ()>,
}

impl<'scope> ScopeFifo<'scope> {
    pub fn spawn_fifo<F>(&self, f: F)
    where
        F: FnOnce(&ScopeFifo<'scope>) + Send + 'scope,
    {
        f(self);
    }
}

pub fn scope<'scope, OP, R>(op: OP) -> R
where
    OP: FnOnce(&Scope<'scope>) -> R + Send,
    R: Send,
{
    let s = Scope { _marker: PhantomData };
    op(&s)
}

pub fn scope_fifo<'scope, OP, R>(op: OP) -> R
where
    OP: FnOnce(&ScopeFifo<'scope>) -> R + Send,
    R: Send,
{
    let s = ScopeFifo { _marker: PhantomData };
    op(&s)
}

pub fn in_place_scope<'scope, OP, R>(op: OP) -> R
where
    OP: FnOnce(&Scope<'scope>) -> R,
{
    let s = Scope { _marker: PhantomData };
    op(&s)
}

pub fn in_place_scope_fifo<'scope, OP, R>(op: OP) -> R
where
    OP: FnOnce(&ScopeFifo<'scope>) -> R,
{
    let s = ScopeFifo { _marker: PhantomData };
    op(&s)
}

pub fn join<A, B, RA, RB>(oper_a: A, oper_b: B) -> (RA, RB)
where
    A: FnOnce() -> RA + Send,
    B: FnOnce() -> RB + Send,
    RA: Send,
    RB: Send,
{
    (oper_a(), oper_b())
}

pub fn join_context<A, B, RA, RB>(oper_a: A, oper_b: B) -> (RA, RB)
where
    A: FnOnce(FnContext) -> RA + Send,
    B: FnOnce(FnContext) -> RB + Send,
    RA: Send,
    RB: Send,
{
    (oper_a(FnContext::new(false)), oper_b(FnContext::new(false)))
}

pub fn spawn<F>(func: F)
where
    F: FnOnce() + Send + 'static,
{
    func();
}

pub fn spawn_fifo<F>(func: F)
where
    F: FnOnce() + Send + 'static,
{
    func();
}

// -----------------------------------------------------------------------------
// Broadcast
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct BroadcastContext {
    index: usize,
    count: usize,
}

impl BroadcastContext {
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn num_threads(&self) -> usize {
        self.count
    }
}

pub fn broadcast<F, R>(op: F) -> Vec<R>
where
    F: Fn(BroadcastContext) -> R + Sync,
    R: Send,
{
    vec![op(BroadcastContext { index: 0, count: 1 })]
}

pub fn spawn_broadcast<F>(op: F)
where
    F: Fn(BroadcastContext) + Send + Sync + 'static,
{
    op(BroadcastContext { index: 0, count: 1 });
}

// -----------------------------------------------------------------------------
// FnContext
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub struct FnContext {
    migrated: bool,
    _marker: PhantomData<*mut ()>,
}

impl FnContext {
    #[inline]
    fn new(migrated: bool) -> Self {
        FnContext { migrated, _marker: PhantomData }
    }
    #[inline]
    pub fn migrated(&self) -> bool {
        self.migrated
    }
}

// -----------------------------------------------------------------------------
// ThreadPool / ThreadPoolBuilder
// -----------------------------------------------------------------------------

pub struct ThreadPool;

impl ThreadPool {
    pub fn install<F, R>(&self, op: F) -> R
    where
        F: FnOnce() -> R + Send,
        R: Send,
    {
        op()
    }

    pub fn current_num_threads(&self) -> usize {
        1
    }

    pub fn current_thread_index(&self) -> Option<usize> {
        Some(0)
    }

    pub fn join<A, B, RA, RB>(&self, oper_a: A, oper_b: B) -> (RA, RB)
    where
        A: FnOnce() -> RA + Send,
        B: FnOnce() -> RB + Send,
        RA: Send,
        RB: Send,
    {
        join(oper_a, oper_b)
    }

    pub fn scope<'scope, OP, R>(&self, op: OP) -> R
    where
        OP: FnOnce(&Scope<'scope>) -> R + Send,
        R: Send,
    {
        scope(op)
    }

    pub fn spawn<F>(&self, func: F)
    where
        F: FnOnce() + Send + 'static,
    {
        func()
    }

    pub fn yield_now(&self) -> Option<Yield> {
        Some(Yield::Idle)
    }

    pub fn yield_local(&self) -> Option<Yield> {
        Some(Yield::Idle)
    }

    fn build(_builder: ThreadPoolBuilder<DefaultSpawn>) -> Result<ThreadPool, ThreadPoolBuildError> {
        Ok(ThreadPool)
    }

    fn wait_until_stopped(&self) {}
}

/// Type-state marker for the default spawn handler.
#[derive(Debug)]
pub struct DefaultSpawn;

/// Type-state marker for a custom spawn handler. Carries an unused
/// closure to satisfy callers that pattern-match generically.
#[derive(Debug)]
pub struct CustomSpawn<F> {
    _f: F,
}

impl<F> CustomSpawn<F> {
    fn new(f: F) -> Self {
        CustomSpawn { _f: f }
    }
}

/// Marker trait corresponding to upstream's `ThreadSpawn`. Implemented
/// for both `DefaultSpawn` and `CustomSpawn<F>` so generic
/// `ThreadPoolBuilder<S: ThreadSpawn>` bounds resolve.
pub trait ThreadSpawn {}
impl ThreadSpawn for DefaultSpawn {}
impl<F> ThreadSpawn for CustomSpawn<F> {}

pub struct ThreadPoolBuilder<S = DefaultSpawn> {
    num_threads: usize,
    stack_size: Option<usize>,
    breadth_first: bool,
    _spawn: S,
}

impl Default for ThreadPoolBuilder<DefaultSpawn> {
    fn default() -> Self {
        ThreadPoolBuilder {
            num_threads: 0,
            stack_size: None,
            breadth_first: false,
            _spawn: DefaultSpawn,
        }
    }
}

impl ThreadPoolBuilder<DefaultSpawn> {
    pub fn new() -> Self {
        Self::default()
    }
}

// The global-init flag enforces the upstream contract that
// `build_global` may only succeed once per process.
static GLOBAL_INITIALIZED: AtomicBool = AtomicBool::new(false);

impl<S: ThreadSpawn> ThreadPoolBuilder<S> {
    pub fn build(self) -> Result<ThreadPool, ThreadPoolBuildError> {
        Ok(ThreadPool)
    }

    pub fn build_global(self) -> Result<(), ThreadPoolBuildError> {
        if GLOBAL_INITIALIZED.swap(true, Ordering::SeqCst) {
            return Err(ThreadPoolBuildError::new(ErrorKind::GlobalPoolAlreadyInitialized));
        }
        Ok(())
    }
}

impl ThreadPoolBuilder<DefaultSpawn> {
    /// Upstream uses `std::thread::scope` here. The shim runs
    /// everything inline; `wrapper` is invoked once with a dummy
    /// `ThreadBuilder` for API parity, then `with_pool` is called.
    pub fn build_scoped<W, F, R>(
        self,
        wrapper: W,
        with_pool: F,
    ) -> Result<R, ThreadPoolBuildError>
    where
        W: Fn(ThreadBuilder) + Sync,
        F: FnOnce(&ThreadPool) -> R,
    {
        wrapper(ThreadBuilder::default());
        let pool = ThreadPool;
        let result = with_pool(&pool);
        pool.wait_until_stopped();
        Ok(result)
    }
}

impl<S> ThreadPoolBuilder<S> {
    pub fn num_threads(mut self, num_threads: usize) -> Self {
        self.num_threads = num_threads;
        self
    }

    pub fn stack_size(mut self, stack_size: usize) -> Self {
        self.stack_size = Some(stack_size);
        self
    }

    pub fn thread_name<F>(self, _closure: F) -> Self
    where
        F: FnMut(usize) -> String + 'static,
    {
        self
    }

    pub fn panic_handler<H>(self, _h: H) -> Self
    where
        H: Fn(Box<dyn Any + Send>) + Send + Sync + 'static,
    {
        self
    }

    pub fn deadlock_handler<H>(self, _h: H) -> Self
    where
        H: Fn() + Send + Sync + 'static,
    {
        // M27 §1.4 + R4 B4: in single-threaded mode there is no other
        // worker thread to invoke a deadlock handler. Query cycles
        // must be detected synchronously by callers; this is a no-op.
        self
    }

    pub fn start_handler<H>(self, _h: H) -> Self
    where
        H: Fn(usize) + Send + Sync + 'static,
    {
        self
    }

    pub fn exit_handler<H>(self, _h: H) -> Self
    where
        H: Fn(usize) + Send + Sync + 'static,
    {
        self
    }

    pub fn acquire_thread_handler<H>(self, _h: H) -> Self
    where
        H: Fn() + Send + Sync + 'static,
    {
        self
    }

    pub fn release_thread_handler<H>(self, _h: H) -> Self
    where
        H: Fn() + Send + Sync + 'static,
    {
        self
    }

    pub fn spawn_handler<F>(self, spawn: F) -> ThreadPoolBuilder<CustomSpawn<F>>
    where
        F: FnMut(ThreadBuilder) -> io::Result<()>,
    {
        ThreadPoolBuilder {
            num_threads: self.num_threads,
            stack_size: self.stack_size,
            breadth_first: self.breadth_first,
            _spawn: CustomSpawn::new(spawn),
        }
    }

    #[deprecated(note = "use `scope_fifo` and `spawn_fifo` for similar effect")]
    pub fn breadth_first(mut self) -> Self {
        self.breadth_first = true;
        self
    }
}

impl<S> fmt::Debug for ThreadPoolBuilder<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadPoolBuilder")
            .field("num_threads", &self.num_threads)
            .field("stack_size", &self.stack_size)
            .field("breadth_first", &self.breadth_first)
            .finish()
    }
}

// -----------------------------------------------------------------------------
// WorkerLocal
// -----------------------------------------------------------------------------

/// Single-slot `WorkerLocal`. Upstream allocates one slot per worker
/// thread; with one worker, the storage is a single value.
pub struct WorkerLocal<T> {
    inner: T,
}

unsafe impl<T: Send> Sync for WorkerLocal<T> {}

impl<T> WorkerLocal<T> {
    pub fn new<F: FnMut(usize) -> T>(mut initial: F) -> WorkerLocal<T> {
        WorkerLocal { inner: initial(0) }
    }

    pub fn into_inner(self) -> Vec<T> {
        vec![self.inner]
    }
}

impl<T> WorkerLocal<Vec<T>> {
    pub fn join(self) -> Vec<T> {
        self.inner
    }
}

impl<T: fmt::Debug> fmt::Debug for WorkerLocal<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerLocal").field("inner", &self.inner).finish()
    }
}

impl<T> std::ops::Deref for WorkerLocal<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &T {
        &self.inner
    }
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub struct ThreadPoolBuildError {
    kind: ErrorKind,
}

#[derive(Debug)]
enum ErrorKind {
    GlobalPoolAlreadyInitialized,
    #[allow(dead_code)]
    IOError(io::Error),
}

impl ThreadPoolBuildError {
    fn new(kind: ErrorKind) -> Self {
        Self { kind }
    }

    pub fn is_unsupported(&self) -> bool {
        matches!(&self.kind, ErrorKind::IOError(e) if e.kind() == io::ErrorKind::Unsupported)
    }
}

impl Error for ThreadPoolBuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            ErrorKind::GlobalPoolAlreadyInitialized => None,
            ErrorKind::IOError(e) => Some(e),
        }
    }
}

impl fmt::Display for ThreadPoolBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::GlobalPoolAlreadyInitialized => {
                "The global thread pool has already been initialized.".fmt(f)
            }
            ErrorKind::IOError(e) => e.fmt(f),
        }
    }
}

// Required by `ThreadPool::build`'s old signature.
#[allow(dead_code)]
fn _builder_construct() -> ThreadPoolBuilder<DefaultSpawn> {
    ThreadPoolBuilder::new()
}

// Touch `Cell` so the unused import warning doesn't fire.
#[allow(dead_code)]
fn _touch() -> Cell<u8> {
    Cell::new(0)
}
