//! Load system CJK fonts so Simplified Chinese UI renders correctly.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::fs;
use std::path::Path;

pub fn install_cjk_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let candidates = cjk_font_candidates();
    let mut loaded = false;
    for path in candidates {
        if !path.exists() {
            continue;
        }
        match fs::read(&path) {
            Ok(bytes) => {
                let name = "adrama_cjk";
                fonts
                    .font_data
                    .insert(name.to_owned(), FontData::from_owned(bytes).into());
                // Prefer CJK for proportional + monospace fallback
                fonts
                    .families
                    .entry(FontFamily::Proportional)
                    .or_default()
                    .insert(0, name.to_owned());
                fonts
                    .families
                    .entry(FontFamily::Monospace)
                    .or_default()
                    .push(name.to_owned());
                loaded = true;
                tracing::info!("loaded CJK font: {}", path.display());
                break;
            }
            Err(e) => {
                tracing::warn!("skip font {}: {e}", path.display());
            }
        }
    }

    if !loaded {
        tracing::warn!(
            "no CJK font found; Chinese text may show as boxes. Install fonts-noto-cjk or Microsoft YaHei."
        );
    }

    ctx.set_fonts(fonts);
}

fn cjk_font_candidates() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();

    // Windows
    if let Some(windir) = std::env::var_os("WINDIR") {
        let fonts = Path::new(&windir).join("Fonts");
        for name in [
            "msyh.ttc",   // 微软雅黑
            "msyhbd.ttc",
            "simhei.ttf",
            "simsun.ttc",
            "msjh.ttc",
            "NotoSansCJK-Regular.ttc",
        ] {
            paths.push(fonts.join(name));
        }
    }

    // Linux
    for p in [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/truetype/arphic/uming.ttc",
        "/usr/share/fonts/OTF/NotoSansCJK-Regular.ttc",
    ] {
        paths.push(Path::new(p).to_path_buf());
    }

    // macOS
    for p in [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
    ] {
        paths.push(Path::new(p).to_path_buf());
    }

    // Optional override
    if let Ok(custom) = std::env::var("ADRAMA_FONT") {
        paths.insert(0, Path::new(&custom).to_path_buf());
    }

    paths
}
