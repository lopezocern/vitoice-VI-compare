use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppConfig {
    pub cache_dir: String,
    pub trash_dir: String,
    pub thumb_size: u32,
    pub workers: usize,
    pub default_sim: u8,
}

impl Default for AppConfig {
    fn default() -> Self {
        let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
        let root = PathBuf::from(&base).join("VitoiceCompare");
        Self {
            cache_dir: root.join("cache").to_string_lossy().to_string(),
            trash_dir: root.join("trash").to_string_lossy().to_string(),
            thumb_size: 720,
            workers: 0,
            default_sim: 85,
        }
    }
}

fn root_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("VideoCC")
}

fn config_path() -> PathBuf {
    root_dir().join("config.json")
}

static CFG: OnceLock<Mutex<AppConfig>> = OnceLock::new();

fn store() -> &'static Mutex<AppConfig> {
    CFG.get_or_init(|| {
        let mut cfg = std::fs::read_to_string(config_path())
            .ok()
            .and_then(|t| serde_json::from_str::<AppConfig>(&t).ok())
            .unwrap_or_default();

        // 环境变量优先（测试 / 便携用法）
        if let Ok(v) = std::env::var("VIDEOCC_CACHE_DIR") {
            cfg.cache_dir = v;
        }
        if let Ok(v) = std::env::var("VIDEOCC_TRASH_ROOT") {
            cfg.trash_dir = v;
        }
        if let Ok(v) = std::env::var("VIDEOCC_THUMB_SIZE") {
            if let Ok(n) = v.parse::<u32>() {
                cfg.thumb_size = n.clamp(240, 1600);
            }
        }
        if let Ok(v) = std::env::var("VIDEOCC_WORKERS") {
            if let Ok(n) = v.parse::<usize>() {
                cfg.workers = n;
            }
        }
        Mutex::new(cfg)
    })
}

pub fn current() -> AppConfig {
    store().lock().map(|c| c.clone()).unwrap_or_default()
}

pub fn save(cfg: &AppConfig) -> Result<(), String> {
    let p = config_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let txt = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&p, txt).map_err(|e| format!("写入配置失败: {e}"))?;
    if let Ok(mut c) = store().lock() {
        *c = cfg.clone();
    }
    Ok(())
}

pub fn cache_dir() -> PathBuf {
    PathBuf::from(current().cache_dir)
}

pub fn trash_dir() -> PathBuf {
    PathBuf::from(current().trash_dir)
}

pub fn thumb_size() -> u32 {
    current().thumb_size.clamp(240, 1600)
}

pub fn workers() -> usize {
    let configured = current().workers;
    if configured > 0 {
        return configured.clamp(1, 32);
    }
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (cores / 2).clamp(2, 8)
}

pub fn default_sim() -> u8 {
    current().default_sim
}

/// 缓存目录的占用统计
pub fn cache_stats() -> (u64, u64) {
    let dir = cache_dir();
    let mut files = 0u64;
    let mut bytes = 0u64;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Ok(inner) = std::fs::read_dir(&p) {
                    for f in inner.flatten() {
                        if let Ok(m) = f.metadata() {
                            files += 1;
                            bytes += m.len();
                        }
                    }
                }
            } else if let Ok(m) = p.metadata() {
                files += 1;
                bytes += m.len();
            }
        }
    }
    (files, bytes)
}

pub fn clear_cache() -> Result<u64, String> {
    let dir = cache_dir();
    let mut removed = 0u64;
    let rd = std::fs::read_dir(&dir).map_err(|e| format!("读取缓存目录失败: {e}"))?;
    for e in rd.flatten() {
        let p = e.path();
        let ok = if p.is_dir() {
            std::fs::remove_dir_all(&p).is_ok()
        } else {
            std::fs::remove_file(&p).is_ok()
        };
        if ok {
            removed += 1;
        }
    }
    Ok(removed)
}
