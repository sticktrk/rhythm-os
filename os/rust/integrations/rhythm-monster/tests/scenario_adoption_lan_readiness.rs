use async_trait::async_trait;
use rhythm_monster::{
    lan::LightTransport, pairing::wait_for_adoption_readback, LightError, LightProperty,
    LightResult,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

struct Spy {
    reads: Mutex<VecDeque<(Duration, LightResult<Value>)>>,
    calls: AtomicUsize,
    active: AtomicUsize,
}

impl Spy {
    fn new(reads: impl IntoIterator<Item = (Duration, LightResult<Value>)>) -> Self {
        Self {
            reads: Mutex::new(reads.into_iter().collect()),
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
        }
    }
}

struct ActiveRead<'a>(&'a AtomicUsize);
impl Drop for ActiveRead<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl LightTransport for Spy {
    async fn read(&self, property: LightProperty) -> LightResult<Value> {
        assert_eq!(property, LightProperty::Power);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveRead(&self.active);
        let (delay, result) = self
            .reads
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or((Duration::ZERO, Err(LightError::Unavailable)));
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        result
    }

    async fn write(&self, _: Vec<(LightProperty, Value)>) -> LightResult<()> {
        panic!("adoption must not write to the light");
    }
}

#[tokio::test(start_paused = true)]
async fn temporary_lan_failures_recover_without_restarting_pairing() {
    let transport = Spy::new([
        (Duration::from_secs(15), Err(LightError::Timeout)),
        (Duration::ZERO, Err(LightError::Unavailable)),
        (Duration::ZERO, Ok(json!(1))),
    ]);
    let mut pending = Vec::new();
    let started = tokio::time::Instant::now();
    assert_eq!(
        wait_for_adoption_readback(&transport, |error| pending.push(error)).await,
        Ok(())
    );
    assert_eq!(pending, [LightError::Timeout, LightError::Unavailable]);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 3);
    assert_eq!(started.elapsed(), Duration::from_secs(19));
    assert_eq!(transport.active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn ready_light_is_accepted_without_a_delay() {
    let transport = Spy::new([(Duration::ZERO, Ok(json!(0)))]);
    let started = tokio::time::Instant::now();
    assert_eq!(
        wait_for_adoption_readback(&transport, |_| panic!("unexpected retry")).await,
        Ok(())
    );
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn authentication_identity_and_other_permanent_failures_are_not_retried() {
    for error in [
        LightError::Authentication,
        LightError::Integrity,
        LightError::Identity,
        LightError::Readback,
        LightError::InvalidInput,
        LightError::Unsupported,
        LightError::Uncertain,
    ] {
        let transport = Spy::new([(Duration::ZERO, Err(error)), (Duration::ZERO, Ok(json!(1)))]);
        assert_eq!(
            wait_for_adoption_readback(&transport, |_| panic!("unexpected retry")).await,
            Err(error)
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn unreachable_light_stops_within_the_adoption_budget() {
    let transport = Spy::new([]);
    let started = tokio::time::Instant::now();
    assert_eq!(
        wait_for_adoption_readback(&transport, |_| {}).await,
        Err(LightError::Timeout)
    );
    assert_eq!(started.elapsed(), Duration::from_secs(60));
    assert!((2..=31).contains(&transport.calls.load(Ordering::SeqCst)));
    assert_eq!(transport.active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn adoption_deadline_cancels_a_stalled_transport_read() {
    let transport = Spy::new([(Duration::from_secs(3600), Ok(json!(1)))]);
    let started = tokio::time::Instant::now();
    assert_eq!(
        wait_for_adoption_readback(&transport, |_| panic!("unexpected retry")).await,
        Err(LightError::Timeout)
    );
    assert_eq!(started.elapsed(), Duration::from_secs(60));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert_eq!(transport.active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn cancelling_the_wait_stops_further_retries() {
    let transport = Spy::new([]);
    assert!(tokio::time::timeout(
        Duration::from_secs(3),
        wait_for_adoption_readback(&transport, |_| {})
    )
    .await
    .is_err());
    let calls = transport.calls.load(Ordering::SeqCst);
    assert_eq!(calls, 2);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(transport.calls.load(Ordering::SeqCst), calls);
    assert_eq!(transport.active.load(Ordering::SeqCst), 0);
}
