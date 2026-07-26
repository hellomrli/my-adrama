//! Google Gemini family: Gemini chat, Imagen / Gemini image, Veo video.
//!
//! The key travels in the `x-goog-api-key` header rather than a `?key=` query
//! parameter so it cannot leak through URLs in logs or proxy access logs.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use futures::future::BoxFuture;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::time::Duration;

use super::http::{preview, Http};
use super::{
    ChatJsonRequest, ChatProvider, ImageProvider, ImageRequest, VideoPoll, VideoProvider,
    VideoRequest,
};
use crate::model::Endpoint;

const API_KEY_HEADER: &str = "x-goog-api-key";

pub struct GoogleClient {
    http: Http,
    api_key: String,
    endpoint: Endpoint,
}

impl GoogleClient {
    pub fn new(http: Http, api_key: String, endpoint: Endpoint) -> Self {
        Self {
            http,
            api_key,
            endpoint,
        }
    }

    fn model_url(&self, action: &str) -> String {
        format!(
            "{}/models/{}:{}",
            self.endpoint.base_url, self.endpoint.model, action
        )
    }

    /// Imagen speaks `:predict`; the Gemini image models speak
    /// `:generateContent` and are the only ones that accept reference images.
    fn is_gemini_image_model(&self) -> bool {
        let m = self.endpoint.model.to_ascii_lowercase();
        m.contains("gemini") || m.contains("flash-image") || m.contains("nano")
    }

    async fn generate_content(&self, body: Value) -> Result<Value> {
        let url = self.model_url("generateContent");
        self.http
            .send_json("Gemini 请求", || {
                self.http
                    .client()
                    .post(&url)
                    .header(API_KEY_HEADER, &self.api_key)
                    .json(&body)
            })
            .await
    }

    async fn image_via_gemini(&self, req: &ImageRequest<'_>) -> Result<Vec<u8>> {
        let mut parts = vec![json!({ "text": req.prompt })];
        for path in req.references {
            let bytes = tokio::fs::read(path)
                .await
                .with_context(|| format!("读取参考图 {}", path.display()))?;
            parts.push(json!({
                "inline_data": {
                    "mime_type": mime_for(path),
                    "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                }
            }));
        }

        let body = json!({
            "contents": [{ "role": "user", "parts": parts }],
            "generationConfig": { "responseModalities": ["IMAGE"] }
        });
        let value = self.generate_content(body).await?;

        collect_parts(&value)
            .into_iter()
            .find_map(|part| {
                part.get("inlineData")
                    .or_else(|| part.get("inline_data"))
                    .and_then(|d| d.get("data"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .ok_or_else(|| anyhow!("Gemini 响应中没有图像数据：{}", preview(&value.to_string())))
            .and_then(|b64| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .context("解码 Gemini 图像")
            })
    }

    async fn image_via_imagen(&self, req: &ImageRequest<'_>) -> Result<Vec<u8>> {
        let body = json!({
            "instances": [{ "prompt": req.prompt }],
            "parameters": {
                "sampleCount": 1,
                "aspectRatio": req.aspect.as_str(),
            }
        });
        let url = self.model_url("predict");
        let value = self
            .http
            .send_json("Imagen 生成", || {
                self.http
                    .client()
                    .post(&url)
                    .header(API_KEY_HEADER, &self.api_key)
                    .json(&body)
            })
            .await?;

        let b64 = value
            .pointer("/predictions/0/bytesBase64Encoded")
            .or_else(|| value.pointer("/predictions/0/image/bytesBase64Encoded"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Imagen 响应中没有图像：{}", preview(&value.to_string())))?;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("解码 Imagen 图像")
    }

    async fn download(&self, uri: &str) -> Result<Vec<u8>> {
        self.check_download_host(uri)?;
        let uri = uri.to_string();
        self.http
            .send_bytes("下载视频", || {
                self.http
                    .client()
                    .get(&uri)
                    .header(API_KEY_HEADER, &self.api_key)
            })
            .await
    }

    /// The download URI comes from a server response. Only follow it when it
    /// points at Google (or the configured proxy) — otherwise a compromised or
    /// mistaken response could walk our API key to an arbitrary host.
    fn check_download_host(&self, uri: &str) -> Result<()> {
        let host = host_of(uri).ok_or_else(|| anyhow!("无法解析下载地址：{uri}"))?;
        let base_host = host_of(&self.endpoint.base_url).unwrap_or_default();
        let allowed = host.ends_with("googleapis.com")
            || host.ends_with("google.com")
            || (!base_host.is_empty() && host == base_host);
        if !allowed {
            bail!("拒绝从非受信主机下载视频：{host}");
        }
        Ok(())
    }
}

impl ChatProvider for GoogleClient {
    fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn complete_json<'a>(&'a self, req: ChatJsonRequest<'a>) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            let body = json!({
                "systemInstruction": { "parts": [{ "text": req.system }] },
                "contents": [{ "role": "user", "parts": [{ "text": req.user }] }],
                "generationConfig": {
                    "responseMimeType": "application/json",
                    "responseSchema": to_gemini_schema(req.schema),
                }
            });

            // 流式：拆解长剧本经常要一两分钟，非流式会被网关判成超时（524）。
            let url = format!("{}?alt=sse", self.model_url("streamGenerateContent"));
            let text = self
                .http
                .send_sse(
                    "Gemini 对话请求",
                    || {
                        self.http
                            .client()
                            .post(&url)
                            .header(API_KEY_HEADER, &self.api_key)
                            .json(&body)
                    },
                    |chunk| {
                        let parts = collect_parts(chunk);
                        let joined: String = parts
                            .iter()
                            .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
                            .collect();
                        (!joined.is_empty()).then_some(joined)
                    },
                )
                .await?;

            // 上游忽略 alt=sse 时返回整包 JSON（可能是数组），照样解析。
            let text = match serde_json::from_str::<Value>(&text) {
                Ok(value) if value.get("candidates").is_some() => collect_parts(&value)
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
                    .collect(),
                Ok(Value::Array(items)) => items
                    .iter()
                    .flat_map(|item| {
                        collect_parts(item)
                            .into_iter()
                            .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .collect(),
                _ => text,
            };

            if text.trim().is_empty() {
                bail!("Gemini 未返回文本内容");
            }
            let cleaned = super::openai::strip_code_fences(&text);
            serde_json::from_str(cleaned)
                .with_context(|| format!("Gemini 返回的不是合法 JSON：{}", preview(cleaned)))
        })
    }
}

impl ImageProvider for GoogleClient {
    fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn supports_references(&self) -> bool {
        self.is_gemini_image_model()
    }

    fn generate<'a>(&'a self, req: ImageRequest<'a>) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            if self.is_gemini_image_model() {
                self.image_via_gemini(&req).await
            } else {
                self.image_via_imagen(&req).await
            }
        })
    }
}

impl VideoProvider for GoogleClient {
    fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn submit<'a>(&'a self, req: VideoRequest<'a>) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let bytes = tokio::fs::read(req.image)
                .await
                .with_context(|| format!("读取首帧 {}", req.image.display()))?;
            let body = json!({
                "instances": [{
                    "prompt": req.prompt,
                    "image": {
                        "bytesBase64Encoded": base64::engine::general_purpose::STANDARD.encode(&bytes),
                        "mimeType": mime_for(req.image),
                    }
                }],
                "parameters": {
                    "aspectRatio": req.aspect.as_str(),
                    "durationSeconds": req.duration_secs,
                    "sampleCount": 1,
                }
            });

            let url = self.model_url("predictLongRunning");
            let value = self
                .http
                .send_json("提交视频任务", || {
                    self.http
                        .client()
                        .post(&url)
                        .header(API_KEY_HEADER, &self.api_key)
                        .json(&body)
                })
                .await?;

            value
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| anyhow!("提交响应缺少 operation name：{}", preview(&value.to_string())))
        })
    }

    fn poll<'a>(&'a self, operation: &'a str) -> BoxFuture<'a, Result<VideoPoll>> {
        Box::pin(async move {
            let name = operation.trim().trim_start_matches('/');
            let url = if name.starts_with("http") {
                name.to_string()
            } else {
                format!("{}/{name}", self.endpoint.base_url)
            };

            let op = self
                .http
                .send_json("查询视频任务", || {
                    self.http
                        .client()
                        .get(&url)
                        .header(API_KEY_HEADER, &self.api_key)
                })
                .await?;

            if op.get("done").and_then(|v| v.as_bool()) != Some(true) {
                return Ok(VideoPoll::Pending);
            }
            if let Some(err) = op.get("error") {
                bail!("视频任务失败：{}", preview(&err.to_string()));
            }

            let response = op.get("response").unwrap_or(&Value::Null);
            if let Some(uri) = find_str(
                response,
                &[
                    "/generateVideoResponse/generatedSamples/0/video/uri",
                    "/generatedSamples/0/video/uri",
                    "/videos/0/uri",
                    "/predictions/0/video/uri",
                ],
            ) {
                return Ok(VideoPoll::Ready(self.download(uri).await?));
            }
            if let Some(b64) = find_str(
                response,
                &[
                    "/generateVideoResponse/generatedSamples/0/video/bytesBase64Encoded",
                    "/generatedSamples/0/video/bytesBase64Encoded",
                    "/videos/0/bytesBase64Encoded",
                    "/predictions/0/bytesBase64Encoded",
                ],
            ) {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .context("解码视频数据")?;
                return Ok(VideoPoll::Ready(bytes));
            }

            bail!(
                "任务已完成但响应中找不到视频数据：{}",
                preview(&op.to_string())
            )
        })
    }
}

/// `GET /models` for the settings connectivity test.
pub async fn list_models(http: &Http, base: &str, key: &str) -> Result<Vec<String>> {
    let url = format!("{base}/models?pageSize=100");
    let value = http
        .send_json("列出模型", || {
            http.client()
                .get(&url)
                .header(API_KEY_HEADER, key)
                .timeout(Duration::from_secs(25))
        })
        .await?;
    // Gemini returns `models/gemini-2.0-flash`; config wants the bare id.
    Ok(value
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let name = m.get("name").and_then(|v| v.as_str())?;
                    Some(name.trim_start_matches("models/").to_string())
                })
                .collect()
        })
        .unwrap_or_default())
}

fn collect_parts(value: &Value) -> Vec<&Value> {
    value
        .pointer("/candidates/0/content/parts")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

fn find_str<'a>(value: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|p| value.pointer(p).and_then(|v| v.as_str()))
}

fn host_of(url: &str) -> Option<String> {
    let rest = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url);
    let host = rest
        .split(['/', '?', '#'])
        .next()?
        .split('@')
        .next_back()?
        .split(':')
        .next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Translate a JSON Schema into the OpenAPI subset Gemini accepts:
/// no `additionalProperties`, and union types become a single type + nullable.
pub fn to_gemini_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = Map::new();
            let mut nullable = false;

            for (key, value) in map {
                match key.as_str() {
                    "additionalProperties" | "$schema" | "strict" => continue,
                    "type" => match value {
                        Value::Array(types) => {
                            let mut primary = None;
                            for t in types {
                                match t.as_str() {
                                    Some("null") => nullable = true,
                                    Some(other) => primary = primary.or(Some(other.to_string())),
                                    None => {}
                                }
                            }
                            out.insert(
                                "type".into(),
                                Value::String(primary.unwrap_or_else(|| "string".into())),
                            );
                        }
                        other => {
                            out.insert("type".into(), other.clone());
                        }
                    },
                    _ => {
                        out.insert(key.clone(), to_gemini_schema(value));
                    }
                }
            }
            if nullable {
                out.insert("nullable".into(), Value::Bool(true));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(to_gemini_schema).collect()),
        other => other.clone(),
    }
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
    use crate::model::{Capability, EndpointMode, ProviderId};

    fn endpoint(model: &str) -> Endpoint {
        Endpoint {
            provider: ProviderId::Google,
            capability: Capability::Video,
            mode: EndpointMode::Official,
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            model: model.into(),
        }
    }

    fn client(model: &str) -> GoogleClient {
        GoogleClient::new(
            Http::new("test-key-0123456789", 1, Duration::from_secs(5)).unwrap(),
            "test-key-0123456789".into(),
            endpoint(model),
        )
    }

    #[test]
    fn schema_is_translated_for_gemini() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["a"],
            "properties": {
                "a": { "type": ["string", "null"] },
                "b": { "type": "array", "items": { "type": "object", "additionalProperties": false } }
            }
        });
        let out = to_gemini_schema(&schema);
        assert!(out.get("additionalProperties").is_none());
        assert_eq!(out["properties"]["a"]["type"], "string");
        assert_eq!(out["properties"]["a"]["nullable"], true);
        assert!(out["properties"]["b"]["items"]
            .get("additionalProperties")
            .is_none());
        assert_eq!(out["required"][0], "a");
    }

    #[test]
    fn download_host_is_restricted() {
        let c = client("veo-3.1-generate-preview");
        assert!(c
            .check_download_host("https://generativelanguage.googleapis.com/v1beta/files/x")
            .is_ok());
        let err = c
            .check_download_host("https://evil.example.com/steal")
            .unwrap_err();
        assert!(err.to_string().contains("拒绝"));
    }

    #[test]
    fn image_model_family_selects_endpoint_shape() {
        assert!(client("gemini-2.5-flash-image").supports_references());
        assert!(!client("imagen-3.0-generate-002").supports_references());
    }

    #[test]
    fn host_parsing_handles_ports_and_paths() {
        assert_eq!(host_of("https://a.b.com:443/x?y=1").as_deref(), Some("a.b.com"));
        assert_eq!(host_of("http://127.0.0.1:8081/v1beta").as_deref(), Some("127.0.0.1"));
    }
}
