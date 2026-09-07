//! Keep disconnect inside the admitted BlueZ operation, including failed connects.
use crate::{LightError, LightResult};
use std::{future::Future, time::Duration};
use tokio::time::{timeout, timeout_at, Instant};

pub(crate) async fn with_disconnect<T>(
    deadline: Instant,
    timeout_error: LightError,
    operation: impl Future<Output = LightResult<T>>,
    disconnect: impl Future<Output = LightResult<()>>,
) -> LightResult<T> {
    // The caller sets this absolute deadline BEFORE shared admission. Its outer
    // coordinator timeout leaves more than five seconds beyond this deadline,
    // so neither slow admission nor session setup can consume the cleanup budget.
    if Instant::now() >= deadline {
        // Nothing was started; do not acquire a connection after our budget.
        return Err(timeout_error);
    }
    let result = timeout_at(deadline, operation)
        .await
        .unwrap_or(Err(timeout_error));
    // A failed/cancelled D-Bus Connect can already have established a link.
    // Always reconcile it before releasing the shared adapter gate.
    match timeout(Duration::from_secs(5), disconnect).await {
        Ok(Ok(())) => result,
        _ => Err(timeout_error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn disconnects_after_success_connect_failure_and_operation_failure() {
        for fail_at in [None, Some("connect"), Some("read")] {
            let events = Arc::new(Mutex::new(vec![]));
            let result = with_disconnect(
                Instant::now() + Duration::from_secs(1),
                LightError::Uncertain,
                async {
                    events.lock().unwrap().push("connect");
                    if fail_at == Some("connect") {
                        return Err(LightError::Unavailable);
                    }
                    events.lock().unwrap().push("read");
                    if fail_at == Some("read") {
                        return Err(LightError::Identity);
                    }
                    Ok(())
                },
                async {
                    events.lock().unwrap().push("disconnect");
                    Ok(())
                },
            )
            .await;
            assert_eq!(
                result,
                match fail_at {
                    Some("connect") => Err(LightError::Unavailable),
                    Some(_) => Err(LightError::Identity),
                    None => Ok(()),
                }
            );
            let expected = if fail_at == Some("connect") {
                vec!["connect", "disconnect"]
            } else {
                vec!["connect", "read", "disconnect"]
            };
            assert_eq!(*events.lock().unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn disconnects_after_stalled_connect_or_operation_before_returning() {
        for stall_connect in [true, false] {
            let events = Mutex::new(vec![]);
            let result: LightResult<()> = with_disconnect(
                Instant::now() + Duration::from_millis(20),
                LightError::Uncertain,
                async {
                    events.lock().unwrap().push("connect");
                    if !stall_connect {
                        events.lock().unwrap().push("read");
                    }
                    std::future::pending().await
                },
                async {
                    events.lock().unwrap().push("disconnect");
                    Ok(())
                },
            )
            .await;
            assert_eq!(result, Err(LightError::Uncertain));
            assert_eq!(events.lock().unwrap().last(), Some(&"disconnect"));
        }
    }

    #[tokio::test]
    async fn expired_admission_does_not_start_a_connection_and_cleanup_failure_is_uncertain() {
        let result: LightResult<()> = with_disconnect(
            Instant::now(),
            LightError::Uncertain,
            async { panic!("expired admission must not connect") },
            async { Ok(()) },
        )
        .await;
        assert_eq!(result, Err(LightError::Uncertain));
        let result = with_disconnect(
            Instant::now() + Duration::from_secs(1),
            LightError::Uncertain,
            async { Ok(()) },
            async { Err(LightError::Unavailable) },
        )
        .await;
        assert_eq!(result, Err(LightError::Uncertain));
    }
    #[tokio::test]
    async fn stalled_disconnect_finishes_before_the_outer_coordinator_deadline() {
        let result = timeout(
            Duration::from_secs(6),
            with_disconnect(
                Instant::now() + Duration::from_secs(1),
                LightError::Uncertain,
                async { Ok(()) },
                std::future::pending(),
            ),
        )
        .await
        .expect("cleanup must finish before the outer coordinator drops it");
        assert_eq!(result, Err(LightError::Uncertain));
    }
}
