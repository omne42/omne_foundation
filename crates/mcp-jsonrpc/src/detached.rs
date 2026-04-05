use std::future::Future;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::OnceLock;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use super::{ClientHandle, CloseReasonPriority, Error, ProtocolErrorKind, drain_pending};

const DETACHED_RUNTIME_WORKER_THREADS: usize = 2;

enum DetachedRuntime {
    Ready(tokio::runtime::Handle),
    Unavailable(String),
}

#[derive(Debug, Clone)]
pub(super) struct DetachedSpawnError {
    task_name: String,
    message: String,
}

impl DetachedSpawnError {
    fn new(task_name: &str, message: impl Into<String>) -> Self {
        Self {
            task_name: task_name.to_string(),
            message: message.into(),
        }
    }

    pub(super) fn close_reason(&self) -> String {
        format!(
            "detached runtime unavailable for {}: {}",
            self.task_name, self.message
        )
    }
}

impl std::fmt::Display for DetachedSpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.close_reason().fmt(f)
    }
}

impl DetachedRuntime {
    fn spawn(
        &self,
        task_name: &str,
        task: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), DetachedSpawnError> {
        #[cfg(test)]
        if FORCED_DETACHED_SPAWN_FAILURES
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(DetachedSpawnError::new(
                task_name,
                "injected detached runtime spawn failure",
            ));
        }

        match self {
            Self::Ready(handle) => {
                drop(handle.spawn(task));
                Ok(())
            }
            Self::Unavailable(message) => Err(DetachedSpawnError::new(task_name, message.clone())),
        }
    }
}

pub(super) fn spawn_detached(
    task_name: &str,
    task: impl Future<Output = ()> + Send + 'static,
) -> Result<(), DetachedSpawnError> {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        drop(runtime.spawn(task));
        return Ok(());
    }

    #[cfg(test)]
    if FORCED_DETACHED_INIT_FAILURES
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
    {
        return Err(DetachedSpawnError::new(
            task_name,
            "injected detached runtime init failure",
        ));
    }

    detached_runtime().spawn(task_name, task)
}

pub(super) fn close_without_runtime(handle: &ClientHandle, reason: String) {
    handle
        .close_reason
        .publish(CloseReasonPriority::Primary, reason.clone());
    handle.closed.store(true, Ordering::Relaxed);
    let err = Error::protocol(ProtocolErrorKind::Closed, reason);
    drain_pending(&handle.pending, &err);
    if let Ok(mut write) = handle.write.try_lock() {
        drop(std::mem::replace(&mut *write, Box::new(tokio::io::sink())));
    }
}

fn detached_runtime() -> &'static DetachedRuntime {
    static DETACHED_RUNTIME: OnceLock<DetachedRuntime> = OnceLock::new();

    DETACHED_RUNTIME.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("mcp-jsonrpc-detached".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(DETACHED_RUNTIME_WORKER_THREADS)
                    .thread_name("mcp-jsonrpc-detached-worker")
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = tx.send(Err(format!("build detached runtime: {err}")));
                        return;
                    }
                };

                let handle = runtime.handle().clone();
                if tx.send(Ok(handle)).is_err() {
                    return;
                }

                runtime.block_on(std::future::pending::<()>());
            });

        match thread {
            Ok(_join_handle) => match rx.recv() {
                Ok(Ok(handle)) => DetachedRuntime::Ready(handle),
                Ok(Err(message)) => DetachedRuntime::Unavailable(message),
                Err(err) => {
                    DetachedRuntime::Unavailable(format!("receive detached runtime handle: {err}"))
                }
            },
            Err(err) => {
                DetachedRuntime::Unavailable(format!("spawn detached runtime thread: {err}"))
            }
        }
    })
}

#[cfg(test)]
static FORCED_DETACHED_SPAWN_FAILURES: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
static FORCED_DETACHED_INIT_FAILURES: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub(super) fn force_detached_spawn_failures(count: usize) {
    FORCED_DETACHED_SPAWN_FAILURES.store(count, Ordering::Relaxed);
}

#[cfg(test)]
pub(super) fn force_detached_init_failures(count: usize) {
    FORCED_DETACHED_INIT_FAILURES.store(count, Ordering::Relaxed);
}

#[cfg(test)]
pub(super) fn detached_runtime_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = GUARD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    force_detached_init_failures(0);
    force_detached_spawn_failures(0);
    guard
}
