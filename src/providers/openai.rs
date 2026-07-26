//! OpenAI-compatible client — covers OpenAI itself, xAI/Grok, and any proxy
//! that speaks `/chat/completions` and `/images/*`.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use futures::future::BoxFuture;
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use std::path::Path;

use super::http::{Http, HttpError};
use super::{
    ChatJsonRequest, ChatProvider, ImageProvider, ImageRequest, SpeechProvider, SpeechRequest,
    VideoPoll, VideoProvider, VideoRequest,
};
use crate::model::Endpoint;

pub struct OpenAiCompatible {
    http: Http,
    api_key: String,
    endpoint: Endpoint,
}

impl OpenAiCompatible {
    pub fn new(http: Http, api_key: String, endpoint: Endpoint) -> Self {
        Self {
            http,
            api_key,
            endpoint,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.endpoint.base_url, path.trim_start_matches('/'))
    }

    /// Strict structured output. Not every proxy implements `json_schema`, so
    /// the caller may fall back to the looser body.
    fn strict_body(&self, req: &ChatJsonRequest<'_>) -> Value {
        json!({
            "model": self.endpoint.model,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": req.user},
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": req.schema_name,
                    "strict": true,
                    "schema": req.schema,
                }
            },
            "stream": true,
        })
    }

    /// Looser mode: ask for a JSON object and describe the shape in the prompt.
    fn loose_body(&self, req: &ChatJsonRequest<'_>) -> Value {
        let schema_hint = serde_json::to_string_pretty(req.schema).unwrap_or_default();
        let user = format!(
            "{}\n\n严格按以下 JSON Schema 输出，只返回 JSON 本体：\n{schema_hint}",
            req.user
        );
        json!({
            "model": self.endpoint.model,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": user},
            ],
            "response_format": { "type": "json_object" },
            "stream": true,
        })
    }

    /// 发一次对话请求。走流式：长剧本 + 慢模型经常要一两分钟，非流式会被
    /// 网关（Cloudflare 默认 100 秒）判成 524。
    async fn chat_once(&self, label: &str, body: Value, req: &ChatJsonRequest<'_>) -> Result<Value> {
        let url = self.url("chat/completions");
        let text = self
            .http
            .send_sse(
                label,
                || {
                    self.http
                        .client()
                        .post(&url)
                        .bearer_auth(&self.api_key)
                        .json(&body)
                },
                extract_delta,
                |progress| {
                    if let Some(report) = req.on_progress {
                        report(progress);
                    }
                },
            )
            .await?;

        // 上游忽略 stream 时拿到的是整包响应，仍按老路子解析。
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            if value.get("choices").is_some() {
                return parse_chat_content(&value);
            }
        }

        let cleaned = strip_code_fences(&text);
        serde_json::from_str(cleaned)
            .with_context(|| format!("模型返回的不是合法 JSON：{}", super::http::preview(cleaned)))
    }

    async fn image_generate(&self, req: &ImageRequest<'_>) -> Result<Vec<u8>> {
        let body = json!({
            "model": self.endpoint.model,
            "prompt": req.prompt,
            "size": req.aspect.openai_size(),
            "n": 1,
        });
        let url = self.url("images/generations");
        let value = self
            .http
            .send_json("图像生成", || {
                self.http
                    .client()
                    .post(&url)
                    .bearer_auth(&self.api_key)
                    .json(&body)
            })
            .await?;
        self.extract_image(&value).await
    }

    /// Reference-guided edit — the mechanism behind character consistency.
    async fn image_edit(&self, req: &ImageRequest<'_>) -> Result<Vec<u8>> {
        let mut parts = Vec::new();
        for path in req.references {
            let bytes = tokio::fs::read(path)
                .await
                .with_context(|| format!("读取参考图 {}", path.display()))?;
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("ref.png")
                .to_string();
            parts.push((name, bytes, mime_for(path)));
        }

        let url = self.url("images/edits");
        let multi = req.references.len() > 1;
        let value = self
            .http
            .send_json("图像编辑", || {
                let mut form = Form::new()
                    .text("model", self.endpoint.model.clone())
                    .text("prompt", req.prompt.to_string())
                    .text("size", req.aspect.openai_size().to_string())
                    .text("n", "1");
                for (i, (name, bytes, mime)) in parts.iter().enumerate() {
                    let part = Part::bytes(bytes.clone())
                        .file_name(name.clone())
                        .mime_str(mime)
                        .expect("static mime type");
                    // Single reference uses `image`, multiple use `image[]`.
                    let field = if multi {
                        format!("image[{i}]")
                    } else {
                        "image".to_string()
                    };
                    form = form.part(field, part);
                }
                self.http
                    .client()
                    .post(&url)
                    .bearer_auth(&self.api_key)
                    .multipart(form)
            })
            .await?;
        self.extract_image(&value).await
    }

    async fn extract_image(&self, value: &Value) -> Result<Vec<u8>> {
        let data = value
            .get("data")
            .and_then(|d| d.get(0))
            .ok_or_else(|| anyhow!("图像响应缺少 data[0]：{}", super::http::preview(&value.to_string())))?;

        if let Some(b64) = data.get("b64_json").and_then(|v| v.as_str()) {
            return base64::engine::general_purpose::STANDARD
                .decode(b64)
                .context("解码 b64_json 图像");
        }
        if let Some(url) = data.get("url").and_then(|v| v.as_str()) {
            let url = url.to_string();
            return self
                .http
                .send_bytes("下载生成的图像", || self.http.client().get(&url))
                .await;
        }
        bail!("图像响应既无 b64_json 也无 url");
    }
}

impl ChatProvider for OpenAiCompatible {
    fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn complete_json<'a>(&'a self, req: ChatJsonRequest<'a>) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            let strict_err = match self
                .chat_once("对话请求", self.strict_body(&req), &req)
                .await
            {
                Ok(value) => return Ok(value),
                Err(err) => err,
            };

            // 超时、限流、网关故障说明「这次没跑通」，不是「不支持 json_schema」；
            // 换成兼容模式只会再烧一次钱，还拿到更差的结果。
            if !should_fall_back(&strict_err) {
                return Err(strict_err);
            }

            tracing::info!("json_schema 模式被拒绝（{strict_err}），改用 json_object 模式");
            self.chat_once("对话请求（兼容模式）", self.loose_body(&req), &req)
                .await
                .map_err(|loose_err| {
                    anyhow!("结构化输出失败：{strict_err}；兼容模式亦失败：{loose_err}")
                })
        })
    }

    fn complete_text<'a>(
        &'a self,
        system: &'a str,
        user: &'a str,
        on_progress: Option<&'a (dyn Fn(super::http::SseProgress) + Send + Sync)>,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move { self.chat_text(system, user, on_progress).await })
    }
}

impl OpenAiCompatible {
    /// 纯文本对话（流式）。
    async fn chat_text(
        &self,
        system: &str,
        user: &str,
        on_progress: Option<&(dyn Fn(super::http::SseProgress) + Send + Sync)>,
    ) -> Result<String> {
        let body = json!({
            "model": self.endpoint.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "stream": true,
        });
        let url = self.url("chat/completions");
        let text = self
            .http
            .send_sse(
                "对话请求（文本）",
                || {
                    self.http
                        .client()
                        .post(&url)
                        .bearer_auth(&self.api_key)
                        .json(&body)
                },
                extract_delta,
                |progress| {
                    if let Some(report) = on_progress {
                        report(progress);
                    }
                },
            )
            .await?;

        // 上游忽略 stream 时是整包响应
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            if let Some(content) = value
                .pointer("/choices/0/message/content")
                .and_then(|v| v.as_str())
            {
                return Ok(content.to_string());
            }
        }
        Ok(strip_code_fences(&text).to_string())
    }
}

impl ImageProvider for OpenAiCompatible {
    fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn supports_references(&self) -> bool {
        true
    }

    fn generate<'a>(&'a self, req: ImageRequest<'a>) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            if req.references.is_empty() {
                self.image_generate(&req).await
            } else {
                self.image_edit(&req).await
            }
        })
    }
}

impl SpeechProvider for OpenAiCompatible {
    fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn synthesize<'a>(&'a self, req: SpeechRequest<'a>) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let body = json!({
                "model": self.endpoint.model,
                "input": req.text,
                "voice": req.voice,
                "response_format": "mp3",
            });
            let url = self.url("audio/speech");
            let bytes = self
                .http
                .send_bytes("语音合成", || {
                    self.http
                        .client()
                        .post(&url)
                        .bearer_auth(&self.api_key)
                        .json(&body)
                })
                .await?;

            // 有些中转对错误也回 200 + JSON；音频不可能以 '{' 开头。
            if bytes.first() == Some(&b'{') {
                let text = String::from_utf8_lossy(&bytes);
                bail!("语音端点返回了错误：{}", super::http::preview(&text));
            }
            if bytes.is_empty() {
                bail!("语音端点返回了空音频");
            }
            Ok(bytes)
        })
    }
}

impl VideoProvider for OpenAiCompatible {
    fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// 提交一个视频任务，返回任务 id。
    ///
    /// OpenAI 兼容阵营（xAI、各类中转）大多是「POST 建任务 → 轮询 → 取回」这一套，
    /// 但路径与字段名并不统一。这里按最常见的形态提交，并把上游原样的响应放进
    /// 错误信息里——一旦对不上，看一眼就知道该怎么改。
    fn submit<'a>(&'a self, req: VideoRequest<'a>) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let frame = tokio::fs::read(req.image)
                .await
                .with_context(|| format!("读取首帧 {}", req.image.display()))?;
            let data_url = format!(
                "data:{};base64,{}",
                mime_for(req.image),
                base64::engine::general_purpose::STANDARD.encode(&frame)
            );

            let mut body = json!({
                "model": self.endpoint.model,
                "prompt": req.prompt,
                "seconds": req.duration_secs,
                "duration": req.duration_secs,
                "aspect_ratio": req.aspect.as_str(),
                "image": data_url,
            });
            if let Some(last) = req.last_image {
                if let Ok(bytes) = tokio::fs::read(last).await {
                    body["last_image"] = json!(format!(
                        "data:{};base64,{}",
                        mime_for(last),
                        base64::engine::general_purpose::STANDARD.encode(&bytes)
                    ));
                }
            }

            // 先试 /videos，不认再试 /videos/generations。
            let mut last_err = None;
            for path in ["videos", "videos/generations"] {
                let url = self.url(path);
                match self
                    .http
                    .send_json("提交视频任务", || {
                        self.http
                            .client()
                            .post(&url)
                            .bearer_auth(&self.api_key)
                            .json(&body)
                    })
                    .await
                {
                    Ok(value) => {
                        return extract_job_id(&value).ok_or_else(|| {
                            anyhow!(
                                "提交成功但没找到任务 id：{}",
                                super::http::preview(&value.to_string())
                            )
                        })
                    }
                    Err(err) => {
                        let unsupported = err
                            .downcast_ref::<HttpError>()
                            .map(|e| matches!(e.status, Some(404 | 405)))
                            .unwrap_or(false);
                        last_err = Some(err);
                        if !unsupported {
                            break;
                        }
                    }
                }
            }
            Err(last_err.unwrap_or_else(|| anyhow!("提交视频任务失败")))
        })
    }

    fn poll<'a>(&'a self, operation: &'a str) -> BoxFuture<'a, Result<VideoPoll>> {
        Box::pin(async move {
            let url = self.url(&format!("videos/{operation}"));
            let value = self
                .http
                .send_json("查询视频任务", || {
                    self.http.client().get(&url).bearer_auth(&self.api_key)
                })
                .await?;

            let status = ["/status", "/state", "/data/status"]
                .iter()
                .find_map(|p| value.pointer(p).and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_ascii_lowercase();

            if ["failed", "error", "cancelled", "canceled"].contains(&status.as_str()) {
                bail!(
                    "视频任务失败：{}",
                    super::http::preview(&value.to_string())
                );
            }

            if let Some(url) = find_video_url(&value) {
                let url = url.to_string();
                let bytes = self
                    .http
                    .send_bytes("下载视频", || self.http.client().get(&url).bearer_auth(&self.api_key))
                    .await?;
                return Ok(VideoPoll::Ready(bytes));
            }
            if let Some(b64) = find_video_base64(&value) {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .context("解码视频数据")?;
                return Ok(VideoPoll::Ready(bytes));
            }

            // 完成了却没给地址：有些实现要单独取内容。
            if ["completed", "succeeded", "success", "done", "ready"].contains(&status.as_str()) {
                let content = self.url(&format!("videos/{operation}/content"));
                return self
                    .http
                    .send_bytes("下载视频", || {
                        self.http.client().get(&content).bearer_auth(&self.api_key)
                    })
                    .await
                    .map(VideoPoll::Ready);
            }

            Ok(VideoPoll::Pending)
        })
    }
}

/// 任务 id 在不同实现里叫法不一。
fn extract_job_id(value: &Value) -> Option<String> {
    ["/id", "/data/id", "/task_id", "/request_id", "/job_id"]
        .iter()
        .find_map(|p| value.pointer(p).and_then(|v| v.as_str()))
        .map(str::to_string)
}

fn find_video_url(value: &Value) -> Option<&str> {
    [
        "/url",
        "/video_url",
        "/data/0/url",
        "/data/url",
        "/output/0/url",
        "/result/url",
        "/video/url",
    ]
    .iter()
    .find_map(|p| value.pointer(p).and_then(|v| v.as_str()))
    .filter(|url| url.starts_with("http"))
}

fn find_video_base64(value: &Value) -> Option<&str> {
    [
        "/b64_json",
        "/data/0/b64_json",
        "/video/bytesBase64Encoded",
        "/output/0/b64_json",
    ]
    .iter()
    .find_map(|p| value.pointer(p).and_then(|v| v.as_str()))
}

/// `GET /models` — used by the settings connectivity test.
pub async fn list_models(http: &Http, base: &str, key: &str) -> Result<Vec<String>> {
    let url = format!("{base}/models");
    let value = http
        .send_json("列出模型", || http.client().get(&url).bearer_auth(key))
        .await?;
    Ok(value
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// 不同中转的分片字段不尽相同：标准是 `delta.content`，也有直接给整条
/// `message.content` 或老式 `text` 的。思维链（`reasoning_content`）不算正文。
fn extract_delta(chunk: &Value) -> Option<String> {
    for pointer in [
        "/choices/0/delta/content",
        "/choices/0/message/content",
        "/choices/0/text",
    ] {
        if let Some(text) = chunk.pointer(pointer).and_then(|v| v.as_str()) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

/// 只有「上游明确不接受这个请求体」或「返回的内容不是合法 JSON」才值得换兼容模式。
/// 超时 / 5xx / 网关故障不属于此列。
fn should_fall_back(err: &anyhow::Error) -> bool {
    match err.downcast_ref::<HttpError>() {
        Some(http) => http.is_client_rejection(),
        // 不是 HTTP 层的失败——多半是模型没按 schema 返回，兼容模式可能更好。
        None => true,
    }
}

fn parse_chat_content(value: &Value) -> Result<Value> {
    let content = value
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            anyhow!(
                "对话响应缺少 choices[0].message.content：{}",
                super::http::preview(&value.to_string())
            )
        })?;
    let cleaned = strip_code_fences(content);
    serde_json::from_str(cleaned)
        .with_context(|| format!("模型返回的不是合法 JSON：{}", super::http::preview(cleaned)))
}

/// Models often wrap JSON in ```json fences despite being told not to.
pub fn strip_code_fences(s: &str) -> &str {
    let s = s.trim();
    let Some(rest) = s.strip_prefix("```") else {
        return s;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.trim_start_matches('\n')
        .trim_end_matches("```")
        .trim()
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_extraction_covers_common_relay_shapes() {
        // 标准
        assert_eq!(
            extract_delta(&json!({"choices":[{"delta":{"content":"你好"}}]})).as_deref(),
            Some("你好")
        );
        // 有些中转直接给整条 message
        assert_eq!(
            extract_delta(&json!({"choices":[{"message":{"content":"完整"}}]})).as_deref(),
            Some("完整")
        );
        // 老式 completions
        assert_eq!(
            extract_delta(&json!({"choices":[{"text":"旧式"}]})).as_deref(),
            Some("旧式")
        );
        // 思维链不算正文
        assert!(extract_delta(&json!({"choices":[{"delta":{"reasoning_content":"想想"}}]})).is_none());
        // 空分片（心跳）
        assert!(extract_delta(&json!({"choices":[{"delta":{}}]})).is_none());
    }

    #[test]
    fn video_job_fields_are_tolerated() {
        assert_eq!(extract_job_id(&json!({"id":"vid_1"})).as_deref(), Some("vid_1"));
        assert_eq!(extract_job_id(&json!({"data":{"id":"vid_2"}})).as_deref(), Some("vid_2"));
        assert_eq!(extract_job_id(&json!({"task_id":"vid_3"})).as_deref(), Some("vid_3"));
        assert!(extract_job_id(&json!({"error":"nope"})).is_none());

        assert_eq!(
            find_video_url(&json!({"data":[{"url":"https://x/v.mp4"}]})),
            Some("https://x/v.mp4")
        );
        // 相对路径不算可下载地址
        assert!(find_video_url(&json!({"url":"/tmp/v.mp4"})).is_none());
        assert_eq!(find_video_base64(&json!({"b64_json":"AAAA"})), Some("AAAA"));
    }

    #[test]
    fn fences_are_stripped() {
        assert_eq!(strip_code_fences("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fences("```\n{\"a\":1}```"), "{\"a\":1}");
        assert_eq!(strip_code_fences("{\"a\":1}"), "{\"a\":1}");
    }

    #[test]
    fn chat_content_is_parsed_from_choices() {
        let resp = json!({
            "choices": [{"message": {"content": "{\"title\":\"x\"}"}}]
        });
        let parsed = parse_chat_content(&resp).unwrap();
        assert_eq!(parsed["title"], "x");
    }

    #[test]
    fn fallback_only_on_client_rejection() {
        let rejected = anyhow::Error::new(HttpError {
            status: Some(400),
            message: "unsupported response_format".into(),
        });
        assert!(should_fall_back(&rejected));

        // Cloudflare 网关超时：重试才对，降级只会再烧一次钱
        let gateway = anyhow::Error::new(HttpError {
            status: Some(524),
            message: "A Timeout Occurred".into(),
        });
        assert!(!should_fall_back(&gateway));

        let rate_limited = anyhow::Error::new(HttpError {
            status: Some(429),
            message: "slow down".into(),
        });
        assert!(!should_fall_back(&rate_limited));

        // 连不上/超时（没有状态码）同样不该降级
        let transport = anyhow::Error::new(HttpError {
            status: None,
            message: "connection reset".into(),
        });
        assert!(!should_fall_back(&transport));

        // 模型返回的不是合法 JSON：换兼容模式有意义
        let parse = anyhow::anyhow!("模型返回的不是合法 JSON：<html>");
        assert!(should_fall_back(&parse));
    }

    #[test]
    fn chat_content_error_mentions_shape() {
        let resp = json!({"error": {"message": "nope"}});
        let err = parse_chat_content(&resp).unwrap_err();
        assert!(err.to_string().contains("choices"));
    }
}
