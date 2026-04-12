#![expect(
    deprecated,
    reason = "this module implements the deprecated blocking lazy compatibility shim in place, so internal references intentionally stay local to the shim implementation"
)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::ThreadId;

/// Blocking lazy initialization primitive for narrow compatibility shims.
///
/// This type is thread-oriented and uses `Condvar`, so it is not a good
/// runtime-facing boundary for async-heavy code. Prefer eager snapshots or a
/// crate-specific handle that swaps already-loaded `Arc` state.
enum LazyState<T: ?Sized, E> {
    Uninitialized,
    Initializing {
        thread_id: ThreadId,
        waiting_threads: usize,
    },
    Failed {
        error: Arc<E>,
        waiting_threads: usize,
    },
    Initialized(Arc<T>),
}

#[deprecated(
    since = "0.1.0",
    note = "LazyInitError is part of a blocking compatibility shim. Prefer eager snapshots or runtime-owned handles at crate boundaries."
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LazyInitConflictKind {
    ReentrantInitialization,
    SameThreadInitializationConflict,
    CrossThreadCycleDetected,
}

#[deprecated(
    since = "0.1.0",
    note = "LazyInitConflictKind is part of a blocking compatibility shim. Prefer eager snapshots or runtime-owned handles at crate boundaries."
)]
#[derive(Debug)]
pub enum LazyInitError<E> {
    Inner(Arc<E>),
    /// The current call stack tried to initialize the same `LazyValue` again.
    ReentrantInitialization,
    /// Another access re-entered the same OS thread while initialization was still in flight.
    ///
    /// Waiting here would block the thread behind its own unfinished initialization attempt, so
    /// the compatibility shim fails fast instead.
    SameThreadInitializationConflict,
    /// Two tracked `LazyValue` initializers would end up waiting on each other across threads.
    CrossThreadCycleDetected,
}

impl<E> LazyInitError<E> {
    #[must_use]
    pub fn conflict_kind(&self) -> Option<LazyInitConflictKind> {
        match self {
            Self::Inner(_) => None,
            Self::ReentrantInitialization => Some(LazyInitConflictKind::ReentrantInitialization),
            Self::SameThreadInitializationConflict => {
                Some(LazyInitConflictKind::SameThreadInitializationConflict)
            }
            Self::CrossThreadCycleDetected => Some(LazyInitConflictKind::CrossThreadCycleDetected),
        }
    }
}

#[deprecated(
    since = "0.1.0",
    note = "LazyValue is a blocking compatibility shim. Prefer eager snapshots or runtime-owned handles at crate boundaries."
)]
pub struct LazyValue<T: ?Sized, E> {
    id: OnceLock<u64>,
    state: Mutex<LazyState<T, E>>,
    ready: Condvar,
}

impl<T: ?Sized, E> LazyValue<T, E> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            id: OnceLock::new(),
            state: Mutex::new(LazyState::Uninitialized),
            ready: Condvar::new(),
        }
    }

    pub fn set(&self, value: Arc<T>) {
        let replaced_owner = {
            let mut guard = lock_unpoisoned(&self.state);
            let replaced_owner = match &*guard {
                LazyState::Initializing { thread_id, .. } => Some(*thread_id),
                _ => None,
            };
            *guard = LazyState::Initialized(value);
            replaced_owner
        };
        if let Some(thread_id) = replaced_owner {
            finish_lazy_initialization(thread_id, self.id());
        }
        self.ready.notify_all();
    }

    /// Returns the initialized value, running `initializer` at most once per
    /// successful initialization attempt.
    ///
    /// Direct recursive initialization from the current call stack is rejected
    /// explicitly. If another access reaches the same `LazyValue` on the same
    /// OS thread while initialization is still in flight, the compatibility
    /// shim fails fast instead of blocking the thread behind its own unfinished
    /// attempt. Other callers wait for the in-flight attempt to settle and
    /// then observe its result.
    ///
    /// Thread-level cross-thread wait cycles between tracked `LazyValue`
    /// initializers/waiters are rejected before blocking.
    ///
    /// Callers still must avoid unrelated waits that can re-enter the same
    /// `LazyValue` without passing through another tracked `LazyValue` edge;
    /// those external cycles remain outside this primitive's view.
    pub fn get_or_init(
        &self,
        initializer: impl FnOnce() -> Result<Arc<T>, E>,
    ) -> Result<Arc<T>, LazyInitError<E>> {
        let thread_id = std::thread::current().id();
        let lazy_id = self.id();
        let mut initializer = Some(initializer);
        let mut waiting_for_current_attempt = false;

        loop {
            let mut guard = lock_unpoisoned(&self.state);
            match &mut *guard {
                LazyState::Initialized(value) => {
                    if waiting_for_current_attempt {
                        finish_lazy_wait(thread_id, lazy_id);
                    }
                    return Ok(Arc::clone(value));
                }
                LazyState::Initializing {
                    thread_id: owner_thread_id,
                    waiting_threads,
                } => {
                    if current_call_stack_is_initializing_lazy(lazy_id) {
                        return Err(LazyInitError::ReentrantInitialization);
                    }

                    if *owner_thread_id == thread_id {
                        return Err(LazyInitError::SameThreadInitializationConflict);
                    }

                    if !waiting_for_current_attempt {
                        if !begin_lazy_wait(thread_id, lazy_id) {
                            return Err(LazyInitError::CrossThreadCycleDetected);
                        }
                        *waiting_threads += 1;
                        waiting_for_current_attempt = true;
                    }

                    drop(wait_unpoisoned(&self.ready, guard));
                }
                LazyState::Failed {
                    error,
                    waiting_threads,
                } => {
                    if waiting_for_current_attempt {
                        finish_lazy_wait(thread_id, lazy_id);
                        let error = Arc::clone(error);
                        if *waiting_threads > 0 {
                            *waiting_threads -= 1;
                        }
                        if *waiting_threads == 0 {
                            *guard = LazyState::Uninitialized;
                            self.ready.notify_all();
                        }
                        return Err(LazyInitError::Inner(error));
                    }

                    if *waiting_threads == 0 {
                        *guard = LazyState::Uninitialized;
                    } else {
                        drop(wait_unpoisoned(&self.ready, guard));
                    }
                }
                LazyState::Uninitialized => {
                    if waiting_for_current_attempt {
                        finish_lazy_wait(thread_id, lazy_id);
                    }
                    *guard = LazyState::Initializing {
                        thread_id,
                        waiting_threads: 0,
                    };
                    begin_lazy_initialization(thread_id, lazy_id);
                    waiting_for_current_attempt = false;
                    drop(guard);

                    let mut reset = InitializationGuard::new(self, thread_id, lazy_id);
                    let result = initializer.take().expect("initializer must only run once")();

                    let mut guard = lock_unpoisoned(&self.state);
                    if matches!(
                        &*guard,
                        LazyState::Initializing {
                            thread_id: owner_thread_id,
                            waiting_threads: _,
                        } if *owner_thread_id == thread_id
                    ) {
                        let outcome = match result {
                            Ok(value) => {
                                let shared = Arc::clone(&value);
                                *guard = LazyState::Initialized(value);
                                finish_lazy_initialization(thread_id, lazy_id);
                                Ok(shared)
                            }
                            Err(error) => {
                                let waiting_threads = match &*guard {
                                    LazyState::Initializing {
                                        waiting_threads, ..
                                    } => *waiting_threads,
                                    _ => unreachable!(),
                                };
                                let error = Arc::new(error);
                                *guard = LazyState::Failed {
                                    error: Arc::clone(&error),
                                    waiting_threads,
                                };
                                finish_lazy_initialization(thread_id, lazy_id);
                                Err(LazyInitError::Inner(error))
                            }
                        };
                        self.ready.notify_all();
                        reset.disarm();
                        return outcome;
                    }
                    finish_lazy_initialization(thread_id, lazy_id);
                    reset.disarm();
                }
            }
        }
    }
}

impl<T: ?Sized, E> Default for LazyValue<T, E> {
    fn default() -> Self {
        Self::new()
    }
}

struct InitializationGuard<'a, T: ?Sized, E> {
    value: &'a LazyValue<T, E>,
    thread_id: ThreadId,
    lazy_id: u64,
    active: bool,
}

impl<'a, T: ?Sized, E> InitializationGuard<'a, T, E> {
    fn new(value: &'a LazyValue<T, E>, thread_id: ThreadId, lazy_id: u64) -> Self {
        Self {
            value,
            thread_id,
            lazy_id,
            active: true,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl<T: ?Sized, E> Drop for InitializationGuard<'_, T, E> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let mut guard = lock_unpoisoned(&self.value.state);
        if matches!(
            &*guard,
            LazyState::Initializing {
                thread_id,
                waiting_threads: _,
            } if *thread_id == self.thread_id
        ) {
            *guard = LazyState::Uninitialized;
            finish_lazy_initialization(self.thread_id, self.lazy_id);
            self.value.ready.notify_all();
        }
    }
}

#[derive(Debug, Default)]
struct LazyWaitGraph {
    threads: HashMap<ThreadId, ThreadWaitState>,
    owners: HashMap<u64, ThreadId>,
}

#[derive(Debug, Default)]
struct ThreadWaitState {
    waiting_on: Option<u64>,
    initializing: HashSet<u64>,
}

impl LazyWaitGraph {
    fn begin_initialization(&mut self, thread_id: ThreadId, lazy_id: u64) {
        let state = self.threads.entry(thread_id).or_default();
        state.initializing.insert(lazy_id);
        self.owners.insert(lazy_id, thread_id);
    }

    fn finish_initialization(&mut self, thread_id: ThreadId, lazy_id: u64) {
        if let Some(state) = self.threads.get_mut(&thread_id) {
            state.initializing.remove(&lazy_id);
        }
        if self.owners.get(&lazy_id).copied() == Some(thread_id) {
            self.owners.remove(&lazy_id);
        }
        self.compact_thread(thread_id);
    }

    fn begin_wait(&mut self, thread_id: ThreadId, lazy_id: u64) -> bool {
        if self.wait_would_cycle(thread_id, lazy_id) {
            return false;
        }
        self.threads.entry(thread_id).or_default().waiting_on = Some(lazy_id);
        true
    }

    fn finish_wait(&mut self, thread_id: ThreadId, lazy_id: u64) {
        if let Some(state) = self.threads.get_mut(&thread_id)
            && state.waiting_on == Some(lazy_id)
        {
            state.waiting_on = None;
        }
        self.compact_thread(thread_id);
    }

    fn wait_would_cycle(&self, waiting_thread: ThreadId, lazy_id: u64) -> bool {
        let mut next_lazy = Some(lazy_id);
        let mut seen_threads = HashSet::new();

        while let Some(current_lazy) = next_lazy {
            let Some(owner_thread) = self.owners.get(&current_lazy).copied() else {
                return false;
            };
            if owner_thread == waiting_thread {
                return true;
            }
            if !seen_threads.insert(owner_thread) {
                return true;
            }
            next_lazy = self
                .threads
                .get(&owner_thread)
                .and_then(|state| state.waiting_on);
        }

        false
    }

    fn compact_thread(&mut self, thread_id: ThreadId) {
        let remove_thread = self
            .threads
            .get(&thread_id)
            .is_some_and(|state| state.waiting_on.is_none() && state.initializing.is_empty());
        if remove_thread {
            self.threads.remove(&thread_id);
        }
    }
}

fn begin_lazy_initialization(thread_id: ThreadId, lazy_id: u64) {
    ACTIVE_LAZY_INITIALIZATIONS.with(|active| {
        active.borrow_mut().insert(lazy_id);
    });
    lock_unpoisoned(lazy_wait_graph()).begin_initialization(thread_id, lazy_id);
}

fn finish_lazy_initialization(thread_id: ThreadId, lazy_id: u64) {
    ACTIVE_LAZY_INITIALIZATIONS.with(|active| {
        active.borrow_mut().remove(&lazy_id);
    });
    lock_unpoisoned(lazy_wait_graph()).finish_initialization(thread_id, lazy_id);
}

fn begin_lazy_wait(thread_id: ThreadId, lazy_id: u64) -> bool {
    lock_unpoisoned(lazy_wait_graph()).begin_wait(thread_id, lazy_id)
}

fn finish_lazy_wait(thread_id: ThreadId, lazy_id: u64) {
    lock_unpoisoned(lazy_wait_graph()).finish_wait(thread_id, lazy_id);
}

fn lazy_wait_graph() -> &'static Mutex<LazyWaitGraph> {
    static WAIT_GRAPH: OnceLock<Mutex<LazyWaitGraph>> = OnceLock::new();
    WAIT_GRAPH.get_or_init(|| Mutex::new(LazyWaitGraph::default()))
}

thread_local! {
    static ACTIVE_LAZY_INITIALIZATIONS: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

fn current_call_stack_is_initializing_lazy(lazy_id: u64) -> bool {
    ACTIVE_LAZY_INITIALIZATIONS.with(|active| active.borrow().contains(&lazy_id))
}

#[cfg(test)]
fn reset_lazy_wait_graph_for_test() {
    *lock_unpoisoned(lazy_wait_graph()) = LazyWaitGraph::default();
}

#[cfg(test)]
fn thread_waiting_on_lazy_for_test(thread_id: ThreadId) -> Option<u64> {
    lock_unpoisoned(lazy_wait_graph())
        .threads
        .get(&thread_id)
        .and_then(|state| state.waiting_on)
}

fn next_lazy_id() -> u64 {
    static NEXT_LAZY_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_LAZY_ID.fetch_add(1, AtomicOrdering::Relaxed)
}

fn lock_unpoisoned<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn wait_unpoisoned<'a, T>(
    condvar: &Condvar,
    guard: std::sync::MutexGuard<'a, T>,
) -> std::sync::MutexGuard<'a, T> {
    condvar
        .wait(guard)
        .unwrap_or_else(|poison| poison.into_inner())
}

impl<T: ?Sized, E> LazyValue<T, E> {
    fn id(&self) -> u64 {
        *self.id.get_or_init(next_lazy_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    fn is_initialized<T: ?Sized, E>(lazy: &LazyValue<T, E>) -> bool {
        matches!(&*lock_unpoisoned(&lazy.state), LazyState::Initialized(_))
    }

    fn lazy_value_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn concurrent_access_waits_for_initialization_to_finish() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();
        let lazy = Arc::new(LazyValue::<u32, &'static str>::new());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let initializing = Arc::clone(&lazy);

        let handle = thread::spawn(move || {
            let value = initializing
                .get_or_init(|| -> Result<Arc<u32>, &'static str> {
                    entered_tx.send(()).expect("signal initializer entered");
                    release_rx.recv().expect("release initializer");
                    Ok(Arc::new(1))
                })
                .expect("initialization should succeed");
            assert_eq!(*value, 1);
        });

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("initializer should start");

        let waiting = Arc::clone(&lazy);
        let (value_tx, value_rx) = mpsc::channel();
        let waiting_handle = thread::spawn(move || {
            let value = waiting
                .get_or_init(|| -> Result<Arc<u32>, &'static str> {
                    panic!("concurrent access must not run a second initializer")
                })
                .expect("concurrent access should observe initialized value");
            value_tx.send(*value).expect("publish initialized value");
        });

        assert!(
            value_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "concurrent access should wait for initialization to complete",
        );

        release_tx.send(()).expect("release initializer");
        handle.join().expect("join initializer thread");
        waiting_handle.join().expect("join waiting thread");

        assert_eq!(
            value_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("waiting thread should receive initialized value"),
            1,
        );

        let value = lazy
            .get_or_init(|| -> Result<Arc<u32>, &'static str> { Ok(Arc::new(3)) })
            .expect("initialized value should be visible");
        assert_eq!(*value, 1);
    }

    #[test]
    fn failed_initialization_can_retry_after_error() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();
        let lazy = LazyValue::<u32, &'static str>::new();

        let error = lazy
            .get_or_init(|| Err("init failed"))
            .expect_err("initialization should fail");
        assert!(matches!(error, LazyInitError::Inner(_)));
        assert!(!is_initialized(&lazy));

        let value = lazy
            .get_or_init(|| -> Result<Arc<u32>, &'static str> { Ok(Arc::new(7)) })
            .expect("retry should succeed");
        assert_eq!(*value, 7);
        assert!(is_initialized(&lazy));
    }

    #[test]
    fn concurrent_waiters_observe_same_failure_before_retry() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();
        let lazy = Arc::new(LazyValue::<u32, &'static str>::new());
        let attempts = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let initializing = Arc::clone(&lazy);
        let init_attempts = Arc::clone(&attempts);

        let handle = thread::spawn(move || {
            initializing.get_or_init(|| -> Result<Arc<u32>, &'static str> {
                assert_eq!(init_attempts.fetch_add(1, Ordering::SeqCst), 0);
                entered_tx.send(()).expect("signal initializer entered");
                release_rx.recv().expect("release initializer");
                Err("init failed")
            })
        });

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("initializer should start");

        let waiting = Arc::clone(&lazy);
        let (result_tx, result_rx) = mpsc::channel();
        let waiting_handle = thread::spawn(move || {
            let result = waiting
                .get_or_init(|| -> Result<Arc<u32>, &'static str> {
                    panic!("waiter must not rerun initializer")
                })
                .map(|value| *value)
                .map_err(|error| match error {
                    LazyInitError::Inner(error) => *error,
                    LazyInitError::ReentrantInitialization => "reentrant",
                    LazyInitError::SameThreadInitializationConflict => "same-thread",
                    LazyInitError::CrossThreadCycleDetected => "cycle",
                });
            result_tx.send(result).expect("publish waiter result");
        });

        assert!(
            result_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "waiter should block until the in-flight attempt settles",
        );

        release_tx.send(()).expect("release initializer");
        let init_error = handle
            .join()
            .expect("join initializer thread")
            .expect_err("initializer should fail");
        waiting_handle.join().expect("join waiter thread");

        assert!(matches!(init_error, LazyInitError::Inner(error) if *error == "init failed"));
        assert_eq!(
            result_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("waiter result should be published"),
            Err("init failed"),
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(!is_initialized(&lazy));

        let value = lazy
            .get_or_init(|| -> Result<Arc<u32>, &'static str> {
                assert_eq!(attempts.fetch_add(1, Ordering::SeqCst), 1);
                Ok(Arc::new(9))
            })
            .expect("retry should succeed");
        assert_eq!(*value, 9);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(is_initialized(&lazy));
    }

    #[test]
    fn panic_during_initialization_resets_state_for_retry() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();
        let lazy = Arc::new(LazyValue::<u32, &'static str>::new());
        let panicking = Arc::clone(&lazy);

        let panic_payload = catch_unwind(AssertUnwindSafe(move || {
            let _ = panicking.get_or_init(|| -> Result<Arc<u32>, &'static str> { panic!("boom") });
        }))
        .expect_err("initializer panic should unwind");
        let panic_text = if let Some(text) = panic_payload.downcast_ref::<&'static str>() {
            *text
        } else {
            panic!("unexpected panic payload")
        };
        assert_eq!(panic_text, "boom");
        assert!(!is_initialized(&lazy));

        let value = lazy
            .get_or_init(|| -> Result<Arc<u32>, &'static str> { Ok(Arc::new(5)) })
            .expect("retry after panic should succeed");
        assert_eq!(*value, 5);
        assert!(is_initialized(&lazy));
    }

    #[test]
    fn spurious_notify_keeps_wait_edge_until_attempt_finishes() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();
        let lazy = Arc::new(LazyValue::<u32, &'static str>::new());
        let lazy_id = lazy.id();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let initializing = Arc::clone(&lazy);

        let init_handle = thread::spawn(move || {
            initializing
                .get_or_init(|| -> Result<Arc<u32>, &'static str> {
                    entered_tx.send(()).expect("signal initializer entered");
                    release_rx.recv().expect("release initializer");
                    Ok(Arc::new(11))
                })
                .expect("initialization should succeed");
        });

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("initializer should start");

        let waiting = Arc::clone(&lazy);
        let (waiter_id_tx, waiter_id_rx) = mpsc::channel();
        let waiter_handle = thread::spawn(move || {
            waiter_id_tx
                .send(thread::current().id())
                .expect("publish waiter thread id");
            let value = waiting
                .get_or_init(|| -> Result<Arc<u32>, &'static str> {
                    panic!("waiter must not rerun initializer")
                })
                .expect("waiter should observe initialized value");
            assert_eq!(*value, 11);
        });

        let waiter_thread_id = waiter_id_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("waiter should publish thread id");
        let start = std::time::Instant::now();
        while thread_waiting_on_lazy_for_test(waiter_thread_id) != Some(lazy_id) {
            assert!(
                start.elapsed() < Duration::from_secs(1),
                "waiter should register wait edge before spurious notify",
            );
            thread::sleep(Duration::from_millis(10));
        }

        let check = catch_unwind(AssertUnwindSafe(|| {
            for _ in 0..5 {
                lazy.ready.notify_all();
                thread::sleep(Duration::from_millis(20));
                assert_eq!(
                    thread_waiting_on_lazy_for_test(waiter_thread_id),
                    Some(lazy_id),
                    "spurious notify must not clear the wait edge before the attempt settles",
                );
            }
        }));

        release_tx.send(()).expect("release initializer");
        init_handle.join().expect("join initializer thread");
        waiter_handle.join().expect("join waiter thread");
        check.expect("spurious notify should keep wait edge registered");
    }

    #[test]
    fn set_publishes_replacement_value() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();
        let lazy = LazyValue::<u32, &'static str>::new();
        lazy.set(Arc::new(1));
        assert_eq!(
            *lazy
                .get_or_init(|| -> Result<Arc<u32>, &'static str> {
                    panic!("set value should bypass initializer")
                })
                .expect("set value should be visible"),
            1,
        );

        lazy.set(Arc::new(7));
        assert_eq!(
            *lazy
                .get_or_init(|| -> Result<Arc<u32>, &'static str> {
                    panic!("replacement should stay visible")
                })
                .expect("replacement should be visible"),
            7,
        );
    }

    #[test]
    fn same_thread_reentrancy_is_rejected() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();
        fn reentrant_init(lazy: &LazyValue<u32, &'static str>) -> Result<Arc<u32>, &'static str> {
            match lazy.get_or_init(|| Ok(Arc::new(2))) {
                Err(LazyInitError::ReentrantInitialization) => Ok(Arc::new(1)),
                _ => panic!("expected reentrant initialization error"),
            }
        }

        let lazy = LazyValue::<u32, &'static str>::new();
        let value = lazy
            .get_or_init(|| reentrant_init(&lazy))
            .expect("outer initialization should recover");
        assert_eq!(*value, 1);
    }

    #[test]
    fn same_thread_initialization_conflict_fails_fast() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();

        let lazy = LazyValue::<u32, &'static str>::new();
        *lock_unpoisoned(&lazy.state) = LazyState::Initializing {
            thread_id: std::thread::current().id(),
            waiting_threads: 0,
        };

        let error = lazy
            .get_or_init(|| -> Result<Arc<u32>, &'static str> { Ok(Arc::new(1)) })
            .expect_err("same-thread in-flight state should fail fast");
        assert!(matches!(
            error,
            LazyInitError::SameThreadInitializationConflict
        ));
        assert_eq!(
            error.conflict_kind(),
            Some(LazyInitConflictKind::SameThreadInitializationConflict)
        );
    }

    #[test]
    fn reentrancy_detection_does_not_depend_on_owner_thread_id() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();

        let lazy = LazyValue::<u32, &'static str>::new();
        let lazy_id = lazy.id();
        let other_thread_id = thread::spawn(|| std::thread::current().id())
            .join()
            .expect("join helper thread");

        *lock_unpoisoned(&lazy.state) = LazyState::Initializing {
            thread_id: other_thread_id,
            waiting_threads: 0,
        };
        begin_lazy_initialization(std::thread::current().id(), lazy_id);

        let err = lazy
            .get_or_init(|| -> Result<Arc<u32>, &'static str> { Ok(Arc::new(1)) })
            .expect_err("active call stack should still be treated as reentrant");
        assert!(matches!(err, LazyInitError::ReentrantInitialization));
        assert_eq!(
            err.conflict_kind(),
            Some(LazyInitConflictKind::ReentrantInitialization)
        );

        finish_lazy_initialization(std::thread::current().id(), lazy_id);
        *lock_unpoisoned(&lazy.state) = LazyState::Uninitialized;
    }

    #[test]
    fn inner_errors_do_not_report_conflict_kinds() {
        let error = LazyInitError::Inner(Arc::new("boom"));
        assert_eq!(error.conflict_kind(), None);
    }

    #[test]
    fn cross_thread_cycle_is_rejected_instead_of_deadlocking() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();
        let lazy_a = Arc::new(LazyValue::<u32, &'static str>::new());
        let lazy_b = Arc::new(LazyValue::<u32, &'static str>::new());
        let barrier = Arc::new(Barrier::new(2));
        let cycle_count = Arc::new(AtomicUsize::new(0));
        let (result_tx, result_rx) = mpsc::channel();

        let a_for_first = Arc::clone(&lazy_a);
        let b_for_first = Arc::clone(&lazy_b);
        let barrier_for_first = Arc::clone(&barrier);
        let cycles_for_first = Arc::clone(&cycle_count);
        let result_tx_first = result_tx.clone();
        let first = thread::spawn(move || {
            let result = a_for_first
                .get_or_init(|| {
                    barrier_for_first.wait();
                    match b_for_first.get_or_init(|| Ok(Arc::new(2_000))) {
                        Ok(value) => Ok(Arc::new(*value + 1)),
                        Err(LazyInitError::CrossThreadCycleDetected) => {
                            cycles_for_first.fetch_add(1, Ordering::SeqCst);
                            Ok(Arc::new(1_000))
                        }
                        Err(_) => panic!("unexpected nested lazy init result"),
                    }
                })
                .map(|value| *value);
            result_tx_first.send(result).expect("publish first result");
        });

        let a_for_second = Arc::clone(&lazy_a);
        let b_for_second = Arc::clone(&lazy_b);
        let barrier_for_second = Arc::clone(&barrier);
        let cycles_for_second = Arc::clone(&cycle_count);
        let result_tx_second = result_tx;
        let second = thread::spawn(move || {
            let result = b_for_second
                .get_or_init(|| {
                    barrier_for_second.wait();
                    match a_for_second.get_or_init(|| Ok(Arc::new(20))) {
                        Ok(value) => Ok(Arc::new(*value + 10)),
                        Err(LazyInitError::CrossThreadCycleDetected) => {
                            cycles_for_second.fetch_add(1, Ordering::SeqCst);
                            Ok(Arc::new(2_000))
                        }
                        Err(_) => panic!("unexpected nested lazy init result"),
                    }
                })
                .map(|value| *value);
            result_tx_second
                .send(result)
                .expect("publish second result");
        });

        let first_result = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first thread should not deadlock")
            .expect("first initializer should recover");
        let second_result = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second thread should not deadlock")
            .expect("second initializer should recover");

        first.join().expect("join first thread");
        second.join().expect("join second thread");

        assert_eq!(
            cycle_count.load(Ordering::SeqCst),
            1,
            "exactly one thread should observe the cycle and fail fast",
        );
        assert!(
            [first_result, second_result]
                .iter()
                .any(|value| matches!(*value, 1_000 | 2_000)),
            "one initializer should fall back to its cycle-detected sentinel",
        );
        assert!(
            [first_result, second_result]
                .iter()
                .any(|value| matches!(*value, 2_001 | 1_010)),
            "the sibling initializer should complete after the cycle breaks",
        );
    }

    #[test]
    fn three_thread_cycle_is_rejected_instead_of_deadlocking() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();
        let lazy_a = Arc::new(LazyValue::<u32, &'static str>::new());
        let lazy_b = Arc::new(LazyValue::<u32, &'static str>::new());
        let lazy_c = Arc::new(LazyValue::<u32, &'static str>::new());
        let barrier = Arc::new(Barrier::new(3));
        let cycle_count = Arc::new(AtomicUsize::new(0));
        let (result_tx, result_rx) = mpsc::channel();

        let a_for_first = Arc::clone(&lazy_a);
        let b_for_first = Arc::clone(&lazy_b);
        let barrier_for_first = Arc::clone(&barrier);
        let cycles_for_first = Arc::clone(&cycle_count);
        let result_tx_first = result_tx.clone();
        let first = thread::spawn(move || {
            let result = a_for_first
                .get_or_init(|| {
                    barrier_for_first.wait();
                    match b_for_first.get_or_init(|| Ok(Arc::new(2_000))) {
                        Ok(value) => Ok(Arc::new(*value + 1)),
                        Err(LazyInitError::CrossThreadCycleDetected) => {
                            cycles_for_first.fetch_add(1, Ordering::SeqCst);
                            Ok(Arc::new(1_000))
                        }
                        Err(_) => panic!("unexpected nested lazy init result"),
                    }
                })
                .map(|value| *value);
            result_tx_first.send(result).expect("publish first result");
        });

        let b_for_second = Arc::clone(&lazy_b);
        let c_for_second = Arc::clone(&lazy_c);
        let barrier_for_second = Arc::clone(&barrier);
        let cycles_for_second = Arc::clone(&cycle_count);
        let result_tx_second = result_tx.clone();
        let second = thread::spawn(move || {
            let result = b_for_second
                .get_or_init(|| {
                    barrier_for_second.wait();
                    match c_for_second.get_or_init(|| Ok(Arc::new(3_000))) {
                        Ok(value) => Ok(Arc::new(*value + 10)),
                        Err(LazyInitError::CrossThreadCycleDetected) => {
                            cycles_for_second.fetch_add(1, Ordering::SeqCst);
                            Ok(Arc::new(2_000))
                        }
                        Err(_) => panic!("unexpected nested lazy init result"),
                    }
                })
                .map(|value| *value);
            result_tx_second
                .send(result)
                .expect("publish second result");
        });

        let c_for_third = Arc::clone(&lazy_c);
        let a_for_third = Arc::clone(&lazy_a);
        let barrier_for_third = Arc::clone(&barrier);
        let cycles_for_third = Arc::clone(&cycle_count);
        let result_tx_third = result_tx;
        let third = thread::spawn(move || {
            let result = c_for_third
                .get_or_init(|| {
                    barrier_for_third.wait();
                    match a_for_third.get_or_init(|| Ok(Arc::new(20))) {
                        Ok(value) => Ok(Arc::new(*value + 100)),
                        Err(LazyInitError::CrossThreadCycleDetected) => {
                            cycles_for_third.fetch_add(1, Ordering::SeqCst);
                            Ok(Arc::new(3_000))
                        }
                        Err(_) => panic!("unexpected nested lazy init result"),
                    }
                })
                .map(|value| *value);
            result_tx_third.send(result).expect("publish third result");
        });

        let first_result = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first thread should not deadlock")
            .expect("first initializer should recover");
        let second_result = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second thread should not deadlock")
            .expect("second initializer should recover");
        let third_result = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("third thread should not deadlock")
            .expect("third initializer should recover");

        first.join().expect("join first thread");
        second.join().expect("join second thread");
        third.join().expect("join third thread");

        assert_eq!(
            cycle_count.load(Ordering::SeqCst),
            1,
            "exactly one thread should observe the cycle and fail fast",
        );
        assert!(
            [first_result, second_result, third_result]
                .iter()
                .any(|value| matches!(*value, 1_000 | 2_000 | 3_000)),
            "one initializer should fall back to its cycle-detected sentinel",
        );
        assert!(
            [first_result, second_result, third_result]
                .iter()
                .filter(|value| matches!(**value, 1_000 | 2_000 | 3_000))
                .count()
                == 1,
            "only one initializer should consume the cycle-detected sentinel path",
        );
        assert!(
            [first_result, second_result, third_result]
                .iter()
                .any(|value| !matches!(*value, 1_000 | 2_000 | 3_000)),
            "the remaining initializers should complete after the cycle breaks",
        );
    }

    #[test]
    fn set_clears_wait_graph_owner_after_replacing_inflight_initialization() {
        let _guard = lazy_value_test_lock()
            .lock()
            .expect("lazy value test mutex poisoned");
        reset_lazy_wait_graph_for_test();
        let lazy = Arc::new(LazyValue::<u32, &'static str>::new());
        let blocker = Arc::new(LazyValue::<u32, &'static str>::new());
        let lazy_id = lazy.id();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let lazy_for_init = Arc::clone(&lazy);
        let blocker_for_init = Arc::clone(&blocker);
        let init_thread = thread::spawn(move || {
            let result = lazy_for_init.get_or_init(|| {
                entered_tx.send(()).expect("signal initializer entered");
                release_rx.recv().expect("release initializer");
                let blocker_value = blocker_for_init
                    .get_or_init(|| Ok(Arc::new(9)))
                    .expect("replacement path should still allow unrelated lazy init");
                Ok(Arc::new(*blocker_value + 1))
            });
            assert_eq!(
                *result.expect("initializer should observe replacement state"),
                7,
                "replacement value should remain visible",
            );
        });

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("initializer should start");
        lazy.set(Arc::new(7));
        release_tx.send(()).expect("release initializer");
        init_thread.join().expect("join initializer thread");

        let wait_graph = lock_unpoisoned(lazy_wait_graph());
        assert!(
            !wait_graph.owners.contains_key(&lazy_id),
            "replacement should clear stale owner records",
        );
        assert!(
            wait_graph.threads.is_empty(),
            "replacement and initializer exit should leave no stale thread wait state: {wait_graph:?}",
        );
    }
}
