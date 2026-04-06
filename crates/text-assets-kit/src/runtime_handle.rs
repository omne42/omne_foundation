use std::sync::{Arc, RwLock};

/// Hot-swappable runtime handle for already-loaded shared state.
///
/// Readers clone the current `Arc<T>` behind a shared `RwLock` and then use the value after the
/// lock has been released. Replacements only contend on swapping the pointer, so slow readers do
/// not block hot reloads.
pub struct SharedRuntimeHandle<T: ?Sized> {
    inner: RwLock<Option<Arc<T>>>,
}

impl<T: ?Sized> SharedRuntimeHandle<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: RwLock::new(None),
        }
    }

    pub fn replace_shared(&self, value: Arc<T>) {
        *write_unpoisoned(&self.inner) = Some(value);
    }

    #[must_use]
    pub fn current(&self) -> Option<Arc<T>> {
        read_unpoisoned(&self.inner).as_ref().map(Arc::clone)
    }
}

impl<T> SharedRuntimeHandle<T> {
    pub fn replace(&self, value: T) {
        self.replace_shared(Arc::new(value));
    }
}

impl<T: ?Sized> Default for SharedRuntimeHandle<T> {
    fn default() -> Self {
        Self::new()
    }
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poison| poison.into_inner())
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests {
    use super::SharedRuntimeHandle;
    use std::sync::Arc;

    #[test]
    fn handle_starts_empty_and_can_swap_values() {
        let handle = SharedRuntimeHandle::new();
        assert!(handle.current().is_none());

        handle.replace(String::from("first"));
        assert_eq!(
            handle.current().as_deref().map(String::as_str),
            Some("first")
        );

        handle.replace_shared(Arc::new(String::from("second")));
        assert_eq!(
            handle.current().as_deref().map(String::as_str),
            Some("second")
        );
    }
}
