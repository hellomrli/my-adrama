use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

use crate::util::with_retry;

#[derive(Clone)]
pub struct OpenAiClient {
    http: Client,
    api_key: String,
    base_url: String,
    chat_model: String,
    image_model: String,
}

impl OpenAiClient {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        chat_model: impl Into<String>,
        image_model: impl Into<String>,
    ) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()?;
        Ok(Self {
            http,
            api_key: api_key.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            chat_model: chat_model.into(),
            image_model: image_model.into(),
        })
    }

    pub async fn chat_json(
        &self,
        system: &str,
        user: &str,
        schema_name: &str,
        schema: Value,
    ) -> Result<Value> {
        let body = json!({
            "model": self.chat_model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": schema_name,
                    "strict": true,
                    "schema": schema
                }
            }
        });

        let url = format!("{}/chat/completions", self.base_url);
        let text = with_retry("openai chat", 3, Duration::from_secs(2), || {
            let url = url.clone();
            let body = body.clone();
            async move {
                let resp = self
                    .http
                    .post(&url)
                    .bearer_auth(&self.api_key)
                    .json(&body)
                    .send()
                    .await
                    .context("openai chat request failed")?;
                let status = resp.status();
                let text = resp.text().await?;
                if !status.is_success() {
                    bail!("openai chat error {status}: {text}");
                }
                Ok(text)
            }
        })
        .await?;

        let v: Value = serde_json::from_str(&text)?;
        let content = v["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow!("missing message content in chat response: {text}"))?;
        serde_json::from_str(content).context("failed to parse chat JSON content")
    }

    /// Fallback chat without strict schema (for providers that don't support json_schema).
    pub async fn chat_json_loose(&self, system: &str, user: &str) -> Result<Value> {
        let body = json!({
            "model": self.chat_model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "response_format": { "type": "json_object" }
        });

        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("openai chat request failed")?;

        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!("openai chat error {status}: {text}");
        }

        let v: Value = serde_json::from_str(&text)?;
        let content = v["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow!("missing message content: {text}"))?;

        // Strip markdown fences if present
        let cleaned = strip_json_fences(content);
        serde_json::from_str(cleaned).context("failed to parse chat JSON content")
    }

    pub async fn generate_image(
        &self,
        prompt: &str,
        size: &str,
        out_path: &Path,
    ) -> Result<()> {
        let body = json!({
            "model": self.image_model,
            "prompt": prompt,
            "size": size,
            "n": 1,
        });

        let url = format!("{}/images/generations", self.base_url);
        let text = with_retry("openai image gen", 3, Duration::from_secs(2), || {
            let url = url.clone();
            let body = body.clone();
            async move {
                let resp = self
                    .http
                    .post(&url)
                    .bearer_auth(&self.api_key)
                    .json(&body)
                    .send()
                    .await
                    .context("openai image generation request failed")?;
                let status = resp.status();
                let text = resp.text().await?;
                if !status.is_success() {
                    bail!("openai image generation error {status}: {text}");
                }
                Ok(text)
            }
        })
        .await?;

        let v: Value = serde_json::from_str(&text)?;
        save_image_from_response(&v, out_path).await
    }

    /// Multi-reference image edit (character consistency).
    pub async fn edit_image(
        &self,
        prompt: &str,
        size: &str,
        reference_images: &[&Path],
        out_path: &Path,
    ) -> Result<()> {
        if reference_images.is_empty() {
            return self.generate_image(prompt, size, out_path).await;
        }

        let mut form = Form::new()
            .text("model", self.image_model.clone())
            .text("prompt", prompt.to_string())
            .text("size", size.to_string())
            .text("n", "1");

        for (i, path) in reference_images.iter().enumerate() {
            let bytes = tokio::fs::read(path)
                .await
                .with_context(|| format!("read reference image {}", path.display()))?;
            let filename = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("ref.png")
                .to_string();
            let mime = mime_from_path(path);
            let part = Part::bytes(bytes)
                .file_name(filename)
                .mime_str(mime)?;
            // OpenAI edits: field name `image` or `image[]` depending on API version
            let field = if reference_images.len() == 1 {
                "image".to_string()
            } else {
                format!("image[{i}]")
            };
            form = form.part(field, part);
        }

        let url = format!("{}/images/edits", self.base_url);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .context("openai image edit request failed")?;

        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!("openai image edit error {status}: {text}");
        }

        let v: Value = serde_json::from_str(&text)?;
        save_image_from_response(&v, out_path).await
    }
}

async fn save_image_from_response(v: &Value, out_path: &Path) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let data = &v["data"][0];
    if let Some(b64) = data["b64_json"].as_str() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("decode b64_json image")?;
        tokio::fs::write(out_path, bytes).await?;
        return Ok(());
    }
    if let Some(url) = data["url"].as_str() {
        let bytes = reqwest::get(url)
            .await
            .context("download generated image")?
            .bytes()
            .await?;
        tokio::fs::write(out_path, &bytes).await?;
        return Ok(());
    }
    bail!("image response has neither b64_json nor url: {v}");
}

fn mime_from_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

fn strip_json_fences(s: &str) -> &str {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("```json") {
        return rest.trim_end_matches("```").trim();
    }
    if let Some(rest) = s.strip_prefix("```") {
        return rest.trim_end_matches("```").trim();
    }
    s
}

