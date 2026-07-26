//! Texture cache for previews.
//!
//! The old GUI uploaded every generated image at full resolution and never
//! released one, so a 30-shot project pinned hundreds of megabytes of GPU
//! memory. Here images are downscaled to the size actually drawn, evicted by
//! least-recent use, and decoded at most a couple per frame so scrolling a
//! large grid never stalls the frame loop.

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAX_DECODES_PER_FRAME: usize = 2;
/// Rough ceiling on cached pixel data (bytes). ~64 MB.
const BUDGET_BYTES: usize = 64 * 1024 * 1024;

struct Entry {
    texture: TextureHandle,
    bytes: usize,
    modified: Option<SystemTime>,
    max_edge: u32,
    last_used: u64,
}

#[derive(Default)]
pub struct Thumbnails {
    cache: HashMap<PathBuf, Entry>,
    clock: u64,
    decodes_this_frame: usize,
    /// Set when a request was deferred, so the app can ask for another frame.
    pending: bool,
}

impl Thumbnails {
    pub fn begin_frame(&mut self) {
        self.clock += 1;
        self.decodes_this_frame = 0;
        self.pending = false;
    }

    pub fn has_pending(&self) -> bool {
        self.pending
    }

    /// Texture for `path`, downscaled so its longest edge is `max_edge`.
    /// Returns `None` while the decode is deferred or the file is unreadable.
    pub fn get(&mut self, ctx: &egui::Context, path: &Path, max_edge: u32) -> Option<TextureHandle> {
        let modified = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());

        if let Some(entry) = self.cache.get_mut(path) {
            // A regenerated file must not keep showing the old picture.
            if entry.modified == modified && entry.max_edge >= max_edge {
                entry.last_used = self.clock;
                return Some(entry.texture.clone());
            }
        }

        if self.decodes_this_frame >= MAX_DECODES_PER_FRAME {
            self.pending = true;
            // Show the stale texture rather than a hole while we catch up.
            return self.cache.get(path).map(|e| e.texture.clone());
        }
        self.decodes_this_frame += 1;

        let image = decode(path, max_edge)?;
        let bytes = image.width() * image.height() * 4;
        let texture = ctx.load_texture(
            path.to_string_lossy().to_string(),
            image,
            TextureOptions::LINEAR,
        );
        self.cache.insert(
            path.to_path_buf(),
            Entry {
                texture: texture.clone(),
                bytes,
                modified,
                max_edge,
                last_used: self.clock,
            },
        );
        self.evict_if_needed();
        Some(texture)
    }

    /// Drop a specific entry (used after regenerating one item).
    pub fn invalidate(&mut self, path: &Path) {
        self.cache.remove(path);
    }

    fn evict_if_needed(&mut self) {
        let mut total: usize = self.cache.values().map(|e| e.bytes).sum();
        if total <= BUDGET_BYTES {
            return;
        }
        let mut entries: Vec<(PathBuf, u64, usize)> = self
            .cache
            .iter()
            .map(|(k, v)| (k.clone(), v.last_used, v.bytes))
            .collect();
        entries.sort_by_key(|(_, last_used, _)| *last_used);

        for (path, _, bytes) in entries {
            if total <= BUDGET_BYTES {
                break;
            }
            self.cache.remove(&path);
            total = total.saturating_sub(bytes);
        }
    }
}

fn decode(path: &Path, max_edge: u32) -> Option<ColorImage> {
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let (w, h) = (image.width(), image.height());
    let longest = w.max(h);

    let rgba = if longest > max_edge && max_edge > 0 {
        let scale = max_edge as f32 / longest as f32;
        let (nw, nh) = (
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
        );
        image::imageops::thumbnail(&image.to_rgba8(), nw, nh)
    } else {
        image.to_rgba8()
    };

    Some(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}
