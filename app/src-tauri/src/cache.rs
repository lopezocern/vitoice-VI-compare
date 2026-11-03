use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CACHE_VER: u32 = 2;

#[derive(Serialize, Deserialize)]
pub struct CachedVideo {
    #[serde(default)]
    pub ver: u32,
    pub path: String,
    pub size: u64,
    pub mtime: u64,
    pub width: u32,
    pub height: u32,
    pub duration: f32,
    pub bitrate: u32,
    pub hashes: Vec<u64>,
    pub times: Vec<f32>,
    pub frames: Vec<String>,
}

pub fn dir_for(root: &Path, file: &Path) -> PathBuf {
    root.join(crate::scanner::path_key(file))
}

pub fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 命中条件：size + mtime 未变，且所有帧图仍在。
pub fn load(dir: &Path, file: &Path) -> Option<CachedVideo> {
    let txt = std::fs::read_to_string(dir.join("meta.json")).ok()?;
    let c: CachedVideo = serde_json::from_str(&txt).ok()?;
    let md = std::fs::metadata(file).ok()?;
    if c.ver != CACHE_VER {
        return None;
    }
    if c.size != md.len() || c.mtime != mtime(file) {
        return None;
    }
    if c.frames.is_empty() || c.hashes.is_empty() {
        return None;
    }
    for f in &c.frames {
        if !dir.join(f).exists() {
            return None;
        }
    }
    Some(c)
}

pub fn save(dir: &Path, c: &CachedVideo) {
    if let Ok(t) = serde_json::to_string(c) {
        let _ = std::fs::write(dir.join("meta.json"), t);
    }
}
