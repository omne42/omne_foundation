use futures_util::FutureExt;
use std::future::Future;
use std::pin::Pin;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, mpsc as std_mpsc};

type DetachedTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

struct DetachedRuntime {
    worker_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<DetachedTask>>>,
}

impl DetachedRuntime {
    fn try_spawn(
        &self,
        task_name: &str,
        task: DetachedTask,
    ) -> std::result::Result<(), DetachedSpawnError> {
        let mut pending_task = Some(task);
        let mut last_error = None;

        for _ in 0..2 {
            let tx = {
                let mut worker_tx = self
                    .worker_tx
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if worker_tx.is_none() {
                    match spawn_detached_runtime_worker(task_name) {
                        Ok(tx) => *worker_tx = Some(tx),
                        Err(err) => {
                            last_error = Some(err);
                            *worker_tx = None;
                        }
                    }
                }
                worker_tx.clone()
            };

            let Some(tx) = tx else {
                continue;
            };

            let task = pending_task
                .take()
                .expect("detached runtime task should still be available");
            match tx.send(task) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    pending_task = Some(err.0);
                    last_error = Some(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        format!("detached runtime worker stopped before accepting {task_name}"),
                    ));
                    let mut worker_tx = self
                        .worker_tx
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *worker_tx = None;
                }
            }
        }

        Err(DetachedSpawnError {
            task: pending_task.expect("detached runtime task should still be available"),
            source: last_error.unwrap_or_else(|| {
                std::io::Error::other(format!("detached runtime unavailable for {task_name}"))
            }),
        })
    }
}

struct DetachedSpawnError {
    task: DetachedTask,
    source: std::io::Error,
}

#[cfg(test)]
static FORCE_SHARED_WORKER_SPAWN_FAILURES: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static FORCE_INLINE_RUNTIME_BUILD_FAILURES: AtomicUsize = AtomicUsize::new(0);

pub(super) fn spawn_detached(
    task_name: &str,
    task: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        drop(runtime.spawn(task));
        return Ok(());
    }

    match detached_runtime().try_spawn(task_name, Box::pin(task)) {
        Ok(()) => Ok(()),
        Err(err) => spawn_fallback_detached_task(task_name, err.task).map_err(|fallback_err| {
            std::io::Error::new(
                fallback_err.kind(),
                format!(
                    "schedule detached task failed for {task_name}: shared worker unavailable ({source}); fallback runtime unavailable ({fallback_err})",
                    source = err.source
                ),
            )
        }),
    }
}

fn detached_runtime() -> &'static DetachedRuntime {
    static DETACHED_RUNTIME: OnceLock<DetachedRuntime> = OnceLock::new();
    DETACHED_RUNTIME.get_or_init(|| DetachedRuntime {
        worker_tx: Mutex::new(None),
    })
}

fn spawn_detached_runtime_worker(
    task_name: &str,
) -> std::io::Result<tokio::sync::mpsc::UnboundedSender<DetachedTask>> {
    #[cfg(test)]
    if FORCE_SHARED_WORKER_SPAWN_FAILURES
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
    {
        return Err(std::io::Error::other(format!(
            "injected detached mcp-jsonrpc runtime spawn failure ({task_name})"
        )));
    }

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DetachedTask>();
    let (ready_tx, ready_rx) = std_mpsc::channel::<std::io::Result<()>>();
    let spawn_result = std::thread::Builder::new()
        .name("mcp-jsonrpc-detached".to_string())
        .spawn(move || run_detached_runtime_worker(rx, ready_tx));
    let _worker = spawn_result.map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!("spawn detached mcp-jsonrpc runtime ({task_name}): {err}"),
        )
    })?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(tx),
        Ok(Err(err)) => Err(std::io::Error::new(
            err.kind(),
            format!("build detached mcp-jsonrpc runtime ({task_name}): {err}"),
        )),
        Err(_) => Err(std::io::Error::other(format!(
            "detached mcp-jsonrpc runtime worker exited before initialization ({task_name})"
        ))),
    }
}

fn run_detached_runtime_worker(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<DetachedTask>,
    ready_tx: std_mpsc::Sender<std::io::Result<()>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = ready_tx.send(Err(err));
            return;
        }
    };
    let _ = ready_tx.send(Ok(()));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async move {
            while let Some(task) = rx.recv().await {
                drop(tokio::spawn(async move {
                    let _ = std::panic::AssertUnwindSafe(task).catch_unwind().await;
                }));
            }
        });
    }));
}

fn spawn_fallback_detached_task(task_name: &str, task: DetachedTask) -> std::io::Result<()> {
    #[cfg(test)]
    if FORCE_INLINE_RUNTIME_BUILD_FAILURES
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
    {
        return Err(std::io::Error::other(
            "injected detached mcp-jsonrpc inline runtime build failure",
        ));
    }

    let (ready_tx, ready_rx) = std_mpsc::channel::<std::io::Result<()>>();
    let spawn_result = std::thread::Builder::new()
        .name("mcp-jsonrpc-detached-fallback".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = ready_tx.send(Err(err));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(task)));
        });
    let _task = spawn_result.map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!("spawn detached fallback runtime ({task_name}): {err}"),
        )
    })?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(std::io::Error::new(
            err.kind(),
            format!("build detached fallback runtime ({task_name}): {err}"),
        )),
        Err(_) => Err(std::io::Error::other(format!(
            "detached fallback runtime worker exited before initialization ({task_name})"
        ))),
    }
}

#[cfg(test)]
pub(super) fn reset_detached_runtime_for_test() {
    let runtime = detached_runtime();
    *runtime
        .worker_tx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    FORCE_SHARED_WORKER_SPAWN_FAILURES.store(0, Ordering::Relaxed);
    FORCE_INLINE_RUNTIME_BUILD_FAILURES.store(0, Ordering::Relaxed);
}

#[cfg(test)]
pub(super) fn force_detached_runtime_spawn_failures(shared_worker: usize, inline_runtime: usize) {
    FORCE_SHARED_WORKER_SPAWN_FAILURES.store(shared_worker, Ordering::Relaxed);
    FORCE_INLINE_RUNTIME_BUILD_FAILURES.store(inline_runtime, Ordering::Relaxed);
}
