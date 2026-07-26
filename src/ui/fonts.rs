//! Load a system CJK font so the Chinese UI renders as text, not tofu.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::path::{Path, PathBuf};

const FAMILY_NAME: &str = "adrama_cjk";

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let Some((path, bytes)) = candidates().into_iter().find_map(load) else {
        tracing::warn!(
            "未找到中文字体，界面可能显示为方块。Linux 安装 fonts-noto-cjk，Windows 自带微软雅黑。"
        );
        ctx.set_fonts(fonts);
        return;
    };

    fonts
        .font_data
        .insert(FAMILY_NAME.to_owned(), FontData::from_owned(bytes).into());
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, FAMILY_NAME.to_owned());
    // Keep the latin monospace first so code/paths stay aligned, with CJK as
    // fallback for any Chinese inside monospace text.
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push(FAMILY_NAME.to_owned());

    tracing::info!("已加载中文字体：{}", path.display());
    ctx.set_fonts(fonts);
}

fn load(path: PathBuf) -> Option<(PathBuf, Vec<u8>)> {
    if !path.is_file() {
        return None;
    }
    match std::fs::read(&path) {
        Ok(bytes) => Some((path, bytes)),
        Err(err) => {
            tracing::warn!("跳过字体 {}：{err}", path.display());
            None
        }
    }
}

fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(custom) = std::env::var("ADRAMA_FONT") {
        paths.push(PathBuf::from(custom));
    }

    if let Some(windir) = std::env::var_os("WINDIR") {
        let dir = Path::new(&windir).join("Fonts");
        for name in ["msyh.ttc", "msyhl.ttc", "simhei.ttf", "simsun.ttc", "msjh.ttc"] {
            paths.push(dir.join(name));
        }
    }

    for path in [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/OTF/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        "/usr/share/fonts/truetype/arphic/uming.ttc",
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ] {
        paths.push(PathBuf::from(path));
    }

    paths
}
