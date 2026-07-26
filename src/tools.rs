//! 本地工具管理：ffmpeg 与 Piper（本地 TTS）。
//!
//! 解析顺序：托管目录（配置目录下 `tools/`）优先，其次系统 PATH。
//! 下载来源固定为上游官方发布（GitHub / Hugging Face），
//! 落盘前先解压验证，能跑 `-version` 才算安装成功。

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::settings::AppSettings;

/// 可托管的工具。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// 拼接 / 混音 / 烧字幕。上游：BtbN/FFmpeg-Builds 滚动最新版。
    Ffmpeg,
    /// 本地 TTS 引擎。上游：rhasspy/piper。
    Piper,
    /// Piper 中文音色（zh_CN-huayan-medium）。上游：Hugging Face piper-voices。
    PiperVoice,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "ffmpeg",
            Tool::Piper => "Piper（本地 TTS 引擎）",
            Tool::PiperVoice => "Piper 中文音色",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolStatus {
    pub path: PathBuf,
    pub version: String,
    /// true = 托管下载的；false = 系统里找到的。
    pub managed: bool,
}

pub fn tools_dir() -> PathBuf {
    AppSettings::config_path()
        .parent()
        .map(|p| p.join("tools"))
        .unwrap_or_else(|| PathBuf::from("tools"))
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

pub fn resolve_ffmpeg() -> Option<ToolStatus> {
    let managed = tools_dir().join(exe("ffmpeg"));
    if managed.is_file() {
        if let Some(version) = version_of(&managed, "-version") {
            return Some(ToolStatus {
                path: managed,
                version,
                managed: true,
            });
        }
    }
    let system = which(&exe("ffmpeg"))?;
    let version = version_of(&system, "-version")?;
    Some(ToolStatus {
        path: system,
        version,
        managed: false,
    })
}

pub fn resolve_piper() -> Option<ToolStatus> {
    // Piper 需要整个目录（可执行 + onnxruntime 库 + espeak-ng-data）
    let managed = tools_dir().join("piper").join(exe("piper"));
    if managed.is_file() {
        return Some(ToolStatus {
            path: managed,
            version: "托管安装".into(),
            managed: true,
        });
    }
    let system = which(&exe("piper"))?;
    Some(ToolStatus {
        path: system,
        version: "系统安装".into(),
        managed: false,
    })
}

/// 已安装的 Piper 音色模型（.onnx，需伴随 .onnx.json）。
pub fn resolve_piper_voice() -> Option<PathBuf> {
    let dir = tools_dir().join("voices");
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut models: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("onnx")
                && p.with_extension("onnx.json").is_file()
        })
        .collect();
    models.sort();
    models.into_iter().next()
}

fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn version_of(path: &Path, flag: &str) -> Option<String> {
    let output = Command::new(path).arg(flag).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    // "ffmpeg version N-114xxx-gxxxx ..." → 取前三段足够识别
    Some(line.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
}

// ---------------------------------------------------------------------------
// 安装
// ---------------------------------------------------------------------------

struct Download {
    url: &'static str,
    file: &'static str,
}

fn downloads(tool: Tool) -> Vec<Download> {
    const BTBN: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest";
    const PIPER: &str = "https://github.com/rhasspy/piper/releases/download/2023.11.14-2";
    const VOICE: &str =
        "https://huggingface.co/rhasspy/piper-voices/resolve/main/zh/zh_CN/huayan/medium";

    match (tool, cfg!(windows)) {
        (Tool::Ffmpeg, true) => vec![Download {
            url: constcat_ffwin(BTBN),
            file: "ffmpeg-master-latest-win64-gpl.zip",
        }],
        (Tool::Ffmpeg, false) => vec![Download {
            url: constcat_fflinux(BTBN),
            file: "ffmpeg-master-latest-linux64-gpl.tar.xz",
        }],
        (Tool::Piper, true) => vec![Download {
            url: constcat_piperwin(PIPER),
            file: "piper_windows_amd64.zip",
        }],
        (Tool::Piper, false) => vec![Download {
            url: constcat_piperlinux(PIPER),
            file: "piper_linux_x86_64.tar.gz",
        }],
        (Tool::PiperVoice, _) => vec![
            Download {
                url: constcat_voice_onnx(VOICE),
                file: "zh_CN-huayan-medium.onnx",
            },
            Download {
                url: constcat_voice_json(VOICE),
                file: "zh_CN-huayan-medium.onnx.json",
            },
        ],
    }
}

// const 拼接的简单替代：leak 一次即可（进程生命周期内固定几个字符串）。
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}
fn constcat_ffwin(base: &str) -> &'static str {
    leak(format!("{base}/ffmpeg-master-latest-win64-gpl.zip"))
}
fn constcat_fflinux(base: &str) -> &'static str {
    leak(format!("{base}/ffmpeg-master-latest-linux64-gpl.tar.xz"))
}
fn constcat_piperwin(base: &str) -> &'static str {
    leak(format!("{base}/piper_windows_amd64.zip"))
}
fn constcat_piperlinux(base: &str) -> &'static str {
    leak(format!("{base}/piper_linux_x86_64.tar.gz"))
}
fn constcat_voice_onnx(base: &str) -> &'static str {
    leak(format!("{base}/zh_CN-huayan-medium.onnx"))
}
fn constcat_voice_json(base: &str) -> &'static str {
    leak(format!("{base}/zh_CN-huayan-medium.onnx.json"))
}

/// 下载来源必须落在可信主机上。
fn check_host(url: &str) -> Result<()> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| anyhow!("拒绝非 HTTPS 下载地址"))?;
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .split('@')
        .next_back()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let trusted = host == "github.com"
        || host.ends_with(".github.com")
        || host.ends_with(".githubusercontent.com")
        || host == "huggingface.co"
        || host.ends_with(".huggingface.co")
        || host.ends_with(".hf.co");
    if !trusted {
        bail!("拒绝从非受信主机下载：{host}");
    }
    Ok(())
}

/// 下载并安装工具。返回一句人话结果。
pub async fn install(tool: Tool, on_progress: impl Fn(u64, u64)) -> Result<String> {
    let dir = tools_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("创建 {}", dir.display()))?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1800))
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("adrama/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let items = downloads(tool);
    let total_files = items.len();
    let staging = dir.join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    // 出错时清理暂存目录
    let _ = total_files;
    let result = install_inner(tool, &client, &items, &dir, &staging, &on_progress).await;
    let _ = std::fs::remove_dir_all(&staging);
    result
}

async fn install_inner(
    tool: Tool,
    client: &reqwest::Client,
    items: &[Download],
    dir: &Path,
    staging: &Path,
    on_progress: &impl Fn(u64, u64),
) -> Result<String> {
    let mut archives: Vec<PathBuf> = Vec::new();
    for item in items {
        check_host(item.url)?;
        let target = staging.join(item.file);
        let mut response = client
            .get(item.url)
            .send()
            .await
            .with_context(|| format!("下载 {} 失败（检查网络或代理）", item.file))?;
        let status = response.status();
        if !status.is_success() {
            bail!("下载 {} 失败：HTTP {status}", item.file);
        }
        let total = response.content_length().unwrap_or(0);
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.context("下载中断")? {
            bytes.extend_from_slice(&chunk);
            on_progress(bytes.len() as u64, total);
        }
        std::fs::write(&target, &bytes).with_context(|| format!("写入 {}", target.display()))?;
        archives.push(target);
    }

    match tool {
        Tool::Ffmpeg => {
            let extract = staging.join("x");
            extract_archive(&archives[0], &extract)?;
            let binary = find_file(&extract, &exe("ffmpeg"))
                .ok_or_else(|| anyhow!("压缩包里没找到 ffmpeg 可执行文件"))?;
            let target = dir.join(exe("ffmpeg"));
            std::fs::copy(&binary, &target)?;
            make_executable(&target)?;
            let version = version_of(&target, "-version")
                .ok_or_else(|| anyhow!("下载的 ffmpeg 无法运行（{} -version 失败）", target.display()))?;
            Ok(format!("ffmpeg 已安装：{version}"))
        }
        Tool::Piper => {
            let extract = staging.join("x");
            extract_archive(&archives[0], &extract)?;
            // Piper 需要整个目录（可执行 + onnxruntime + espeak-ng-data）
            let binary = find_file(&extract, &exe("piper"))
                .ok_or_else(|| anyhow!("压缩包里没找到 piper 可执行文件"))?;
            let source_dir = binary.parent().unwrap().to_path_buf();
            let target_dir = dir.join("piper");
            let _ = std::fs::remove_dir_all(&target_dir);
            copy_dir(&source_dir, &target_dir)?;
            make_executable(&target_dir.join(exe("piper")))?;
            Ok("Piper 已安装（本地 TTS 引擎）".into())
        }
        Tool::PiperVoice => {
            let voices = dir.join("voices");
            std::fs::create_dir_all(&voices)?;
            for archive in &archives {
                let name = archive.file_name().unwrap();
                std::fs::copy(archive, voices.join(name))?;
            }
            Ok("中文音色 zh_CN-huayan-medium 已安装".into())
        }
    }
}

/// 解压：不引入压缩库，用系统自带工具。
/// Linux 桌面必有 tar；Windows 10+ 自带 PowerShell 的 Expand-Archive。
fn extract_archive(archive: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    let name = archive.to_string_lossy();

    let status = if name.ends_with(".zip") {
        if cfg!(windows) {
            Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &format!(
                        "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                        archive.display(),
                        target.display()
                    ),
                ])
                .status()
        } else {
            Command::new("unzip")
                .args(["-o", &name, "-d", &target.to_string_lossy()])
                .status()
        }
    } else {
        // .tar.xz / .tar.gz：tar 自动识别压缩格式
        Command::new("tar")
            .args(["-xf", &name, "-C", &target.to_string_lossy()])
            .status()
    }
    .context("找不到解压工具（Linux 需要 tar，Windows 需要 PowerShell）")?;

    if !status.success() {
        bail!("解压 {} 失败（退出码 {status}）", archive.display());
    }
    Ok(())
}

fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.file_name().is_some_and(|f| f == name) {
            return Some(path);
        }
        if path.is_dir() {
            dirs.push(path);
        }
    }
    dirs.into_iter().find_map(|d| find_file(&d, name))
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)?.permissions().mode();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// 本地 TTS 调用
// ---------------------------------------------------------------------------

/// 用 Piper 合成一段中文语音到 wav。文本走 stdin，避免命令行长度与转义问题。
pub fn piper_synthesize(piper: &Path, model: &Path, text: &str, out_wav: &Path) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    if let Some(parent) = out_wav.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut child = Command::new(piper)
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(out_wav)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("启动 {}", piper.display()))?;

    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(text.as_bytes())
        .context("向 Piper 写入文本")?;

    let output = child.wait_with_output().context("等待 Piper 结束")?;
    if !output.status.success() {
        bail!(
            "Piper 合成失败：{}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or("未知错误")
        );
    }
    if !out_wav.is_file() {
        bail!("Piper 没有产出音频文件");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_trusted_hosts_are_allowed() {
        assert!(check_host("https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/x.zip").is_ok());
        assert!(check_host("https://huggingface.co/rhasspy/piper-voices/resolve/main/x.onnx").is_ok());
        assert!(check_host("https://cdn-lfs.huggingface.co/x").is_ok());
        assert!(check_host("https://evil.example.com/ffmpeg.zip").is_err());
        assert!(check_host("http://github.com/x").is_err());
    }

    #[test]
    fn download_lists_match_platform() {
        let ff = downloads(Tool::Ffmpeg);
        assert_eq!(ff.len(), 1);
        assert!(ff[0].url.contains("BtbN"));
        let voice = downloads(Tool::PiperVoice);
        assert_eq!(voice.len(), 2, "onnx + json 两个文件");
        assert!(voice.iter().all(|d| d.url.contains("huggingface")));
    }

    #[test]
    fn version_line_is_condensed() {
        // 用 /bin/echo 假装是个 -version 输出正常的工具
        if cfg!(unix) {
            let v = version_of(Path::new("/bin/echo"), "-version");
            assert_eq!(v.as_deref(), Some("-version"));
        }
    }
}
