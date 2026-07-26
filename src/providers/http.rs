//! Shared HTTP plumbing: one client per provider, status-aware retries, and
//! secret redaction so an API key can never reach a log line or the UI.

use anyhow::{bail, Result};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::Value;
use std::time::Duration;
use tracing::warn;

#[derive(Clone)]
pub struct Http {
    client: Client,
    retries: u32,
    /// Redacted from every error message this client produces.
    secret: String,
}

impl Http {
    pub fn new(secret: impl Into<String>, retries: u32, timeout: Duration) -> Result<Self> {
        let client = Client::builder()
            .timeout(timeout)
            .user_agent(concat!("adrama/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            client,
            retries: retries.max(1),
            secret: secret.into(),
        })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn redact(&self, text: &str) -> String {
        redact_secret(text, &self.secret)
    }

    /// Send a request, retrying transient failures. `make` is called once per
    /// attempt because a `RequestBuilder` (multipart in particular) is consumed
    /// on send.
    pub async fn send(&self, label: &str, make: impl Fn() -> RequestBuilder) -> Result<String> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let outcome = self.attempt(&make).await;
            match outcome {
                Ok(body) => return Ok(body),
                Err(err) => {
                    if !err.retryable || attempt >= self.retries {
                        bail!("{label} 失败：{}", err.message);
                    }
                    let delay = Duration::from_secs(2u64.pow(attempt.min(4)));
                    warn!(
                        "{label} 第 {attempt}/{} 次失败：{}；{:?} 后重试",
                        self.retries, err.message, delay
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    /// Same retry policy, but keeps the body as bytes (media downloads).
    pub async fn send_bytes(&self, label: &str, make: impl Fn() -> RequestBuilder) -> Result<Vec<u8>> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let result = async {
                let resp = make().send().await.map_err(|e| Attempt {
                    retryable: e.is_timeout() || e.is_connect() || e.is_request(),
                    message: self.redact(&format!("{e}")),
                })?;
                let status = resp.status();
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(Attempt {
                        retryable: is_retryable_status(status),
                        message: format!(
                            "HTTP {status}{} — {}",
                            auth_hint(status),
                            self.redact(&preview(&body))
                        ),
                    });
                }
                resp.bytes().await.map(|b| b.to_vec()).map_err(|e| Attempt {
                    retryable: true,
                    message: self.redact(&format!("{e}")),
                })
            }
            .await;

            match result {
                Ok(bytes) => return Ok(bytes),
                Err(err) => {
                    if !err.retryable || attempt >= self.retries {
                        bail!("{label} 失败：{}", err.message);
                    }
                    tokio::time::sleep(Duration::from_secs(2u64.pow(attempt.min(4)))).await;
                }
            }
        }
    }

    pub async fn send_json(&self, label: &str, make: impl Fn() -> RequestBuilder) -> Result<Value> {
        let body = self.send(label, make).await?;
        serde_json::from_str(&body)
            .map_err(|e| anyhow::anyhow!("{label} 返回的不是合法 JSON：{e}；原文 {}", preview(&body)))
    }

    async fn attempt(
        &self,
        make: &impl Fn() -> RequestBuilder,
    ) -> std::result::Result<String, Attempt> {
        let resp = make().send().await.map_err(|e| Attempt {
            retryable: e.is_timeout() || e.is_connect() || e.is_request(),
            message: self.redact(&format!("{e}")),
        })?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.is_success() {
            return Ok(body);
        }

        Err(Attempt {
            retryable: is_retryable_status(status),
            message: format!(
                "HTTP {status}{} — {}",
                auth_hint(status),
                self.redact(&preview(&body))
            ),
        })
    }
}

struct Attempt {
    retryable: bool,
    message: String,
}

fn is_retryable_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504)
}

fn auth_hint(status: StatusCode) -> &'static str {
    match status.as_u16() {
        401 | 403 => "（密钥无效或无权限）",
        404 => "（端点或模型名不存在）",
        429 => "（触发限流）",
        _ => "",
    }
}

/// Replace an API key with a masked form anywhere it appears.
pub fn redact_secret(text: &str, secret: &str) -> String {
    let secret = secret.trim();
    if secret.len() < 8 {
        return text.to_string();
    }
    text.replace(secret, &mask(secret))
}

/// `sk-abcd…wxyz (51 字符)` — enough to tell two keys apart, useless to a thief.
pub fn mask(key: &str) -> String {
    let key = key.trim();
    let len = key.chars().count();
    if len <= 8 {
        return "****".into();
    }
    let head: String = key.chars().take(4).collect();
    let tail: String = key
        .chars()
        .skip(len.saturating_sub(4))
        .collect();
    format!("{head}…{tail} ({len} 字符)")
}

/// Trim a response body for error messages.
pub fn preview(body: &str) -> String {
    const MAX: usize = 300;
    let trimmed = body.trim();
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(MAX).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_from_messages() {
        let key = "sk-verysecretvalue0123456789";
        let text = format!("boom at https://api/x?key={key}&y=1");
        let out = redact_secret(&text, key);
        assert!(!out.contains(key));
        assert!(out.contains("sk-v…6789"));
    }

    #[test]
    fn short_keys_are_fully_masked() {
        assert_eq!(mask("abc"), "****");
        assert!(mask("0123456789").starts_with("0123…"));
    }

    #[test]
    fn retry_policy_is_status_based() {
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(StatusCode::BAD_GATEWAY));
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED));
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST));
    }
}
