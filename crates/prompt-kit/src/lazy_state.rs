use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;

pub(crate) enum LazyState<T: ?Sized, E> {
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
pub(crate) enum LazyInitError<E> {
    Inner(Arc<E>),
    ReentrantInitialization,
}

pub(crate) struct LazyValue<T: ?Sized, E> {
    state: Mutex<LazyState<T, E>>,
    ready: Condvar,
}

impl<T: ?Sized, E> LazyValue<T, E> {
    pub(crate) const fn new() -> Self {
        Self {
            state: Mutex::new(LazyState::Uninitialized),
            ready: Condvar::new(),
        }
    }

    pub(crate) fn get_or_init(
        &self,
        initializer: impl FnOnce() -> Result<Arc<T>, E>,
    ) -> Result<Arc<T>, LazyInitError<E>> {
        let thread_id = std::thread::current().id();
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
                    waiting_for_current_attempt = false;
                    drop(guard);

                    let mut reset = InitializationGuard::new(self, thread_id);
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
                                Err(LazyInitError::Inner(error))
                            }
                        };
                        self.ready.notify_all();
                        reset.disarm();
                        return outcome;
                    }
                    reset.disarm();
                }
            }
        }
    }
}

struct InitializationGuard<'a, T: ?Sized, E> {
    value: &'a LazyValue<T, E>,
    thread_id: ThreadId,
    active: bool,
}

impl<'a, T: ?Sized, E> InitializationGuard<'a, T, E> {
    fn new(value: &'a LazyValue<T, E>, thread_id: ThreadId) -> Self {
        Self {
            value,
            thread_id,
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
            self.value.ready.notify_all();
        }
    }
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
