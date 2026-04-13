use std::time::Duration;

use tokio::io::{AsyncWrite, AsyncWriteExt};

pub(crate) async fn lock_with_timeout<'a, T>(
    mutex: &'a tokio::sync::Mutex<T>,
    timeout: Duration,
) -> Option<tokio::sync::MutexGuard<'a, T>> {
    if let Ok(guard) = mutex.try_lock() {
        return Some(guard);
    }

    tokio::time::timeout(timeout, mutex.lock()).await.ok()
}

pub(crate) fn lock_with_timeout_without_runtime<'a, T>(
    mutex: &'a tokio::sync::Mutex<T>,
    timeout: Duration,
) -> Option<tokio::sync::MutexGuard<'a, T>> {
    if let Ok(guard) = mutex.try_lock() {
        return Some(guard);
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .ok()?;
    runtime.block_on(async { tokio::time::timeout(timeout, mutex.lock()).await.ok() })
}

pub(crate) fn replace_write_with_sink(
    write: &mut tokio::sync::MutexGuard<'_, Box<dyn AsyncWrite + Send + Unpin>>,
) {
    let _ = futures_util::FutureExt::now_or_never(write.shutdown());
    // Many `AsyncWrite` impls (e.g. `tokio::process::ChildStdin`) only fully close on drop.
    // Replacing the writer guarantees the underlying write end is closed.
    let _ = std::mem::replace(&mut **write, Box::new(tokio::io::sink()));
}

#[cfg(test)]
mod tests {
    use super::{lock_with_timeout, lock_with_timeout_without_runtime};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn lock_with_timeout_times_out_without_spinning_forever() {
        let mutex = Arc::new(tokio::sync::Mutex::new(()));
        let guard = mutex.lock().await;

        let start = Instant::now();
        let acquired = lock_with_timeout(&mutex, Duration::from_millis(20)).await;
        let elapsed = start.elapsed();

        assert!(acquired.is_none(), "lock acquisition should time out");
        assert!(
            elapsed >= Duration::from_millis(20),
            "lock wait should honor timeout lower bound, got {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "lock wait should remain bounded, got {elapsed:?}"
        );

        drop(guard);
    }

    #[test]
    fn lock_with_timeout_without_runtime_times_out_without_spinning_forever() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let mutex = Arc::new(tokio::sync::Mutex::new(()));
        let guard = runtime.block_on(mutex.lock());
        drop(runtime);

        let start = Instant::now();
        let acquired = lock_with_timeout_without_runtime(&mutex, Duration::from_millis(20));
        let elapsed = start.elapsed();

        assert!(acquired.is_none(), "lock acquisition should time out");
        assert!(
            elapsed >= Duration::from_millis(20),
            "lock wait should honor timeout lower bound, got {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "lock wait should remain bounded, got {elapsed:?}"
        );

        drop(guard);
    }
}
