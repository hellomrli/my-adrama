//! Shared helpers (retry, etc.)

use std::future::Future;
use std::time::Duration;
use tracing::warn;

/// Retry an async operation on transient failures.
pub async fn with_retry<T, E, F, Fut>(
    label: &str,
    attempts: u32,
    base_delay: Duration,
    mut f: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let attempts = attempts.max(1);
    let mut last_err = None;
    for i in 0..attempts {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let msg = e.to_string();
                let retryable = is_retryable(&msg);
                if !retryable || i + 1 == attempts {
                    return Err(e);
                }
                let delay = base_delay * 2u32.pow(i);
                warn!("{label} failed (attempt {}/{attempts}): {msg}; retry in {delay:?}", i + 1);
                tokio::time::sleep(delay).await;
                last_err = Some(e);
            }
        }
    }
    Err(last_err.expect("attempts >= 1"))
}

fn is_retryable(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("timeout")
        || m.contains("timed out")
        || m.contains("connection")
        || m.contains("reset")
        || m.contains("temporarily")
        || m.contains("429")
        || m.contains("rate limit")
        || m.contains("502")
        || m.contains("503")
        || m.contains("504")
        || m.contains("unavailable")
}
