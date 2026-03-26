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

    pub(crate) fn set(&self, value: Arc<T>) {
        *lock_unpoisoned(&self.state) = LazyState::Initialized(value);
        self.ready.notify_all();
    }

    /// Returns the initialized value, running `initializer` at most once per
    /// successful initialization attempt.
    ///
    /// Same-thread recursive initialization is rejected explicitly. Callers
    /// must also avoid cross-thread or cross-task cycles where the initializer
    /// waits for work that re-enters the same `LazyValue`; that pattern cannot
    /// be detected here and may deadlock.
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
            }
                if *thread_id == self.thread_id
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Duration;

    fn is_initialized<T: ?Sized, E>(lazy: &LazyValue<T, E>) -> bool {
        matches!(&*lock_unpoisoned(&lazy.state), LazyState::Initialized(_))
    }

    #[test]
    fn concurrent_access_waits_for_initialization_to_finish() {
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
                });
            result_tx.send(result).expect("publish waiter result");
        });

        assert!(
            result_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "waiter should wait for initialization to complete",
        );

        release_tx.send(()).expect("release initializer");
        let init_error = handle
            .join()
            .expect("join initializer thread")
            .expect_err("initializer should fail");
        assert!(matches!(init_error, LazyInitError::Inner(error) if *error == "init failed"));
        waiting_handle.join().expect("join waiting thread");
        assert_eq!(
            result_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("waiter should observe the failure"),
            Err("init failed"),
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        let value = lazy
            .get_or_init(|| -> Result<Arc<u32>, &'static str> { Ok(Arc::new(7)) })
            .expect("next call should retry after the shared failure");
        assert_eq!(*value, 7);
    }

    #[test]
    fn retry_in_progress_remains_uninitialized_until_success() {
        let lazy = Arc::new(LazyValue::<u32, &'static str>::new());

        let error = lazy
            .get_or_init(|| Err("init failed"))
            .expect_err("initialization should fail");
        assert!(matches!(error, LazyInitError::Inner(_)));

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let retrying = Arc::clone(&lazy);
        let handle = thread::spawn(move || {
            retrying
                .get_or_init(|| -> Result<Arc<u32>, &'static str> {
                    entered_tx.send(()).expect("signal retry entered");
                    release_rx.recv().expect("release retry");
                    Ok(Arc::new(11))
                })
                .expect("retry should succeed")
        });

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("retry should start");

        assert!(!is_initialized(&lazy));

        release_tx.send(()).expect("release retry");
        let value = handle.join().expect("join retry thread");
        assert_eq!(*value, 11);
        assert!(is_initialized(&lazy));
    }

    #[test]
    fn panicked_retry_leaves_value_uninitialized() {
        let lazy = LazyValue::<u32, &'static str>::new();

        let error = lazy
            .get_or_init(|| Err("init failed"))
            .expect_err("initialization should fail");
        assert!(matches!(error, LazyInitError::Inner(_)));

        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _ = lazy
                .get_or_init(|| -> Result<Arc<u32>, &'static str> { panic!("initializer panic") });
        }));
        assert!(panic.is_err());

        assert!(!is_initialized(&lazy));
    }

    #[test]
    fn set_installs_initialized_value_without_running_initializer() {
        let lazy = LazyValue::<u32, &'static str>::new();
        lazy.set(Arc::new(7));

        let value = lazy
            .get_or_init(|| -> Result<Arc<u32>, &'static str> {
                panic!("initializer should not run after set")
            })
            .expect("set value should be returned");

        assert_eq!(*value, 7);
        assert!(is_initialized(&lazy));
    }
}
