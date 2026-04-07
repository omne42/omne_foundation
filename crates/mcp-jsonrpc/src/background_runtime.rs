use futures_util::FutureExt;
use std::future::Future;
use std::pin::Pin;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};

pub(super) type DetachedTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

struct DetachedTaskEnvelope {
    task: Arc<Mutex<Option<DetachedTask>>>,
    started_tx: std_mpsc::Sender<()>,
}

pub(super) struct DetachedSpawner {
    worker_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<DetachedTaskEnvelope>>>,
}

impl DetachedSpawner {
    pub(super) fn new() -> Self {
        Self {
            worker_tx: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(super) fn spawn(
        &self,
        task_name: &str,
        task: impl Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn(task));
            return Ok(());
        }

        self.spawn_boxed(task_name, Box::pin(task))
    }

    pub(super) fn spawn_boxed(&self, task_name: &str, task: DetachedTask) -> std::io::Result<()> {
        match self.try_spawn(task_name, task) {
            Ok(()) => Ok(()),
            Err(err) => {
                let Some(task) = err.task else {
                    return Err(std::io::Error::new(
                        err.source.kind(),
                        format!(
                            "schedule detached task failed for {task_name}: shared worker lost the task before fallback could reclaim it ({})",
                            err.source
                        ),
                    ));
                };
                spawn_fallback_detached_task(task_name, task).map_err(|fallback_err| {
                    std::io::Error::new(
                        fallback_err.kind(),
                        format!(
                            "schedule detached task failed for {task_name}: shared worker unavailable ({source}); fallback runtime unavailable ({fallback_err})",
                            source = err.source
                        ),
                    )
                })
            }
        }
    }

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
            let shared_task = Arc::new(Mutex::new(Some(task)));
            let (started_tx, started_rx) = std_mpsc::channel();
            let envelope = DetachedTaskEnvelope {
                task: Arc::clone(&shared_task),
                started_tx,
            };

            match tx.send(envelope) {
                Ok(()) => match started_rx.recv() {
                    Ok(()) => return Ok(()),
                    Err(_) => {
                        pending_task = shared_task
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take();
                        last_error = Some(std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            format!("detached runtime worker stopped before starting {task_name}"),
                        ));
                        let mut worker_tx = self
                            .worker_tx
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        *worker_tx = None;
                        if pending_task.is_none() {
                            return Err(DetachedSpawnError {
                                task: None,
                                source: last_error
                                    .take()
                                    .expect("last_error should be set for lost worker"),
                            });
                        }
                    }
                },
                Err(err) => {
                    pending_task = err
                        .0
                        .task
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
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
            task: pending_task,
            source: last_error.unwrap_or_else(|| {
                std::io::Error::other(format!("detached runtime unavailable for {task_name}"))
            }),
        })
    }
}

struct DetachedSpawnError {
    task: Option<DetachedTask>,
    source: std::io::Error,
}

impl Default for DetachedSpawner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
static FORCE_SHARED_WORKER_SPAWN_FAILURES: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static FORCE_SHARED_WORKER_RUNTIME_BUILD_FAILURES: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static FORCE_INLINE_RUNTIME_BUILD_FAILURES: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static FORCE_SHARED_WORKER_DROP_BEFORE_START: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static SHARED_WORKER_SPAWN_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(super) fn spawn_fallback_detached(task_name: &str, task: DetachedTask) -> std::io::Result<()> {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        drop(runtime.spawn(task));
        return Ok(());
    }

    spawn_fallback_detached_task(task_name, task)
}

fn spawn_detached_runtime_worker(
    task_name: &str,
) -> std::io::Result<tokio::sync::mpsc::UnboundedSender<DetachedTaskEnvelope>> {
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

    #[cfg(test)]
    SHARED_WORKER_SPAWN_COUNT.fetch_add(1, Ordering::Relaxed);

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DetachedTaskEnvelope>();
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
    mut rx: tokio::sync::mpsc::UnboundedReceiver<DetachedTaskEnvelope>,
    ready_tx: std_mpsc::Sender<std::io::Result<()>>,
) {
    #[cfg(test)]
    if FORCE_SHARED_WORKER_RUNTIME_BUILD_FAILURES
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
    {
        let _ = ready_tx.send(Err(std::io::Error::other(
            "injected detached mcp-jsonrpc shared worker runtime build failure",
        )));
        return;
    }

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
            while let Some(envelope) = rx.recv().await {
                #[cfg(test)]
                if FORCE_SHARED_WORKER_DROP_BEFORE_START
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok()
                {
                    return;
                }

                let Some(task) = envelope
                    .task
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                else {
                    let _ = envelope.started_tx.send(());
                    continue;
                };

                drop(tokio::spawn(async move {
                    let _ = std::panic::AssertUnwindSafe(task).catch_unwind().await;
                }));
                let _ = envelope.started_tx.send(());
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
pub(super) fn reset_detached_runtime_test_state() {
    FORCE_SHARED_WORKER_SPAWN_FAILURES.store(0, Ordering::Relaxed);
    FORCE_SHARED_WORKER_RUNTIME_BUILD_FAILURES.store(0, Ordering::Relaxed);
    FORCE_INLINE_RUNTIME_BUILD_FAILURES.store(0, Ordering::Relaxed);
    FORCE_SHARED_WORKER_DROP_BEFORE_START.store(0, Ordering::Relaxed);
    SHARED_WORKER_SPAWN_COUNT.store(0, Ordering::Relaxed);
}

#[cfg(test)]
impl DetachedSpawner {
    pub(super) fn reset_for_test(&self) {
        *self
            .worker_tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

#[cfg(test)]
pub(super) fn force_detached_runtime_spawn_failures(shared_worker: usize, inline_runtime: usize) {
    FORCE_SHARED_WORKER_SPAWN_FAILURES.store(shared_worker, Ordering::Relaxed);
    FORCE_INLINE_RUNTIME_BUILD_FAILURES.store(inline_runtime, Ordering::Relaxed);
}

#[cfg(test)]
pub(super) fn force_shared_worker_runtime_build_failures(count: usize) {
    FORCE_SHARED_WORKER_RUNTIME_BUILD_FAILURES.store(count, Ordering::Relaxed);
}

#[cfg(test)]
pub(super) fn force_shared_worker_drop_before_start(count: usize) {
    FORCE_SHARED_WORKER_DROP_BEFORE_START.store(count, Ordering::Relaxed);
}

#[cfg(test)]
pub(super) fn shared_worker_spawn_count() -> usize {
    SHARED_WORKER_SPAWN_COUNT.load(Ordering::Relaxed)
}
