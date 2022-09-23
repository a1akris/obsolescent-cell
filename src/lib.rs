//! A type that represents a value that can be considered valid for
//! a certain amount of time. It's mainly useful for implementing caches.
//!

// TODO: Use Reentrant mutexes + RefCell to deal with reentrancy deadlock issue

use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::Mutex as TokioMutex;

/// Enum representing status of value inside the cell
#[derive(Debug)]
pub enum ObsolescentValue<T> {
    Absent,
    Obsolete(T),
    Fresh(T),
}

#[derive(Debug)]
pub struct ObsolescentCell<T> {
    inner: Mutex<Option<Inner<T>>>,
    time_to_live: Duration,
}

impl<T> ObsolescentCell<T> {
    /// time_to_live defines how long cell is considered valid
    pub fn new(time_to_live: Duration) -> Self {
        Self {
            inner: Mutex::new(None),
            time_to_live,
        }
    }

    pub fn set(&self, value: T) {
        let mut guard = self.inner.lock().unwrap();

        *guard = Some(Inner {
            value,
            last_modified: Instant::now(),
        });
    }
}

impl<T: Clone> ObsolescentCell<T> {
    pub fn get(&self) -> ObsolescentValue<T> {
        let guard = self.inner.lock().unwrap();

        guard
            .as_ref()
            .map(|inner| {
                if inner.duration_unmodified() > self.time_to_live {
                    ObsolescentValue::Obsolete(inner.value.clone())
                } else {
                    ObsolescentValue::Fresh(inner.value.clone())
                }
            })
            .unwrap_or(ObsolescentValue::Absent)
    }

    /// Get or update value atomically.
    /// In async contexts you want be able to provide async updater and using get(),
    /// <some_async_calL>, set() call sequence will introduce race conditions, so use
    /// ObsolescentCellAsync instead
    ///
    /// Warning: be careful to not use the same cell in the updater closure, or you will get a
    /// deadlock
    #[allow(clippy::significant_drop_in_scrutinee)]
    pub fn get_or_refresh<F, E>(&self, updater: F) -> Result<T, E>
    where
        F: FnOnce() -> Result<T, E>,
    {
        let now = Instant::now();
        let mut guard = self.inner.lock().unwrap();

        let inner = match guard
            .take()
            .filter(|inner| inner.duration_unmodified_since(now) < self.time_to_live)
        {
            Some(inner) => inner,
            None => Inner {
                value: updater()?,
                last_modified: now,
            },
        };

        let ret = inner.value.clone();
        *guard = Some(inner);

        Ok(ret)
    }
}

/// This type provides atomic get_or_refresh for async contexts
/// It's not recommended to use its set and get methods
/// They're provided to match ObsolescentCell API
#[derive(Debug)]
pub struct ObsolescentCellAsync<T> {
    inner: TokioMutex<Option<Inner<T>>>,
    time_to_live: Duration,
}

impl<T> ObsolescentCellAsync<T> {
    pub fn new(time_to_live: Duration) -> Self {
        Self {
            inner: TokioMutex::new(None),
            time_to_live,
        }
    }

    pub async fn set(&self, value: T) {
        let mut guard = self.inner.lock().await;

        *guard = Some(Inner {
            value,
            last_modified: Instant::now(),
        });
    }
}

impl<T: Clone> ObsolescentCellAsync<T> {
    pub async fn get(&self) -> ObsolescentValue<T> {
        let guard = self.inner.lock().await;

        guard
            .as_ref()
            .map(|inner| {
                if inner.duration_unmodified() > self.time_to_live {
                    ObsolescentValue::Obsolete(inner.value.clone())
                } else {
                    ObsolescentValue::Fresh(inner.value.clone())
                }
            })
            .unwrap_or(ObsolescentValue::Absent)
    }

    /// Provides atomic get_or_refresh method for async contexts
    /// Warning: be careful to not use the same cell in the updater closure, or you will get a
    /// deadlock
    pub async fn get_or_refresh<E>(
        &self,
        refresher: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let mut guard = self.inner.lock().await;

        let now = Instant::now();

        let inner = match guard
            .take()
            .filter(|inner| inner.duration_unmodified_since(now) < self.time_to_live)
        {
            Some(inner) => inner,
            None => Inner {
                value: refresher.await?,
                last_modified: now,
            },
        };

        let ret = inner.value.clone();
        *guard = Some(inner);

        Ok(ret)
    }
}

#[derive(Debug)]
struct Inner<T> {
    value: T,
    last_modified: Instant,
}

impl<T> Inner<T> {
    fn duration_unmodified(&self) -> Duration {
        Instant::now() - self.last_modified
    }

    fn duration_unmodified_since(&self, time_point: Instant) -> Duration {
        time_point - self.last_modified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsolescent_cell_sync() {
        let cell = ObsolescentCell::new(Duration::from_secs(3));
        assert!(matches!(cell.get(), ObsolescentValue::Absent));

        cell.set(true);
        assert!(matches!(cell.get(), ObsolescentValue::Fresh(true)));
        assert_eq!(cell.get_or_refresh(|| Ok::<_, ()>(false)), Ok(true));

        std::thread::sleep(Duration::from_secs(4));
        assert!(matches!(cell.get(), ObsolescentValue::Obsolete(true)));
        assert_eq!(cell.get_or_refresh(|| Err("Oh no")), Err("Oh no"));
        assert_eq!(cell.get_or_refresh(|| Ok::<_, ()>(false)), Ok(false));
    }

    #[tokio::test]
    async fn obsolescent_cell_async() {
        let cell = ObsolescentCellAsync::new(Duration::from_secs(3));
        assert!(matches!(cell.get().await, ObsolescentValue::Absent));

        cell.set(true).await;
        assert!(matches!(cell.get().await, ObsolescentValue::Fresh(true)));
        assert_eq!(
            cell.get_or_refresh(async { Ok::<_, ()>(false) }).await,
            Ok(true)
        );

        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(matches!(cell.get().await, ObsolescentValue::Obsolete(true)));
        assert_eq!(
            cell.get_or_refresh(async { Err("Oh no") }).await,
            Err("Oh no")
        );
        assert_eq!(
            cell.get_or_refresh(async { Ok::<_, ()>(false) }).await,
            Ok(false)
        );
    }
}
