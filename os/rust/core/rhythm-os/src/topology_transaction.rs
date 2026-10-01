//! Serialize topology commits separately from ordinary controller dispatch.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

/// Mutations, takeover, release, and restore fence dispatch by default.
/// Reconciliation may retain topology serialization without fencing dispatch
/// when the integration keeps its existing authority and safely routes around
/// any unverified native groups.
#[derive(Default)]
pub struct ExternalTopologyTransactionLock {
    topology: Mutex<()>,
    dispatch: Arc<Mutex<()>>,
}

pub struct ExternalTopologyTransactionGuard<'a> {
    _dispatch: Option<MutexGuard<'a, ()>>,
    _topology: MutexGuard<'a, ()>,
}

impl ExternalTopologyTransactionLock {
    pub fn is_poisoned(&self) -> bool {
        self.topology.is_poisoned() || self.dispatch.is_poisoned()
    }
    pub fn dispatch_lock(&self) -> Arc<Mutex<()>> {
        self.dispatch.clone()
    }

    pub fn lock(&self) -> Result<ExternalTopologyTransactionGuard<'_>, PoisonError<()>> {
        let topology = self.topology.lock().map_err(|_| PoisonError::new(()))?;
        let dispatch = self.dispatch.lock().map_err(|_| PoisonError::new(()))?;
        Ok(ExternalTopologyTransactionGuard {
            _topology: topology,
            _dispatch: Some(dispatch),
        })
    }

    pub fn lock_reconciliation(
        &self,
    ) -> Result<ExternalTopologyTransactionGuard<'_>, PoisonError<()>> {
        let topology = self.topology.lock().map_err(|_| PoisonError::new(()))?;
        if self.dispatch.is_poisoned() {
            return Err(PoisonError::new(()));
        }
        Ok(ExternalTopologyTransactionGuard {
            _topology: topology,
            _dispatch: None,
        })
    }

    pub fn try_lock(&self) -> Result<ExternalTopologyTransactionGuard<'_>, TryLockError<()>> {
        fn map_error<T>(error: TryLockError<T>) -> TryLockError<()> {
            match error {
                TryLockError::WouldBlock => TryLockError::WouldBlock,
                TryLockError::Poisoned(_) => TryLockError::Poisoned(PoisonError::new(())),
            }
        }
        let topology = self.topology.try_lock().map_err(map_error)?;
        let dispatch = self.dispatch.try_lock().map_err(map_error)?;
        Ok(ExternalTopologyTransactionGuard {
            _topology: topology,
            _dispatch: Some(dispatch),
        })
    }
}
