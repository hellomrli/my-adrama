//! Shared HTTP plumbing: one client per provider, status-aware retries, and
//! secret redaction so an API key can never reach a log line or the UI.

use anyhow::{bail, Result};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::Value;
use std::time::Duration;
use tracing::warn;

/// 流式接收的实时情况：用来告诉用户「数据在回来」还是「一直没动静」。
#[derive(Debug, Clone, Copy, Default)]
pub struct SseProgress {
    /// 收到的 data 段数。
    pub events: usize,
    /// 已累积的正文字数。
    pub chars: usize,
    /// 只有思维链、还没有正文的段数（推理模型常见）。
    pub thinking: usize,
    pub elapsed: Duration,
}

/// 带状态码的失败，调用方据此判断「换个方式重来」还是「直接放弃」。
#[derive(Debug)]
pub struct HttpError {
    pub status: Option<u16>,
    pub message: String,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HttpError {}

impl HttpError {
    /// 上游明确拒绝了这个请求体（而不是超时/网关故障）。
    pub fn is_client_rejection(&self) -> bool {
        matches!(self.status, Some(400 | 404 | 415 | 422 | 501))
    }
}

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
                        return Err(anyhow::Error::new(HttpError {
                            status: err.status,
                            message: format!("{label} 失败：{}", err.message),
                        }));
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
                let resp = make().send().await.map_err(|e| {
                    Attempt::transport(
                        None,
                        e.is_timeout() || e.is_connect() || e.is_request(),
                        self.redact(&format!("{e}")),
                    )
                })?;
                let status = resp.status();
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(Attempt::transport(
                        Some(status.as_u16()),
                        is_retryable_status(status),
                        format!(
                            "HTTP {status}{} — {}",
                            status_hint(status),
                            self.redact(&preview(&body))
                        ),
                    ));
                }
                resp.bytes()
                    .await
                    .map(|b| b.to_vec())
                    .map_err(|e| Attempt::transport(None, true, self.redact(&format!("{e}"))))
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

    /// 流式请求（SSE）。
    ///
    /// 长响应走非流式时，网关（尤其是 Cloudflare，100 秒）会在上游还没算完时
    /// 判定超时并返回 524。流式下字节持续到达，连接不会空闲，也能顺带看到
    /// 生成进度。`extract` 从每个 data 分片里取出增量文本。
    ///
    /// 若上游忽略了 `stream`（有些代理如此），会退回按整包 JSON 解析。
    pub async fn send_sse(
        &self,
        label: &str,
        make: impl Fn() -> RequestBuilder,
        extract: impl Fn(&Value) -> Option<String>,
        on_progress: impl Fn(SseProgress),
    ) -> Result<String> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.stream_once(&make, &extract, &on_progress).await {
                Ok(text) => return Ok(text),
                Err(err) => {
                    if !err.retryable || attempt >= self.retries {
                        return Err(anyhow::Error::new(HttpError {
                            status: err.status,
                            message: format!("{label} 失败：{}", err.message),
                        }));
                    }
                    let delay = Duration::from_secs(2u64.pow(attempt.min(4)));
                    warn!(
                        "{label} 第 {attempt}/{} 次失败：{}；{delay:?} 后重试",
                        self.retries, err.message
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    async fn stream_once(
        &self,
        make: &impl Fn() -> RequestBuilder,
        extract: &impl Fn(&Value) -> Option<String>,
        on_progress: &impl Fn(SseProgress),
    ) -> std::result::Result<String, Attempt> {
        let started = std::time::Instant::now();
        let mut progress = SseProgress::default();
        let mut response = make().send().await.map_err(|e| {
            Attempt::transport(
                None,
                e.is_timeout() || e.is_connect() || e.is_request(),
                self.redact(&format!("{e}")),
            )
        })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Attempt::transport(
                Some(status.as_u16()),
                is_retryable_status(status),
                format!(
                    "HTTP {status}{} — {}",
                    status_hint(status),
                    self.redact(&preview(&body))
                ),
            ));
        }

        let mut pending: Vec<u8> = Vec::new();
        let mut raw: Vec<u8> = Vec::new();
        let mut text = String::new();

        while let Some(chunk) = response.chunk().await.map_err(|e| {
            // 已经收到内容还断开：重试会重复计费，也拿不到前半段，直接报错。
            Attempt::transport(None, text.is_empty(), self.redact(&format!("{e}")))
        })? {
            raw.extend_from_slice(&chunk);
            pending.extend_from_slice(&chunk);
            // 按行切分：SSE 每行完整，不会切断多字节字符。
            while let Some(eol) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=eol).collect();
                let line = String::from_utf8_lossy(&line);
                let Some(data) = line.trim_end().strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<Value>(data) {
                    progress.events += 1;
                    match extract(&value) {
                        Some(delta) => text.push_str(&delta),
                        // 推理模型会先吐一大段思维链，正文要等后面才来。
                        None if is_thinking(&value) => progress.thinking += 1,
                        None => {}
                    }
                    progress.chars = text.chars().count();
                    progress.elapsed = started.elapsed();
                    on_progress(progress);
                }
            }
        }

        if !text.is_empty() {
            return Ok(text);
        }

        // 上游没按 SSE 回：当成普通 JSON 响应处理。
        let body = String::from_utf8_lossy(&raw).to_string();
        if body.trim().is_empty() {
            return Err(Attempt::transport(
                Some(status.as_u16()),
                true,
                "上游返回了空响应".into(),
            ));
        }
        if progress.events > 0 {
            // 收到了流式分片却一个字正文都没有——多半是字段名不一样。
            return Err(Attempt::transport(
                Some(status.as_u16()),
                false,
                format!(
                    "上游返回了 {} 段流式数据但没有正文{}；\
                     该中转的字段可能与 OpenAI 不一致，可在设置里换一个模型试试。原文：{}",
                    progress.events,
                    if progress.thinking > 0 {
                        format!("（其中 {} 段只有思维链）", progress.thinking)
                    } else {
                        String::new()
                    },
                    self.redact(&preview(&body))
                ),
            ));
        }
        Ok(body)
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
        let resp = make().send().await.map_err(|e| {
            Attempt::transport(
                None,
                e.is_timeout() || e.is_connect() || e.is_request(),
                self.redact(&format!("{e}")),
            )
        })?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.is_success() {
            return Ok(body);
        }

        Err(Attempt::transport(
            Some(status.as_u16()),
            is_retryable_status(status),
            format!(
                "HTTP {status}{} — {}",
                status_hint(status),
                self.redact(&preview(&body))
            ),
        ))
    }
}

struct Attempt {
    retryable: bool,
    status: Option<u16>,
    message: String,
}

impl Attempt {
    fn transport(status: Option<u16>, retryable: bool, message: String) -> Self {
        Self {
            retryable,
            status,
            message,
        }
    }
}

/// 推理模型的思维链分片：算「有动静」，但不是正文。
fn is_thinking(chunk: &Value) -> bool {
    ["/choices/0/delta/reasoning_content", "/choices/0/delta/reasoning"]
        .iter()
        .any(|p| chunk.pointer(p).map(|v| !v.is_null()).unwrap_or(false))
}

fn is_retryable_status(status: StatusCode) -> bool {
    matches!(
        status.as_u16(),
        // 常规瞬时故障
        408 | 409 | 425 | 429 | 500 | 502 | 503 | 504
        // Cloudflare 自定义区间：网关自己超时/连不上源站，重试有意义。
        // 525/526 是 TLS 配置问题，重试没用。
        | 520 | 521 | 522 | 523 | 524 | 527
    )
}

/// 把常见状态码翻译成「接下来该怎么办」。
fn status_hint(status: StatusCode) -> &'static str {
    match status.as_u16() {
        401 | 403 => "（密钥无效或无权限）",
        404 => "（端点或模型名不存在）",
        429 => "（触发限流）",
        413 => "（请求体过大，剧本可能太长）",
        520 | 521 | 523 => "（Cloudflare 连不上上游服务）",
        522 => "（Cloudflare 连接上游超时）",
        524 => "（Cloudflare 网关超时：上游在 100 秒内没返回。多见于代理 + 慢模型 + 长剧本，可换更快的模型、缩短剧本，或改用官方端点）",
        525 | 526 => "（Cloudflare 与上游的 TLS 握手失败）",
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
        // Cloudflare 网关超时属于瞬时故障
        assert!(is_retryable_status(StatusCode::from_u16(524).unwrap()));
        assert!(is_retryable_status(StatusCode::from_u16(520).unwrap()));
        // TLS 配置问题重试无意义
        assert!(!is_retryable_status(StatusCode::from_u16(525).unwrap()));
    }

    #[test]
    fn gateway_timeout_explains_itself() {
        let hint = status_hint(StatusCode::from_u16(524).unwrap());
        assert!(hint.contains("Cloudflare"), "{hint}");
        assert!(hint.contains("100"), "{hint}");
    }
}
