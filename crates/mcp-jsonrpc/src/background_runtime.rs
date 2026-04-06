use std::future::Future;
use std::pin::Pin;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, mpsc as std_mpsc};

type DetachedTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

struct DetachedRuntime {
    worker_tx: Mutex<Option<std_mpsc::Sender<DetachedTask>>>,
}

impl DetachedRuntime {
    fn spawn(&self, task_name: &str, task: DetachedTask) {
        let task = match self.try_spawn(task_name, task) {
            Ok(()) => return,
            Err(task) => task,
        };
        spawn_detached_fallback(task_name, task);
    }

    fn try_spawn(
        &self,
        task_name: &str,
        task: DetachedTask,
    ) -> std::result::Result<(), DetachedTask> {
        let mut pending_task = Some(task);

        for _ in 0..2 {
            let tx = {
                let mut worker_tx = self
                    .worker_tx
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if worker_tx.is_none() {
                    *worker_tx = spawn_detached_runtime_worker(task_name).ok();
                }
                worker_tx.clone()
            };

            let Some(tx) = tx else {
                break;
            };

            let task = pending_task
                .take()
                .expect("detached runtime task should still be available");
            match tx.send(task) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    pending_task = Some(err.0);
                    let mut worker_tx = self
                        .worker_tx
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *worker_tx = None;
                }
            }
        }

        Err(pending_task.expect("detached runtime task should still be available"))
    }
}

#[cfg(test)]
static FORCE_SHARED_WORKER_SPAWN_FAILURES: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static FORCE_FALLBACK_WORKER_SPAWN_FAILURES: AtomicUsize = AtomicUsize::new(0);

pub(super) fn spawn_detached(task_name: &str, task: impl Future<Output = ()> + Send + 'static) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        drop(runtime.spawn(task));
        return;
    }

    detached_runtime().spawn(task_name, Box::pin(task));
}

fn detached_runtime() -> &'static DetachedRuntime {
    static DETACHED_RUNTIME: OnceLock<DetachedRuntime> = OnceLock::new();
    DETACHED_RUNTIME.get_or_init(|| DetachedRuntime {
        worker_tx: Mutex::new(None),
    })
}

fn spawn_detached_runtime_worker(
    task_name: &str,
) -> std::io::Result<std_mpsc::Sender<DetachedTask>> {
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

    let (tx, rx) = std_mpsc::channel::<DetachedTask>();
    std::thread::Builder::new()
        .name("mcp-jsonrpc-detached".to_string())
        .spawn(move || run_detached_runtime_worker(rx))
        .map(|_| tx)
        .map_err(|err| {
            std::io::Error::new(
                err.kind(),
                format!("spawn detached mcp-jsonrpc runtime ({task_name}): {err}"),
            )
        })
}

fn run_detached_runtime_worker(rx: std_mpsc::Receiver<DetachedTask>) {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => {
            while let Ok(task) = rx.recv() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(task)
                }));
            }
        }
        Err(_) => {
            while let Ok(task) = rx.recv() {
                let _ = run_detached_task(task);
            }
        }
    }
}

fn spawn_detached_fallback(task_name: &str, task: DetachedTask) {
    #[cfg(test)]
    if FORCE_FALLBACK_WORKER_SPAWN_FAILURES
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
    {
        let _ = run_detached_task(task);
        return;
    }

    let task = std::sync::Arc::new(Mutex::new(Some(task)));
    let spawned = std::thread::Builder::new()
        .name("mcp-jsonrpc-detached-fallback".to_string())
        .spawn({
            let task = std::sync::Arc::clone(&task);
            move || {
                if let Some(task) = task
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    let _ = run_detached_task(task);
                }
            }
        });

    if spawned.is_err() {
        let mut task = task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(task) = task.take() {
            let _ = run_detached_task(task);
        }
        let _ = task_name;
    }
}

fn run_detached_task(task: DetachedTask) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(task)));
    Ok(())
}

#[cfg(test)]
pub(super) fn reset_detached_runtime_for_test() {
    let runtime = detached_runtime();
    *runtime
        .worker_tx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    FORCE_SHARED_WORKER_SPAWN_FAILURES.store(0, Ordering::Relaxed);
    FORCE_FALLBACK_WORKER_SPAWN_FAILURES.store(0, Ordering::Relaxed);
}

#[cfg(test)]
pub(super) fn force_detached_runtime_spawn_failures(shared_worker: usize, fallback_worker: usize) {
    FORCE_SHARED_WORKER_SPAWN_FAILURES.store(shared_worker, Ordering::Relaxed);
    FORCE_FALLBACK_WORKER_SPAWN_FAILURES.store(fallback_worker, Ordering::Relaxed);
}
