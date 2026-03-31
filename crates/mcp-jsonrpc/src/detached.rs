use std::future::Future;
use std::pin::Pin;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::OnceLock;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
type DetachedTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

struct DetachedRuntime {
    runtime: tokio::runtime::Runtime,
}

#[derive(Debug, Clone)]
pub(crate) struct DetachedSpawnError {
    task_name: String,
    detail: String,
}

impl DetachedSpawnError {
    fn from_io(task_name: &str, err: &std::io::Error) -> Self {
        Self {
            task_name: task_name.to_string(),
            detail: err.to_string(),
        }
    }
}

impl std::fmt::Display for DetachedSpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "detached fallback unavailable for {}: {}",
            self.task_name, self.detail
        )
    }
}

impl DetachedRuntime {
    fn new() -> Result<Self, std::io::Error> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("mcp-jsonrpc-detached")
            .build()?;
        Ok(Self { runtime })
    }

    fn spawn(&self, task: DetachedTask) {
        drop(self.runtime.spawn(task));
    }
}

pub(crate) fn spawn_detached(
    task_name: &str,
    task: impl Future<Output = ()> + Send + 'static,
) -> Result<(), DetachedSpawnError> {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        drop(runtime.spawn(task));
        return Ok(());
    }

    let runtime = detached_runtime(task_name)?;
    runtime.spawn(Box::pin(task));
    Ok(())
}

fn detached_runtime(task_name: &str) -> Result<&'static DetachedRuntime, DetachedSpawnError> {
    static DETACHED_RUNTIME: OnceLock<DetachedRuntime> = OnceLock::new();
    #[cfg(test)]
    {
        if DETACHED_RUNTIME_FORCED_INIT_FAILURES
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                if remaining > 0 {
                    Some(remaining - 1)
                } else {
                    None
                }
            })
            .is_ok()
        {
            return Err(DetachedSpawnError {
                task_name: task_name.to_string(),
                detail: "forced detached runtime init failure".to_string(),
            });
        }
    }

    if let Some(runtime) = DETACHED_RUNTIME.get() {
        return Ok(runtime);
    }

    let runtime =
        DetachedRuntime::new().map_err(|err| DetachedSpawnError::from_io(task_name, &err))?;
    Ok(DETACHED_RUNTIME.get_or_init(|| runtime))
}

#[cfg(test)]
static DETACHED_RUNTIME_FORCED_INIT_FAILURES: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
pub(crate) fn force_detached_runtime_init_failures(count: u64) {
    DETACHED_RUNTIME_FORCED_INIT_FAILURES.store(count, Ordering::Relaxed);
}

#[cfg(test)]
pub(crate) fn detached_runtime_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    GUARD.get_or_init(|| Mutex::new(())).lock().unwrap()
}
