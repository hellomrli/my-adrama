//! OpenAI-compatible client — covers OpenAI itself, xAI/Grok, and any proxy
//! that speaks `/chat/completions` and `/images/*`.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use futures::future::BoxFuture;
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use std::path::Path;

use super::http::{Http, HttpError};
use super::{ChatJsonRequest, ChatProvider, ImageProvider, ImageRequest};
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
    async fn chat_once(&self, label: &str, body: Value) -> Result<Value> {
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
                |chunk| {
                    chunk
                        .pointer("/choices/0/delta/content")
                        .and_then(|c| c.as_str())
                        .map(str::to_string)
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
            let strict_err = match self.chat_once("对话请求", self.strict_body(&req)).await {
                Ok(value) => return Ok(value),
                Err(err) => err,
            };

            // 超时、限流、网关故障说明「这次没跑通」，不是「不支持 json_schema」；
            // 换成兼容模式只会再烧一次钱，还拿到更差的结果。
            if !should_fall_back(&strict_err) {
                return Err(strict_err);
            }

            tracing::info!("json_schema 模式被拒绝（{strict_err}），改用 json_object 模式");
            self.chat_once("对话请求（兼容模式）", self.loose_body(&req))
                .await
                .map_err(|loose_err| {
                    anyhow!("结构化输出失败：{strict_err}；兼容模式亦失败：{loose_err}")
                })
        })
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
