//! 在线更新：查询 GitHub Releases、校验并替换自身。
//!
//! 只信任两件事：HTTPS 到 github.com，以及 release 里随附的 `SHA256SUMS.txt`。
//! 通过包管理器（.deb）安装的副本不会被自我替换——那是包管理器的地盘，
//! 直接覆盖 `/usr/bin` 里的文件会和 apt 打架。

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const REPO: &str = "hellomrli/my-adrama";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const API_URL: &str = "https://api.github.com/repos";
const CHECKSUM_FILE: &str = "SHA256SUMS.txt";
/// 自动检查的最小间隔。
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// 一个可下载的产物。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// GitHub 上的一个 release。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    /// 去掉 v 前缀的版本号，如 `0.2.1`。
    pub version: String,
    pub tag: String,
    pub title: String,
    pub notes: String,
    pub page_url: String,
    pub assets: Vec<Asset>,
}

impl ReleaseInfo {
    /// 当前平台该下载哪个产物。
    pub fn asset_for_platform(&self) -> Option<&Asset> {
        let wanted = platform_asset_name();
        self.assets.iter().find(|a| a.name == wanted)
    }

    fn checksums(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == CHECKSUM_FILE)
    }
}

#[derive(Debug, Clone)]
pub enum UpdateStatus {
    UpToDate,
    Available(Box<ReleaseInfo>),
}

/// 这份程序是怎么装上的，决定了能不能自我替换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// 绿色版 / 直接下载的可执行文件：可以就地更新。
    Portable(PathBuf),
    /// 由包管理器安装（如 .deb）：交给包管理器。
    Managed(PathBuf),
    /// 定位不到自身路径（少见）。
    Unknown,
}

impl InstallKind {
    pub fn can_self_update(&self) -> bool {
        matches!(self, InstallKind::Portable(_))
    }

    pub fn describe(&self) -> String {
        match self {
            InstallKind::Portable(p) => format!("独立可执行文件：{}", p.display()),
            InstallKind::Managed(p) => {
                format!("由包管理器安装（{}），请用 apt / dpkg 更新", p.display())
            }
            InstallKind::Unknown => "无法定位程序自身路径".into(),
        }
    }
}

/// 更新完成后的结果。
#[derive(Debug, Clone)]
pub struct Applied {
    pub version: String,
    /// 替换后的可执行文件路径，用于「立即重启」。
    pub executable: PathBuf,
    pub verified: bool,
}

pub fn install_kind() -> InstallKind {
    let Ok(exe) = std::env::current_exe() else {
        return InstallKind::Unknown;
    };
    // 系统目录一律视为包管理器管辖，即使当前是 root 也不去动它。
    for prefix in ["/usr/", "/opt/", "/snap/"] {
        if exe.starts_with(prefix) {
            return InstallKind::Managed(exe);
        }
    }
    match exe.parent().map(dir_is_writable) {
        Some(true) => InstallKind::Portable(exe),
        _ => InstallKind::Managed(exe),
    }
}

fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".adrama-write-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 当前平台对应的产物文件名。
pub fn platform_asset_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "adrama.exe"
    } else {
        "adrama-linux-x86_64"
    }
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("adrama/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("创建 HTTP 客户端失败")
}

/// 查询最新 release 并与当前版本比较。
pub async fn check() -> Result<UpdateStatus> {
    let release = latest_release().await?;
    Ok(if is_newer(&release.version, CURRENT_VERSION) {
        UpdateStatus::Available(Box::new(release))
    } else {
        UpdateStatus::UpToDate
    })
}

pub async fn latest_release() -> Result<ReleaseInfo> {
    let url = format!("{API_URL}/{REPO}/releases/latest");
    let response = client()?
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("连接 GitHub 失败（检查网络或代理）")?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("查询更新失败：HTTP {status}{}", rate_limit_hint(status));
    }
    parse_release(&body)
}

fn rate_limit_hint(status: reqwest::StatusCode) -> &'static str {
    match status.as_u16() {
        403 | 429 => "（GitHub 匿名接口限流，稍后再试）",
        404 => "（仓库还没有发布过 release）",
        _ => "",
    }
}

fn parse_release(body: &str) -> Result<ReleaseInfo> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("GitHub 返回的不是合法 JSON")?;

    let tag = value
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("release 响应缺少 tag_name"))?
        .to_string();

    let assets = value
        .get("assets")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|a| {
                    Some(Asset {
                        name: a.get("name")?.as_str()?.to_string(),
                        url: a.get("browser_download_url")?.as_str()?.to_string(),
                        size: a.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(ReleaseInfo {
        version: normalize_version(&tag).to_string(),
        tag: tag.clone(),
        title: value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&tag)
            .to_string(),
        notes: value
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        page_url: value
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        assets,
    })
}

/// 下载对应产物、校验、替换自身。`on_progress(已下载, 总量)`。
pub async fn download_and_apply(
    release: &ReleaseInfo,
    on_progress: impl Fn(u64, u64),
) -> Result<Applied> {
    let install = install_kind();
    let InstallKind::Portable(exe) = &install else {
        bail!("{}", install.describe());
    };

    let asset = release.asset_for_platform().ok_or_else(|| {
        anyhow!(
            "该版本没有当前平台的产物（需要 {}）",
            platform_asset_name()
        )
    })?;

    let bytes = download(&asset.url, asset.size, &on_progress).await?;
    if bytes.is_empty() {
        bail!("下载到的文件为空");
    }

    let verified = match verify(release, &asset.name, &bytes).await {
        Ok(v) => v,
        Err(err) => bail!("校验失败，已放弃更新：{err:#}"),
    };

    replace_executable(exe, &bytes)?;
    Ok(Applied {
        version: release.version.clone(),
        executable: exe.clone(),
        verified,
    })
}

async fn download(url: &str, expected_size: u64, on_progress: &impl Fn(u64, u64)) -> Result<Vec<u8>> {
    check_host(url)?;
    let mut response = client()?
        .get(url)
        .send()
        .await
        .context("下载失败（检查网络或代理）")?;

    let status = response.status();
    if !status.is_success() {
        bail!("下载失败：HTTP {status}");
    }

    let total = response.content_length().unwrap_or(expected_size);
    let mut bytes = Vec::with_capacity(total as usize);
    while let Some(chunk) = response.chunk().await.context("下载中断")? {
        bytes.extend_from_slice(&chunk);
        on_progress(bytes.len() as u64, total);
    }
    Ok(bytes)
}

/// 只从 GitHub 下载；响应里的地址不能把我们引到任意主机。
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
        || host.ends_with(".githubusercontent.com");
    if !trusted {
        bail!("拒绝从非 GitHub 主机下载：{host}");
    }
    Ok(())
}

/// 与 release 里的 `SHA256SUMS.txt` 比对。没有该文件时返回 false（未校验）。
async fn verify(release: &ReleaseInfo, asset_name: &str, bytes: &[u8]) -> Result<bool> {
    let Some(sums) = release.checksums() else {
        return Ok(false);
    };
    let text = String::from_utf8(download(&sums.url, sums.size, &|_, _| {}).await?)
        .context("校验和文件不是文本")?;
    let Some(expected) = expected_digest(&text, asset_name) else {
        // 有校验和文件但没有这一项：宁可当作异常，也不装一个来路不明的二进制。
        bail!("{CHECKSUM_FILE} 中没有 {asset_name} 的校验和");
    };

    let actual = sha256_hex(bytes);
    if actual != expected {
        bail!("SHA-256 不匹配（期望 {expected}，实际 {actual}）");
    }
    Ok(true)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 解析 `sha256sum` 输出：`<hex>  <filename>`。
pub fn expected_digest(text: &str, asset_name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let digest = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset_name && digest.len() == 64).then(|| digest.to_ascii_lowercase())
    })
}

/// 原地替换可执行文件。
///
/// Linux/macOS：`rename` 是原子的，且正在运行的进程持有旧 inode，不受影响。
/// Windows：运行中的 exe 不能删除，但可以改名——先把自己改名，再把新文件放到原位，
/// 遗留的 `.old` 文件在下次启动时清理。
fn replace_executable(exe: &Path, bytes: &[u8]) -> Result<()> {
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow!("无法定位安装目录"))?;
    let staged = dir.join(format!("adrama-update-{}.tmp", std::process::id()));

    std::fs::write(&staged, bytes)
        .with_context(|| format!("写入临时文件 {}", staged.display()))?;
    copy_permissions(exe, &staged)?;

    if cfg!(windows) {
        let backup = dir.join(format!(
            "{}.old",
            exe.file_name().and_then(|s| s.to_str()).unwrap_or("adrama")
        ));
        let _ = std::fs::remove_file(&backup);
        std::fs::rename(exe, &backup).with_context(|| {
            format!(
                "无法重命名当前程序（{}），请关闭其它正在运行的实例后重试",
                exe.display()
            )
        })?;
        if let Err(err) = std::fs::rename(&staged, exe) {
            // 放回去，别把用户的程序弄丢了。
            let _ = std::fs::rename(&backup, exe);
            let _ = std::fs::remove_file(&staged);
            return Err(err).with_context(|| format!("写入 {}", exe.display()));
        }
    } else if let Err(err) = std::fs::rename(&staged, exe) {
        let _ = std::fs::remove_file(&staged);
        return Err(err).with_context(|| format!("替换 {}", exe.display()));
    }

    Ok(())
}

#[cfg(unix)]
fn copy_permissions(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(from)
        .map(|m| m.permissions().mode())
        .unwrap_or(0o755);
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(mode | 0o111))
        .context("设置可执行权限")
}

#[cfg(not(unix))]
fn copy_permissions(_from: &Path, _to: &Path) -> Result<()> {
    Ok(())
}

/// 启动时清理上一次更新遗留的备份（Windows）。
pub fn cleanup_leftovers() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with("adrama") && (name.ends_with(".old") || name.ends_with(".tmp")) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// 用新版本重启自己。
pub fn restart(executable: &Path) -> Result<()> {
    std::process::Command::new(executable)
        .spawn()
        .with_context(|| format!("启动 {}", executable.display()))?;
    Ok(())
}

/// 去掉 `v` 前缀。
pub fn normalize_version(tag: &str) -> &str {
    tag.trim().trim_start_matches(['v', 'V'])
}

/// `candidate` 是否比 `current` 新。预发布后缀（`-beta.1`）按「小于正式版」处理。
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        // 解析不了就不要主动提示更新，免得反复弹。
        _ => false,
    }
}

/// `(major, minor, patch, 是否正式版)`；预发布版排在同号正式版之前。
fn parse_version(text: &str) -> Option<(u32, u32, u32, u8)> {
    let text = normalize_version(text);
    let (core, pre) = match text.split_once(['-', '+']) {
        Some((core, _)) => (core, 0u8),
        None => (text, 1u8),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.trim().parse().ok()?;
    let minor = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    let patch = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    Some((major, minor, patch, pre))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(is_newer("v0.3.0", "0.2.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        // 预发布版比同号正式版旧
        assert!(!is_newer("0.3.0-beta.1", "0.3.0"));
        assert!(is_newer("0.3.0", "0.3.0-beta.1"));
        // 解析不了就不提示
        assert!(!is_newer("nightly", "0.2.0"));
    }

    #[test]
    fn release_json_is_parsed() {
        let body = r#"{
            "tag_name": "v0.3.0",
            "name": "my-adrama v0.3.0",
            "body": "更新说明",
            "html_url": "https://github.com/hellomrli/my-adrama/releases/tag/v0.3.0",
            "assets": [
                {"name": "adrama.exe", "browser_download_url": "https://github.com/x/adrama.exe", "size": 123},
                {"name": "adrama-linux-x86_64", "browser_download_url": "https://github.com/x/adrama-linux-x86_64", "size": 456},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://github.com/x/SHA256SUMS.txt", "size": 78}
            ]
        }"#;
        let release = parse_release(body).unwrap();
        assert_eq!(release.version, "0.3.0");
        assert_eq!(release.tag, "v0.3.0");
        assert_eq!(release.assets.len(), 3);
        assert!(release.checksums().is_some());

        let asset = release.asset_for_platform().expect("当前平台有产物");
        assert_eq!(asset.name, platform_asset_name());
    }

    #[test]
    fn missing_platform_asset_is_reported() {
        let body = r#"{"tag_name":"v9.9.9","assets":[{"name":"other.bin","browser_download_url":"https://github.com/x","size":1}]}"#;
        let release = parse_release(body).unwrap();
        assert!(release.asset_for_platform().is_none());
    }

    #[test]
    fn checksum_file_is_parsed() {
        let text = "\
d2a84f4b8b650937ec8f73cd8be2c74add5a911ba64df27458ed8229da804a26  adrama.exe
5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03  adrama-linux-x86_64
";
        assert_eq!(
            expected_digest(text, "adrama-linux-x86_64").as_deref(),
            Some("5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03")
        );
        assert!(expected_digest(text, "missing.bin").is_none());
        // 二进制模式的 `*` 前缀
        assert!(expected_digest(
            "abc  *adrama.exe",
            "adrama.exe"
        )
        .is_none()); // 摘要长度不对，应拒绝
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn only_github_hosts_are_trusted() {
        assert!(check_host("https://github.com/hellomrli/my-adrama/releases/download/v1/adrama").is_ok());
        assert!(check_host("https://objects.githubusercontent.com/x").is_ok());
        assert!(check_host("https://evil.example.com/adrama").is_err());
        assert!(check_host("http://github.com/x").is_err());
        // 用户名混淆：user@evil 的真实主机是 evil
        assert!(check_host("https://github.com@evil.example.com/x").is_err());
    }

    #[test]
    fn replace_executable_swaps_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("adrama");
        std::fs::write(&exe, b"old binary").unwrap();

        replace_executable(&exe, b"new binary").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new binary");
        // 不留临时文件
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn system_paths_are_treated_as_managed() {
        assert!(!InstallKind::Managed(PathBuf::from("/usr/bin/adrama")).can_self_update());
        assert!(InstallKind::Portable(PathBuf::from("/home/u/adrama")).can_self_update());
        assert!(InstallKind::Managed(PathBuf::from("/usr/bin/adrama"))
            .describe()
            .contains("包管理器"));
    }
}
