use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::ThreadId;

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
pub(super) enum BlockingLazyInitError<E> {
    Inner(Arc<E>),
    ReentrantInitialization,
    SameThreadInitializationConflict,
    CrossThreadCycleDetected,
}

pub(super) struct BlockingLazyValue<T: ?Sized, E> {
    id: OnceLock<u64>,
    state: Mutex<LazyState<T, E>>,
    ready: Condvar,
}

impl<T: ?Sized, E> BlockingLazyValue<T, E> {
    pub(super) const fn new() -> Self {
        Self {
            id: OnceLock::new(),
            state: Mutex::new(LazyState::Uninitialized),
            ready: Condvar::new(),
        }
    }

    pub(super) fn set(&self, value: Arc<T>) {
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

    pub(super) fn get_or_init(
        &self,
        initializer: impl FnOnce() -> Result<Arc<T>, E>,
    ) -> Result<Arc<T>, BlockingLazyInitError<E>> {
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
                        return Err(BlockingLazyInitError::ReentrantInitialization);
                    }

                    if *owner_thread_id == thread_id {
                        return Err(BlockingLazyInitError::SameThreadInitializationConflict);
                    }

                    if !waiting_for_current_attempt {
                        if !begin_lazy_wait(thread_id, lazy_id) {
                            return Err(BlockingLazyInitError::CrossThreadCycleDetected);
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
                        return Err(BlockingLazyInitError::Inner(error));
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
                                Err(BlockingLazyInitError::Inner(error))
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

    fn id(&self) -> u64 {
        *self.id.get_or_init(next_lazy_id)
    }
}

struct InitializationGuard<'a, T: ?Sized, E> {
    value: &'a BlockingLazyValue<T, E>,
    thread_id: ThreadId,
    lazy_id: u64,
    active: bool,
}

impl<'a, T: ?Sized, E> InitializationGuard<'a, T, E> {
    fn new(value: &'a BlockingLazyValue<T, E>, thread_id: ThreadId, lazy_id: u64) -> Self {
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
