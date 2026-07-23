use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use reqwest::Client;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::util::with_retry;

#[derive(Clone)]
pub struct GoogleClient {
    http: Client,
    api_key: String,
    base_url: String,
    video_model: String,
}

#[derive(Debug, Clone)]
pub struct VideoJob {
    pub operation_name: String,
}

impl GoogleClient {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        video_model: impl Into<String>,
    ) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self {
            http,
            api_key: api_key.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            video_model: video_model.into(),
        })
    }

    /// Submit image-to-video (Veo long-running). Returns operation name.
    pub async fn start_image_to_video(
        &self,
        prompt: &str,
        image_path: &Path,
        aspect_ratio: &str,
        duration_secs: u32,
    ) -> Result<VideoJob> {
        let image_bytes = tokio::fs::read(image_path)
            .await
            .with_context(|| format!("read image {}", image_path.display()))?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);
        let mime = match image_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("jpg") | Some("jpeg") => "image/jpeg",
            _ => "image/png",
        };

        let duration = duration_secs.clamp(4, 8);

        // Gemini API Veo predictLongRunning payload shape
        let body = json!({
            "instances": [{
                "prompt": prompt,
                "image": {
                    "bytesBase64Encoded": b64,
                    "mimeType": mime
                }
            }],
            "parameters": {
                "aspectRatio": aspect_ratio,
                "durationSeconds": duration,
                "sampleCount": 1
            }
        });

        let url = format!(
            "{}/models/{}:predictLongRunning?key={}",
            self.base_url, self.video_model, self.api_key
        );

        let text = with_retry("veo start", 3, Duration::from_secs(3), || {
            let url = url.clone();
            let body = body.clone();
            async move {
                let resp = self
                    .http
                    .post(&url)
                    .json(&body)
                    .send()
                    .await
                    .context("veo predictLongRunning request failed")?;
                let status = resp.status();
                let text = resp.text().await?;
                if !status.is_success() {
                    bail!("veo start error {status}: {text}");
                }
                Ok(text)
            }
        })
        .await?;

        let v: Value = serde_json::from_str(&text)?;
        let name = v["name"]
            .as_str()
            .ok_or_else(|| anyhow!("missing operation name in response: {text}"))?
            .to_string();

        Ok(VideoJob {
            operation_name: name,
        })
    }

    /// Poll operation until done; download first video sample to out_path.
    pub async fn wait_and_download(
        &self,
        operation_name: &str,
        out_path: &Path,
        poll_interval: Duration,
        max_wait: Duration,
    ) -> Result<()> {
        let started = std::time::Instant::now();
        loop {
            if started.elapsed() > max_wait {
                bail!(
                    "video operation timed out after {:?} ({operation_name})",
                    max_wait
                );
            }

            let op = self.get_operation(operation_name).await?;
            if op["done"].as_bool() == Some(true) {
                if let Some(err) = op.get("error") {
                    bail!("video operation failed: {err}");
                }
                return self.download_from_operation(&op, out_path).await;
            }

            info!(
                "waiting for video operation {} ({:.0}s elapsed)",
                operation_name,
                started.elapsed().as_secs_f32()
            );
            sleep(poll_interval).await;
        }
    }

    pub async fn get_operation(&self, operation_name: &str) -> Result<Value> {
        // operation_name is like "operations/xxx" or full resource path
        let name = operation_name.trim_start_matches('/');
        let url = if name.starts_with("http") {
            format!("{name}?key={}", self.api_key)
        } else {
            format!("{}/{}?key={}", self.base_url, name, self.api_key)
        };

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("get operation failed")?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!("get operation error {status}: {text}");
        }
        Ok(serde_json::from_str(&text)?)
    }

    async fn download_from_operation(&self, op: &Value, out_path: &Path) -> Result<()> {
        if let Some(parent) = out_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Common response shapes for Veo
        let response = &op["response"];

        // Shape A: generateVideoResponse.generatedSamples[0].video.uri
        if let Some(uri) = response
            .pointer("/generateVideoResponse/generatedSamples/0/video/uri")
            .and_then(|v| v.as_str())
            .or_else(|| {
                response
                    .pointer("/generatedSamples/0/video/uri")
                    .and_then(|v| v.as_str())
            })
            .or_else(|| {
                response
                    .pointer("/videos/0/uri")
                    .and_then(|v| v.as_str())
            })
        {
            return self.download_uri(uri, out_path).await;
        }

        // Shape B: base64 bytes
        if let Some(b64) = response
            .pointer("/generateVideoResponse/generatedSamples/0/video/bytesBase64Encoded")
            .and_then(|v| v.as_str())
            .or_else(|| {
                response
                    .pointer("/generatedSamples/0/video/bytesBase64Encoded")
                    .and_then(|v| v.as_str())
            })
            .or_else(|| {
                response
                    .pointer("/videos/0/bytesBase64Encoded")
                    .and_then(|v| v.as_str())
            })
        {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .context("decode video base64")?;
            tokio::fs::write(out_path, bytes).await?;
            return Ok(());
        }

        // Shape C: predictions
        if let Some(uri) = response
            .pointer("/predictions/0/bytesBase64Encoded")
            .and_then(|v| v.as_str())
        {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(uri)
                .context("decode prediction video")?;
            tokio::fs::write(out_path, bytes).await?;
            return Ok(());
        }

        warn!("unexpected operation response shape: {op}");
        bail!("could not find video data in completed operation response");
    }

    async fn download_uri(&self, uri: &str, out_path: &Path) -> Result<()> {
        // File download URIs from Gemini often need API key
        let url = if uri.contains('?') {
            format!("{uri}&key={}", self.api_key)
        } else {
            format!("{uri}?key={}", self.api_key)
        };

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("download video uri failed")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("video download error {status}: {text}");
        }
        let bytes = resp.bytes().await?;
        tokio::fs::write(out_path, &bytes).await?;
        Ok(())
    }
}
