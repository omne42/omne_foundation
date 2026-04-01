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

#[derive(Debug)]
pub enum LazyInitError<E> {
    Inner(Arc<E>),
    ReentrantInitialization,
    CrossThreadCycleDetected,
}

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
    /// Same-thread recursive initialization is rejected explicitly, and
    /// thread-level cross-thread wait cycles between tracked `LazyValue`
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
                LazyState::Initialized(value) => return Ok(Arc::clone(value)),
                LazyState::Initializing {
                    thread_id: owner_thread_id,
                    waiting_threads,
                } => {
                    if *owner_thread_id == thread_id {
                        return Err(LazyInitError::ReentrantInitialization);
                    }

                    if !waiting_for_current_attempt {
                        if !begin_lazy_wait(thread_id, lazy_id) {
                            return Err(LazyInitError::CrossThreadCycleDetected);
                        }
                        *waiting_threads += 1;
                        waiting_for_current_attempt = true;
                    }

                    guard = wait_unpoisoned(&self.ready, guard);
                    finish_lazy_wait(thread_id, lazy_id);
                    drop(guard);
                }
                LazyState::Failed {
                    error,
                    waiting_threads,
                } => {
                    if waiting_for_current_attempt {
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
        if let Some(state) = self.threads.get_mut(&thread_id) {
            if state.waiting_on == Some(lazy_id) {
                state.waiting_on = None;
            }
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
    lock_unpoisoned(lazy_wait_graph()).begin_initialization(thread_id, lazy_id);
}

fn finish_lazy_initialization(thread_id: ThreadId, lazy_id: u64) {
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

#[cfg(test)]
fn reset_lazy_wait_graph_for_test() {
    *lock_unpoisoned(lazy_wait_graph()) = LazyWaitGraph::default();
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
                    match b_for_first.get_or_init(|| Ok(Arc::new(200))) {
                        Ok(value) => Ok(Arc::new(*value + 1)),
                        Err(LazyInitError::CrossThreadCycleDetected) => {
                            cycles_for_first.fetch_add(1, Ordering::SeqCst);
                            Ok(Arc::new(100))
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
                            Ok(Arc::new(200))
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
                .any(|value| matches!(*value, 100 | 200)),
            "one initializer should fall back to its cycle-detected sentinel",
        );
        assert!(
            [first_result, second_result]
                .iter()
                .any(|value| matches!(*value, 110 | 201)),
            "the sibling initializer should complete after the cycle breaks",
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
