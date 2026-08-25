use std::{num::NonZeroUsize, sync::Arc};

use tokio::sync::{
    Mutex, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, OwnedSemaphorePermit,
    RwLock, Semaphore,
};
#[cfg(any(test, feature = "test-support"))]
use tokio::sync::{Mutex as TestMutex, oneshot};

/// Maximum number of request mutations admitted to the database-write queue.
pub const DEFAULT_DATABASE_WRITE_REQUEST_ADMISSION: NonZeroUsize =
    NonZeroUsize::new(64).expect("default database-write admission is nonzero");

/// Clone-shared coordinator for one database handle's gateway write mutations.
///
/// A request first reserves an admission permit, then serializes with other requests
/// before taking a read guard. Workers take the corresponding write guard. Tokio's
/// write-preferring `RwLock` prevents requests arriving after a queued worker from
/// passing it, while the request mutex leaves at most one request ahead of that worker.
#[derive(Clone, Debug)]
pub struct DatabaseWriteLock {
    /// Every clone of this coordinator shares these mutation primitives.
    inner: Arc<DatabaseWriteLockInner>,
}

/// Shared synchronization primitives for one database write coordinator.
#[derive(Debug)]
struct DatabaseWriteLockInner {
    /// Serializes request mutations and bounds the request queue before `mutation`.
    request_serialization: Arc<Mutex<()>>,
    /// Gives a queued worker priority over later request mutations.
    mutation: Arc<RwLock<()>>,
    /// Bounded admission for request-side waiters and active request mutations.
    request_admission: Arc<Semaphore>,
    /// Optional test-only observer notified at a request or worker acquisition boundary.
    #[cfg(any(test, feature = "test-support"))]
    observer: TestMutex<Option<oneshot::Sender<()>>>,
}

impl Default for DatabaseWriteLock {
    fn default() -> Self {
        Self::new(DEFAULT_DATABASE_WRITE_REQUEST_ADMISSION)
    }
}

impl DatabaseWriteLock {
    /// Creates a coordinator with a nonzero request-side admission limit.
    #[must_use]
    pub fn new(request_admission: NonZeroUsize) -> Self {
        Self {
            inner: Arc::new(DatabaseWriteLockInner {
                request_serialization: Arc::new(Mutex::new(())),
                mutation: Arc::new(RwLock::new(())),
                request_admission: Arc::new(Semaphore::new(request_admission.get())),
                #[cfg(any(test, feature = "test-support"))]
                observer: TestMutex::new(None),
            }),
        }
    }

    /// Admits and acquires a request-side database mutation guard.
    pub async fn acquire_request(&self) -> Result<RequestDatabaseWriteGuard, WriteAdmissionError> {
        let admission = self
            .inner
            .request_admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| WriteAdmissionError::Saturated)?;
        #[cfg(any(test, feature = "test-support"))]
        self.notify_test_observer().await;
        let request_serialization = self.inner.request_serialization.clone().lock_owned().await;
        let mutation = self.inner.mutation.clone().read_owned().await;
        Ok(RequestDatabaseWriteGuard {
            _admission: admission,
            _request_serialization: request_serialization,
            _mutation: mutation,
        })
    }

    /// Acquires a worker-side database mutation guard without request admission.
    pub async fn acquire_worker(&self) -> WorkerDatabaseWriteGuard {
        #[cfg(any(test, feature = "test-support"))]
        self.notify_test_observer().await;
        WorkerDatabaseWriteGuard {
            _mutation: self.inner.mutation.clone().write_owned().await,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Installs a one-shot test observer at the next request or worker queue boundary.
    #[doc(hidden)]
    pub async fn observe_next_acquisition(&self) -> oneshot::Receiver<()> {
        let (sender, receiver) = oneshot::channel();
        *self.inner.observer.lock().await = Some(sender);
        receiver
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn notify_test_observer(&self) {
        if let Some(observer) = self.inner.observer.lock().await.take() {
            let _ = observer.send(());
        }
    }
}

/// Request-side database-write admission failed because the bounded queue is full.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteAdmissionError {
    /// All request-side admission permits are held by waiting or active mutations.
    Saturated,
}

impl std::fmt::Display for WriteAdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Saturated => formatter.write_str("database write admission is saturated"),
        }
    }
}

impl std::error::Error for WriteAdmissionError {}

/// Opaque guard holding request admission and the shared mutation lock.
#[must_use]
pub struct RequestDatabaseWriteGuard {
    /// Shared mutation guard, excluded by a worker's write guard.
    _mutation: OwnedRwLockReadGuard<()>,
    /// Prevents a second request from joining the mutation lock before this one finishes.
    _request_serialization: OwnedMutexGuard<()>,
    /// Permit bounding waiting and active request mutations.
    _admission: OwnedSemaphorePermit,
}

/// Opaque guard holding the shared worker mutation lock.
#[must_use]
pub struct WorkerDatabaseWriteGuard {
    /// Exclusive mutation guard that blocks request mutations.
    _mutation: OwnedRwLockWriteGuard<()>,
}

#[cfg(test)]
mod tests;
