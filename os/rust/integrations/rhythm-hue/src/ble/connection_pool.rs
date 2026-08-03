//! Platform-neutral admission state for the Linux Hue BLE connection pool.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

#[cfg(all(target_os = "linux", feature = "bluez"))]
use super::{BLE_CONNECTION_POOL_CAPACITY, BLE_PARALLEL_CONNECT_LIMIT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    users: usize,
    last_used: u64,
    evicting: bool,
}

#[derive(Debug)]
struct PoolState {
    capacity: usize,
    clock: u64,
    entries: HashMap<String, Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Claim {
    Lease,
    Evict(String),
    Wait,
}

impl PoolState {
    fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "BLE connection pool requires a slot");
        Self {
            capacity,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    fn claim(&mut self, key: &str) -> Claim {
        let last_used = self.tick();
        if let Some(entry) = self.entries.get_mut(key) {
            if entry.evicting {
                return Claim::Wait;
            }
            entry.users += 1;
            entry.last_used = last_used;
            return Claim::Lease;
        }

        if self.entries.len() < self.capacity {
            self.entries.insert(
                key.to_string(),
                Entry {
                    users: 1,
                    last_used,
                    evicting: false,
                },
            );
            return Claim::Lease;
        }

        let victim = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.users == 0 && !entry.evicting)
            .min_by(|(left_key, left), (right_key, right)| {
                left.last_used
                    .cmp(&right.last_used)
                    .then_with(|| left_key.cmp(right_key))
            })
            .map(|(key, _)| key.clone());
        let Some(victim) = victim else {
            return Claim::Wait;
        };
        self.entries
            .get_mut(&victim)
            .expect("selected connection-pool victim exists")
            .evicting = true;
        Claim::Evict(victim)
    }

    fn release(&mut self, key: &str) {
        let last_used = self.tick();
        if let Some(entry) = self.entries.get_mut(key) {
            entry.users = entry.users.saturating_sub(1);
            entry.last_used = last_used;
        }
    }

    fn complete_eviction(&mut self, key: &str) {
        if self
            .entries
            .get(key)
            .is_some_and(|entry| entry.evicting && entry.users == 0)
        {
            self.entries.remove(key);
        }
    }

    fn cancel_eviction(&mut self, key: &str) {
        let last_used = self.tick();
        if let Some(entry) = self.entries.get_mut(key) {
            entry.evicting = false;
            entry.last_used = last_used;
        }
    }

    #[cfg(all(target_os = "linux", feature = "bluez"))]
    fn forget(&mut self, key: &str) {
        self.entries.remove(key);
    }
}

/// Keeps the number of daemon-owned Hue ACL links below the controller budget
/// while allowing a much larger durable device catalog.
pub(crate) struct BleConnectionPool {
    state: Mutex<PoolState>,
    changed: Notify,
    connect_gate: Arc<Semaphore>,
}

impl BleConnectionPool {
    #[cfg(all(target_os = "linux", feature = "bluez"))]
    pub(crate) fn new() -> Arc<Self> {
        Self::with_limits(BLE_CONNECTION_POOL_CAPACITY, BLE_PARALLEL_CONNECT_LIMIT)
    }

    fn with_limits(capacity: usize, parallel_connects: usize) -> Arc<Self> {
        assert!(parallel_connects > 0, "BLE connect gate requires a permit");
        Arc::new(Self {
            state: Mutex::new(PoolState::new(capacity)),
            changed: Notify::new(),
            connect_gate: Arc::new(Semaphore::new(parallel_connects)),
        })
    }

    pub(crate) async fn admit(
        self: &Arc<Self>,
        key: &str,
        deadline: tokio::time::Instant,
    ) -> Result<ConnectionPoolAdmission> {
        loop {
            // Register before checking the state so a release cannot be lost
            // between observing Wait and beginning to await the notification.
            let changed = self.changed.notified();
            let claim = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("Hue BLE connection pool lock poisoned"))?
                .claim(key);
            match claim {
                Claim::Lease => {
                    return Ok(ConnectionPoolAdmission::Lease(ConnectionLease {
                        pool: Arc::clone(self),
                        key: key.to_string(),
                    }));
                }
                Claim::Evict(victim) => {
                    return Ok(ConnectionPoolAdmission::Evict(EvictionLease {
                        pool: Arc::clone(self),
                        key: victim,
                        completed: false,
                    }));
                }
                Claim::Wait => {
                    tokio::time::timeout_at(deadline, changed)
                        .await
                        .context("timed out waiting for a Hue BLE connection slot")?;
                }
            }
        }
    }

    pub(crate) async fn admit_connect(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<OwnedSemaphorePermit> {
        tokio::time::timeout_at(deadline, self.connect_gate.clone().acquire_owned())
            .await
            .context("timed out waiting for a Hue BLE connect permit")?
            .map_err(|_| anyhow::anyhow!("Hue BLE connect gate closed"))
    }

    #[cfg(all(target_os = "linux", feature = "bluez"))]
    pub(crate) fn forget(&self, key: &str) -> Result<()> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE connection pool lock poisoned"))?
            .forget(key);
        self.changed.notify_waiters();
        Ok(())
    }
}

pub(crate) enum ConnectionPoolAdmission {
    Lease(ConnectionLease),
    Evict(EvictionLease),
}

pub(crate) struct ConnectionLease {
    pool: Arc<BleConnectionPool>,
    key: String,
}

impl Drop for ConnectionLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.pool.state.lock() {
            state.release(&self.key);
        }
        self.pool.changed.notify_waiters();
    }
}

pub(crate) struct EvictionLease {
    pool: Arc<BleConnectionPool>,
    key: String,
    completed: bool,
}

impl EvictionLease {
    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    pub(crate) fn complete(mut self) {
        if let Ok(mut state) = self.pool.state.lock() {
            state.complete_eviction(&self.key);
        }
        self.completed = true;
        self.pool.changed.notify_waiters();
    }
}

impl Drop for EvictionLease {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        if let Ok(mut state) = self.pool.state.lock() {
            state.cancel_eviction(&self.key);
        }
        self.pool.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(admission: ConnectionPoolAdmission) -> ConnectionLease {
        match admission {
            ConnectionPoolAdmission::Lease(lease) => lease,
            ConnectionPoolAdmission::Evict(_) => panic!("expected a connection lease"),
        }
    }

    fn eviction(admission: ConnectionPoolAdmission) -> EvictionLease {
        match admission {
            ConnectionPoolAdmission::Evict(eviction) => eviction,
            ConnectionPoolAdmission::Lease(_) => panic!("expected an eviction lease"),
        }
    }

    #[tokio::test]
    async fn idle_lru_connection_is_evicted_before_admitting_another_device() {
        let pool = BleConnectionPool::with_limits(2, 1);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        drop(lease(pool.admit("bulb-a", deadline).await.unwrap()));
        drop(lease(pool.admit("bulb-b", deadline).await.unwrap()));

        let victim = eviction(pool.admit("bulb-c", deadline).await.unwrap());
        assert_eq!(victim.key(), "bulb-a");
        victim.complete();
        let _third = lease(pool.admit("bulb-c", deadline).await.unwrap());

        let state = pool.state.lock().unwrap();
        assert_eq!(state.entries.len(), 2);
        assert!(state.entries.contains_key("bulb-b"));
        assert!(state.entries.contains_key("bulb-c"));
    }

    #[tokio::test]
    async fn occupied_slots_wait_until_a_device_lease_is_released() {
        let pool = BleConnectionPool::with_limits(1, 1);
        let first = lease(
            pool.admit(
                "bulb-a",
                tokio::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .await
            .unwrap(),
        );
        let waiting_pool = Arc::clone(&pool);
        let waiter = tokio::spawn(async move {
            waiting_pool
                .admit(
                    "bulb-b",
                    tokio::time::Instant::now() + std::time::Duration::from_secs(1),
                )
                .await
                .unwrap()
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        drop(first);
        let victim = eviction(waiter.await.unwrap());
        assert_eq!(victim.key(), "bulb-a");
    }

    #[tokio::test]
    async fn cancelled_eviction_restores_the_previous_connection() {
        let pool = BleConnectionPool::with_limits(1, 1);
        drop(lease(
            pool.admit(
                "bulb-a",
                tokio::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .await
            .unwrap(),
        ));
        let eviction = eviction(
            pool.admit(
                "bulb-b",
                tokio::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .await
            .unwrap(),
        );
        drop(eviction);

        let same = lease(
            pool.admit(
                "bulb-a",
                tokio::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .await
            .unwrap(),
        );
        assert_eq!(pool.state.lock().unwrap().entries["bulb-a"].users, 1);
        drop(same);
    }

    #[tokio::test]
    async fn connect_attempts_have_a_smaller_independent_bound() {
        let pool = BleConnectionPool::with_limits(4, 2);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        let first = pool.admit_connect(deadline).await.unwrap();
        let second = pool.admit_connect(deadline).await.unwrap();
        assert!(pool.connect_gate.clone().try_acquire_owned().is_err());
        drop(first);
        assert!(pool.connect_gate.clone().try_acquire_owned().is_ok());
        drop(second);
    }
}
