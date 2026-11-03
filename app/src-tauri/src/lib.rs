mod audio;
mod cache;
mod config;
mod images;
mod jobs;
mod logging;
pub mod media;
mod model;
pub mod scanner;
mod tools;

use base64::Engine;
use model::*;
use scanner::VideoRecord;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

/// 一次扫描的完整结果，按工具分别保存，切换工具时直接复用
#[derive(Default, Clone)]
pub struct ToolSnapshot {
    pub records: Vec<VideoRecord>,
    pub groups: Vec<(Vec<usize>, u8)>,
    pub items: Vec<Item>,
    /// 产生这份结果时的扫描范围签名
    pub sig: String,
}

pub struct AppState {
    results: Mutex<std::collections::HashMap<String, ToolSnapshot>>,
    scope_sig: Mutex<String>,
    tool: Mutex<String>,
    trashed: Mutex<std::collections::HashSet<String>>,
    scanning: AtomicBool,
    cancel: Arc<AtomicBool>,
    job_running: AtomicBool,
    job_cancel: Arc<AtomicBool>,
}

fn trash_root() -> PathBuf {
    config::trash_dir()
}

fn trash_core(
    paths: &[String],
    trashed: &Mutex<std::collections::HashSet<String>>,
) -> (String, usize, u64) {
    let batch = now_ms().to_string();
    let dir = trash_root().join(&batch);
    let _ = std::fs::create_dir_all(&dir);

    let mut entries: Vec<TrashEntry> = Vec::new();
    let mut moved = 0usize;
    let mut bytes = 0u64;

    for (i, p) in paths.iter().enumerate() {
        let src = PathBuf::from(p);
        if !src.exists() {
            continue;
        }
        let name = src.file_name().and_then(|x| x.to_str()).unwrap_or("file");
        let dst = dir.join(format!("{i:04}_{name}"));
        let size = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);

        let ok = std::fs::rename(&src, &dst).is_ok() || {
            std::fs::copy(&src, &dst).is_ok() && std::fs::remove_file(&src).is_ok()
        };
        if !ok {
            continue;
        }

        if let Ok(mut t) = trashed.lock() {
            t.insert(p.clone());
        }
        entries.push(TrashEntry {
            src: p.clone(),
            dst: dst.to_string_lossy().to_string(),
            size,
        });
        moved += 1;
        bytes += size;
    }

    let manifest = TrashManifest {
        batch: batch.clone(),
        created_ms: now_ms(),
        entries,
    };
    if let Ok(txt) = serde_json::to_string_pretty(&manifest) {
        let _ = std::fs::write(dir.join("manifest.json"), txt);
    }

    (batch, moved, bytes)
}

fn undo_core(batch: &str, trashed: &Mutex<std::collections::HashSet<String>>) -> usize {
    let dir = trash_root().join(batch);
    let Ok(txt) = std::fs::read_to_string(dir.join("manifest.json")) else {
        return 0;
    };
    let Ok(manifest) = serde_json::from_str::<TrashManifest>(&txt) else {
        return 0;
    };

    let mut restored = 0usize;
    for e in &manifest.entries {
        let dst = PathBuf::from(&e.dst);
        let src = PathBuf::from(&e.src);
        if !dst.exists() {
            continue;
        }
        if let Some(parent) = src.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let ok = std::fs::rename(&dst, &src).is_ok() || {
            std::fs::copy(&dst, &src).is_ok() && std::fs::remove_file(&dst).is_ok()
        };
        if ok {
            restored += 1;
            if let Ok(mut t) = trashed.lock() {
                t.remove(&e.src);
            }
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
    restored
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct TrashEntry {
    pub src: String,
    pub dst: String,
    pub size: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct TrashManifest {
    pub batch: String,
    pub created_ms: u64,
    pub entries: Vec<TrashEntry>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn thumb_root() -> PathBuf {
    config::cache_dir()
}

fn data_url(p: &Path) -> String {
    match std::fs::read(p) {
        Ok(bytes) => format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ),
        Err(_) => String::new(),
    }
}

#[derive(serde::Serialize, Clone)]
pub struct ToolMeta {
    pub id: &'static str,
    pub name: &'static str,
    pub glyph: &'static str,
    pub group: &'static str,
    pub implemented: bool,
}

#[tauri::command]
fn tool_list() -> Vec<ToolMeta> {
    let mk = |id: &'static str, name: &'static str, glyph: &'static str, group: &'static str| ToolMeta {
        id,
        name,
        glyph,
        group,
        implemented: tools::IMPLEMENTED.contains(&id),
    };
    vec![
        mk("duplicates",       "重复文件",       "⧉", "basic"),
        mk("empty-folders",    "空文件夹",       "▤", "basic"),
        mk("biggest-files",    "大文件",         "⇲", "basic"),
        mk("empty-files",      "空文件",         "◌", "basic"),
        mk("temporary",        "临时文件",       "⏱", "basic"),
        mk("similar-images",   "相似图像",       "▣", "media"),
        mk("similar-videos",   "相似视频",       "▶", "media"),
        mk("same-music",       "重复音乐",       "♫", "media"),
        mk("invalid-symlinks", "无效的符号链接", "⤳", "advanced"),
        mk("broken-files",     "损坏的文件",     "⚠", "advanced"),
        mk("bad-extensions",   "错误的扩展名",   "⌥", "advanced"),
        mk("bad-names",        "不规范名称",     "Aa", "advanced"),
        mk("exif-remover",     "EXIF 移除器",    "⛨", "advanced"),
        mk("video-optimizer",  "视频优化器",     "⚙", "advanced"),
    ]
}

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
fn job_preview(
    state: State<'_, AppState>,
    tool: String,
    includes: Vec<String>,
    excludes: Vec<String>,
    recursive: Option<bool>,
) -> Result<jobs::JobPreview, String> {
    let _ = state;
    let roots: Vec<PathBuf> = includes
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect();
    if roots.is_empty() {
        return Err("没有可用的扫描目录".to_string());
    }
    let excludes: Vec<String> = excludes
        .iter()
        .map(|e| e.replace('/', "\\").to_lowercase().trim_end_matches('\\').to_string())
        .collect();
    Ok(jobs::preview(&tool, &roots, &excludes, recursive.unwrap_or(true)))
}

#[tauri::command]
fn job_start(
    app: AppHandle,
    state: State<'_, AppState>,
    tool: String,
    preset: String,
    files: Vec<jobs::JobFile>,
    first_root: String,
) -> Result<(), String> {
    if state.job_running.swap(true, Ordering::SeqCst) {
        return Err("已有任务在执行".to_string());
    }
    state.job_cancel.store(false, Ordering::SeqCst);
    let cancel = state.job_cancel.clone();
    jobs::run(app, tool, preset, files, PathBuf::from(first_root), cancel);
    Ok(())
}

#[tauri::command]
fn job_cancel(state: State<'_, AppState>) {
    state.job_cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn job_finish(state: State<'_, AppState>) {
    state.job_running.store(false, Ordering::SeqCst);
}

#[tauri::command]
fn job_undo(out_dir: String) -> Result<usize, String> {
    jobs::undo(&PathBuf::from(out_dir))
}

#[tauri::command]
fn get_config() -> config::AppConfig {
    config::current()
}

#[tauri::command]
fn set_config(cfg: config::AppConfig) -> Result<config::AppConfig, String> {
    let mut c = cfg;
    c.thumb_size = c.thumb_size.clamp(240, 1600);
    c.workers = c.workers.min(32);
    c.default_sim = c.default_sim.clamp(50, 100);
    config::save(&c)?;
    Ok(config::current())
}

#[tauri::command]
fn cache_stats() -> serde_json::Value {
    let (files, bytes) = config::cache_stats();
    serde_json::json!({ "files": files, "bytes": bytes, "dir": config::current().cache_dir })
}

#[tauri::command]
fn clear_cache() -> Result<serde_json::Value, String> {
    let removed = config::clear_cache()?;
    let (files, bytes) = config::cache_stats();
    Ok(serde_json::json!({ "removed": removed, "files": files, "bytes": bytes }))
}

#[tauri::command]
fn frame_data_url(path: String, at: f64, width: Option<u32>) -> Result<String, String> {
    let w = width.unwrap_or(1280).clamp(320, 2560);
    let vf = format!("scale='min({w},iw)':-2");
    let out = crate::media::cmd("ffmpeg")
        .args([
            "-hide_banner", "-loglevel", "error",
            "-ss", &format!("{:.3}", at.max(0.0)),
            "-i", &path,
            "-frames:v", "1",
            "-vf", &vf,
            "-q:v", "3",
            "-f", "image2pipe", "-vcodec", "mjpeg", "-",
        ])
        .output()
        .map_err(|e| format!("ffmpeg 启动失败: {e}"))?;

    if !out.status.success() || out.stdout.is_empty() {
        return Err("抽帧失败".to_string());
    }
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(out.stdout)
    ))
}

#[tauri::command]
fn file_data_url(path: String) -> Result<String, String> {
    let p = PathBuf::from(&path);
    let md = std::fs::metadata(&p).map_err(|e| format!("读取失败: {e}"))?;
    if md.len() > 40 * 1024 * 1024 {
        return Err("文件过大（>40MB），请用系统播放器查看".to_string());
    }
    let bytes = std::fs::read(&p).map_err(|e| format!("读取失败: {e}"))?;
    let mime = match p
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/jpeg",
    };
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        std::fs::create_dir_all(&p).map_err(|e| format!("目录不存在且创建失败: {e}"))?;
    }
    crate::media::cmd("explorer")
        .arg(p.to_string_lossy().to_string())
        .spawn()
        .map_err(|e| format!("无法打开资源管理器: {e}"))?;
    Ok(())
}

#[tauri::command]
fn open_logs() -> Result<String, String> {
    let dir = crate::logging::log_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建日志目录失败: {e}"))?;
    crate::media::cmd("explorer")
        .arg(dir.to_string_lossy().to_string())
        .spawn()
        .map_err(|e| format!("无法打开资源管理器: {e}"))?;
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn dev_flags() -> serde_json::Value {
    let scope_hint = std::env::var("VIDEOCC_SCOPE").ok().map(|s| {
        let mut parts = s.split('|');
        let inc: Vec<String> = parts
            .next()
            .unwrap_or("")
            .split(';')
            .filter(|x| !x.is_empty())
            .map(String::from)
            .collect();
        let exc: Vec<String> = parts
            .next()
            .unwrap_or("")
            .split(';')
            .filter(|x| !x.is_empty())
            .map(String::from)
            .collect();
        serde_json::json!({
            "includes": inc,
            "excludes": exc,
            "recursive": std::env::var("VIDEOCC_RECURSIVE").map(|v| v != "0").unwrap_or(true),
        })
    });

    serde_json::json!({
        "autocompare": std::env::var("VIDEOCC_AUTOCOMPARE").is_ok(),
        "auto_trash": std::env::var("VIDEOCC_AUTO_TRASH").is_ok(),
        "auto_trash_view": std::env::var("VIDEOCC_AUTO_TRASH_VIEW").is_ok(),
        "autoscan": std::env::var("VIDEOCC_AUTOSCAN").is_ok(),
        "open_settings": std::env::var("VIDEOCC_OPEN_SETTINGS").is_ok(),
        "open_about": std::env::var("VIDEOCC_OPEN_ABOUT").is_ok(),
        "preview": std::env::var("VIDEOCC_PREVIEW").ok(),
        "purge": std::env::var("VIDEOCC_PURGE").ok(),
        "open_trash": std::env::var("VIDEOCC_OPEN_TRASH").is_ok(),
        "diag_keepall": std::env::var("VIDEOCC_DIAG_KEEPALL").is_ok(),
        "diag_cancel_ms": std::env::var("VIDEOCC_DIAG_CANCEL").ok().and_then(|v| v.parse::<u64>().ok()),
        "diag_flow": std::env::var("VIDEOCC_DIAG_FLOW").is_ok(),
        "diag_sel": std::env::var("VIDEOCC_DIAG_SEL").is_ok(),
        "diag_close": std::env::var("VIDEOCC_DIAG_CLOSE").is_ok(),
        "compare": std::env::var("VIDEOCC_COMPARE").is_ok(),
        "diag_switch": std::env::var("VIDEOCC_DIAG_SWITCH").is_ok(),
        "job_tool": std::env::var("VIDEOCC_JOB_TOOL").ok(),
        "job_start": std::env::var("VIDEOCC_JOB_START").is_ok(),
        "scope_hint": scope_hint,
        "demo_pos": std::env::var("VIDEOCC_DEMO_POS").ok()
            .and_then(|v| v.parse::<u32>().ok()),
    })
}

fn worker_count() -> usize {
    config::workers()
}

fn process_one(file: &Path, cancel: &AtomicBool) -> Option<VideoRecord> {
    let dir = thumb_root().join(scanner::path_key(file));

    if let Some(c) = cache::load(&dir, file) {
        return Some(VideoRecord {
            path: file.to_path_buf(),
            meta: media::Probe {
                width: c.width,
                height: c.height,
                duration: c.duration,
                bitrate: c.bitrate,
                size: c.size,
            },
            hashes: c.hashes,
            frames: c.frames.iter().map(|f| dir.join(f)).collect(),
            times: c.times,
        });
    }

    let meta = media::probe(file).ok()?;
    let ex = media::extract(file, &dir, meta.duration, 8, 48, 2.0, Some(cancel)).ok()?;

    let names: Vec<String> = ex
        .frames
        .iter()
        .filter_map(|p| p.file_name().and_then(|x| x.to_str()).map(|s| s.to_string()))
        .collect();

    cache::save(
        &dir,
        &cache::CachedVideo {
            ver: cache::CACHE_VER,
            path: file.to_string_lossy().to_string(),
            size: meta.size,
            mtime: cache::mtime(file),
            width: meta.width,
            height: meta.height,
            duration: meta.duration,
            bitrate: meta.bitrate,
            hashes: ex.hashes.clone(),
            times: ex.times.clone(),
            frames: names,
        },
    );

    Some(VideoRecord {
        path: file.to_path_buf(),
        meta,
        hashes: ex.hashes,
        frames: ex.frames,
        times: ex.times,
    })
}

fn spawn_scan(app2: AppHandle, roots: Vec<PathBuf>, excludes: Vec<String>, recursive: bool, sim: u8, cancel: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        crate::logging::log("[scan] thread start");
        let started = Instant::now();

        let files = scanner::collect_videos_scoped(&roots, &excludes, recursive);
        let total = files.len();

        let workers = worker_count();
        let next = AtomicUsize::new(0);
        let done = AtomicUsize::new(0);
        let last_emit = Mutex::new(Instant::now() - Duration::from_secs(5));

        let mut slots: Vec<Option<VideoRecord>> = Vec::new();
        slots.resize_with(total, || None);

        let (tx, rx) = std::sync::mpsc::channel::<(usize, VideoRecord)>();

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let tx = tx.clone();
                let app = app2.clone();
                let cancel = cancel.clone();
                let files = &files;
                let next = &next;
                let done = &done;
                let last_emit = &last_emit;

                scope.spawn(move || loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= files.len() || cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Some(rec) = process_one(&files[i], &cancel) {
                        let _ = tx.send((i, rec));
                    }
                    done.fetch_add(1, Ordering::Relaxed);

                    let should_emit = last_emit
                        .lock()
                        .map(|mut t| {
                            if t.elapsed() >= Duration::from_millis(120) {
                                *t = Instant::now();
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);

                    if should_emit {
                        let name = files[i]
                            .file_name()
                            .and_then(|x| x.to_str())
                            .unwrap_or("")
                            .to_string();
                        let _ = app.emit(
                            "scan://progress",
                            ScanProgress {
                                done: done.load(Ordering::Relaxed),
                                total,
                                current: name,
                            },
                        );
                    }
                });
            }
            drop(tx);

            for (i, rec) in rx {
                if i < slots.len() {
                    slots[i] = Some(rec);
                }
            }
        });

        let records: Vec<VideoRecord> = slots.into_iter().flatten().collect();

        let _ = app2.emit(
            "scan://progress",
            ScanProgress { done: total, total, current: String::new() },
        );

        let groups = scanner::group_records(&records, sim);
        let matched_files: usize = groups.iter().map(|(m, _)| m.len()).sum();

        let st2 = app2.state::<AppState>();
        if let Ok(mut m) = st2.results.lock() {
            m.insert(
                "similar-videos".to_string(),
                ToolSnapshot { records: records.clone(), groups: groups.clone(), items: Vec::new(), sig: sig_now(&st2) },
            );
        }
        if let Ok(mut t) = st2.tool.lock() {
            *t = "similar-videos".to_string();
        }
        app2.state::<AppState>().scanning.store(false, Ordering::SeqCst);

        let _ = app2.emit(
            "scan://finished",
            ScanSummary {
                groups: groups.len(),
                files: total,
                matched_files,
                elapsed_ms: started.elapsed().as_millis() as u64,
            },
        );

        crate::logging::log(&format!(
            "[scan] files={} groups={} matched={} elapsed={:.1}s workers={}",
            total,
            groups.len(),
            matched_files,
            started.elapsed().as_secs_f64(),
            workers
        ));
    });
}

fn spawn_tool_scan(app: AppHandle, roots: Vec<PathBuf>, excludes: Vec<String>, recursive: bool, tool: String, cancel: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let started = Instant::now();
        let items = if tool == "same-music" {
            crate::audio::run(&roots, &excludes, recursive)
        } else {
            tools::run(&tool, &roots, &excludes, recursive, &cancel)
        };
        let st = app.state::<AppState>();
        if let Ok(mut m) = st.results.lock() {
            m.insert(
                tool.clone(),
                ToolSnapshot { records: Vec::new(), groups: Vec::new(), items: items.clone(), sig: sig_now(&st) },
            );
        }
        if let Ok(mut t) = st.tool.lock() {
            *t = tool.clone();
        }
        st.scanning.store(false, Ordering::SeqCst);

        let _ = app.emit(
            "scan://finished",
            serde_json::json!({
                "tool": tool,
                "groups": 0,
                "files": items.len(),
                "matched_files": items.len(),
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );

        crate::logging::log(&format!(
            "[tool] {} items={} elapsed={:.1}s",
            tool,
            items.len(),
            started.elapsed().as_secs_f64()
        ));
    });
}

fn spawn_image_scan(
    app: AppHandle,
    roots: Vec<PathBuf>,
    excludes: Vec<String>,
    recursive: bool,
    sim: u8,
    cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let started = Instant::now();
        let files = images::collect(&roots, &excludes, recursive);
        let total = files.len();

        let workers = worker_count();
        let next = AtomicUsize::new(0);
        let done = AtomicUsize::new(0);
        let last_emit = Mutex::new(Instant::now() - Duration::from_secs(5));

        let mut slots: Vec<Option<VideoRecord>> = Vec::new();
        slots.resize_with(total, || None);

        let (tx, rx) = std::sync::mpsc::channel::<(usize, VideoRecord)>();

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let tx = tx.clone();
                let app = app.clone();
                let cancel = cancel.clone();
                let files = &files;
                let next = &next;
                let done = &done;
                let last_emit = &last_emit;

                scope.spawn(move || loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= files.len() || cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Some(r) = images::process(&files[i]) {
                        let _ = tx.send((i, r));
                    }
                    done.fetch_add(1, Ordering::Relaxed);

                    let should_emit = last_emit
                        .lock()
                        .map(|mut t| {
                            if t.elapsed() >= Duration::from_millis(150) {
                                *t = Instant::now();
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);

                    if should_emit {
                        let name = files[i]
                            .file_name()
                            .and_then(|x| x.to_str())
                            .unwrap_or("")
                            .to_string();
                        let _ = app.emit(
                            "scan://progress",
                            ScanProgress {
                                done: done.load(Ordering::Relaxed),
                                total,
                                current: name,
                            },
                        );
                    }
                });
            }
            drop(tx);
            for (i, r) in rx {
                if i < slots.len() {
                    slots[i] = Some(r);
                }
            }
        });

        let records: Vec<VideoRecord> = slots.into_iter().flatten().collect();

        let groups = images::group(&records, sim);
        let matched: usize = groups.iter().map(|(m, _)| m.len()).sum();

        let st = app.state::<AppState>();
        if let Ok(mut m) = st.results.lock() {
            m.insert(
                "similar-images".to_string(),
                ToolSnapshot { records: records.clone(), groups: groups.clone(), items: Vec::new(), sig: sig_now(&st) },
            );
        }
        if let Ok(mut t) = st.tool.lock() {
            *t = "similar-images".to_string();
        }
        st.scanning.store(false, Ordering::SeqCst);

        let _ = app.emit(
            "scan://finished",
            ScanSummary {
                groups: groups.len(),
                files: total,
                matched_files: matched,
                elapsed_ms: started.elapsed().as_millis() as u64,
            },
        );

        crate::logging::log(&format!(
            "[images] files={} groups={} matched={} elapsed={:.1}s",
            total,
            groups.len(),
            matched,
            started.elapsed().as_secs_f64()
        ));
    });
}

#[tauri::command]
fn scan_start(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    sim: Option<u8>,
    tool: Option<String>,
) -> Result<(), String> {
    scan_start_scope(app, state, vec![path], vec![], sim, tool, None)
}

#[tauri::command]
fn scan_start_scope(
    app: AppHandle,
    state: State<'_, AppState>,
    includes: Vec<String>,
    excludes: Vec<String>,
    sim: Option<u8>,
    tool: Option<String>,
    recursive: Option<bool>,
) -> Result<(), String> {
    if state.scanning.swap(true, Ordering::SeqCst) {
        return Err("已有扫描在进行中".to_string());
    }
    state.cancel.store(false, Ordering::SeqCst);

    let recursive = recursive.unwrap_or(true);
    crate::logging::log(&format!(
        "[scan] start tool={} sim={:?} roots={} recursive={}",
        tool.clone().unwrap_or_else(|| "similar-videos".into()),
        sim,
        includes.len(),
        recursive
    ));
    let roots: Vec<PathBuf> = includes
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect();

    if roots.is_empty() {
        state.scanning.store(false, Ordering::SeqCst);
        return Err("没有可扫描的有效目录".to_string());
    }

    let excludes: Vec<String> = excludes
        .iter()
        .map(|e| e.replace('/', "\\").to_lowercase().trim_end_matches('\\').to_string())
        .collect();

    let tool = tool.unwrap_or_else(|| "similar-videos".to_string());
    if let Ok(mut t) = state.tool.lock() {
        *t = tool.clone();
    }

    // 记录本次范围；旧工具的 result 是否过期由 has_result 按签名判断
    let sig = format!("{includes:?}|{excludes:?}|{recursive}");
    if let Ok(mut s) = state.scope_sig.lock() {
        *s = sig;
    }

    if tool == "similar-images" {
        let cancel = state.cancel.clone();
        spawn_image_scan(app, roots, excludes, recursive, crate::config::default_sim(), cancel);
        return Ok(());
    }

    if tool != "similar-videos" {
        spawn_tool_scan(app, roots, excludes, recursive, tool, state.cancel.clone());
        return Ok(());
    }

    let cancel = state.cancel.clone();
    spawn_scan(app, roots, excludes, recursive, sim.unwrap_or(88).clamp(50, 100), cancel);
    Ok(())
}

#[tauri::command]
fn pick_folders(app: AppHandle) -> Vec<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .blocking_pick_folders()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string())
        .collect()
}

#[tauri::command]
fn check_paths(paths: Vec<String>) -> Vec<bool> {
    paths.iter().map(|p| PathBuf::from(p).is_dir()).collect()
}

/// 工具 id → 快照，缺省用当前工具
fn snapshot_of(state: &AppState, tool: Option<String>) -> ToolSnapshot {
    let key = tool.unwrap_or_else(|| {
        state
            .tool
            .lock()
            .map(|t| t.clone())
            .unwrap_or_else(|_| "similar-videos".to_string())
    });
    state
        .results
        .lock()
        .ok()
        .and_then(|m| m.get(&key).cloned())
        .unwrap_or_default()
}

/// 某工具是否已经有扫描结果
fn sig_now(state: &AppState) -> String {
    state
        .scope_sig
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default()
}

#[tauri::command]
fn has_result(state: State<'_, AppState>, tool: String) -> bool {
    let cur = sig_now(&state);
    state
        .results
        .lock()
        .map(|m| m.get(&tool).map(|s| s.sig == cur).unwrap_or(false))
        .unwrap_or(false)
}

#[tauri::command]
fn get_items(state: State<'_, AppState>, tool: Option<String>) -> Vec<Item> {
    snapshot_of(&state, tool).items
}

#[tauri::command]
fn get_tool(state: State<'_, AppState>) -> String {
    state.tool.lock().map(|v| v.clone()).unwrap_or_default()
}

#[tauri::command]
fn scan_cancel(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn is_scanning(state: State<'_, AppState>) -> bool {
    state.scanning.load(Ordering::SeqCst)
}

#[tauri::command]
fn get_groups(state: State<'_, AppState>, tool: Option<String>) -> Result<Vec<GroupSummary>, String> {
    let snap = snapshot_of(&state, tool);
    let groups = &snap.groups;
    let records = &snap.records;
    let trashed = state.trashed.lock().map_err(|_| "状态锁失败")?;

    let alive_of = |members: &[usize]| -> Vec<usize> {
        members
            .iter()
            .copied()
            .filter(|&m| !trashed.contains(&records[m].path.to_string_lossy().to_string()))
            .collect()
    };

    let mut out = Vec::with_capacity(groups.len());
    for (idx, (members, sim)) in groups.iter().enumerate() {
        let alive = alive_of(members);
        if alive.len() < 2 {
            continue;
        }
        let first = &records[alive[0]];
        let covers: Vec<String> = first
            .frames
            .iter()
            .take(3)
            .map(|p| data_url(p))
            .collect();
        let total_bytes = alive
            .iter()
            .map(|&m| records[m].meta.size)
            .sum::<u64>();
        let names: Vec<String> = alive
            .iter()
            .take(8)
            .map(|&m| {
                records[m]
                    .path
                    .file_name()
                    .and_then(|x| x.to_str())
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        out.push(GroupSummary {
            id: idx as u64,
            sim: *sim,
            file_count: alive.len(),
            total_bytes,
            covers,
            names,
        });
    }
    Ok(out)
}

#[tauri::command]
fn group_detail(state: State<'_, AppState>, id: u64, tool: Option<String>) -> Result<GroupDetail, String> {
    let snap = snapshot_of(&state, tool);
    let groups = &snap.groups;
    let records = &snap.records;
    let trashed = state.trashed.lock().map_err(|_| "状态锁失败")?;

    let (all_members, sim) = groups
        .get(id as usize)
        .ok_or_else(|| format!("组 {id} 不存在"))?;

    let members: Vec<usize> = all_members
        .iter()
        .copied()
        .filter(|&m| !trashed.contains(&records[m].path.to_string_lossy().to_string()))
        .collect();

    if members.is_empty() {
        return Err("该组已全部清理".to_string());
    }

    // 保留项：分辨率 × 码率 × 时长 最大
    let mut best = 0usize;
    let mut best_score: f64 = -1.0;
    for (i, &m) in members.iter().enumerate() {
        let r = &records[m];
        let s = (r.meta.width as f64) * (r.meta.height as f64) * (r.meta.bitrate.max(1) as f64)
            * (r.meta.duration.max(1.0) as f64);
        if s > best_score {
            best_score = s;
            best = i;
        }
    }

    let mut files = Vec::with_capacity(members.len());
    for (i, &m) in members.iter().enumerate() {
        let r = &records[m];
        let frames = r
            .frames
            .iter()
            .enumerate()
            .map(|(k, p)| FrameRef {
                url: data_url(p),
                at: r.times.get(k).copied().unwrap_or(0.0),
            })
            .collect();
        files.push(FileEntry {
            path: r.path.to_string_lossy().to_string(),
            name: r
                .path
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_string(),
            width: r.meta.width,
            height: r.meta.height,
            bitrate: r.meta.bitrate,
            duration: r.meta.duration,
            size: r.meta.size,
            keep: i == best,
            frames,
        });
    }

    let base = &records[members[0]].hashes;
    let mut evidence = Evidence {
        matched_frames: 0,
        total_frames: 0,
        longest_run: 0,
        avg_hamming: 64.0,
    };
    let mut best_rate = -1.0f32;
    for &m in members.iter().skip(1) {
        let s = scanner::evidence_for(base, &records[m].hashes);
        if s.total == 0 {
            continue;
        }
        let rate = s.matched as f32 / s.total as f32;
        if rate > best_rate {
            best_rate = rate;
            evidence = Evidence {
                matched_frames: s.matched,
                total_frames: s.total,
                longest_run: s.longest_run,
                avg_hamming: s.avg_hamming,
            };
        }
    }

    let total_bytes = members.iter().map(|&m| records[m].meta.size).sum();
    let group = GroupSummary {
        id,
        sim: *sim,
        file_count: members.len(),
        total_bytes,
        names: files.iter().take(8).map(|f| f.name.clone()).collect(),
        covers: files
            .first()
            .map(|f| f.frames.iter().take(3).map(|x| x.url.clone()).collect())
            .unwrap_or_default(),
    };

    Ok(GroupDetail { group, files, evidence })
}

#[tauri::command]
fn trash_files(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<serde_json::Value, String> {
    let (batch, moved, bytes) = trash_core(&paths, &state.trashed);
    crate::logging::log(&format!("[trash] batch={batch} moved={moved} bytes={bytes}"));
    Ok(serde_json::json!({
        "batch": batch,
        "moved": moved,
        "bytes": bytes,
    }))
}

#[tauri::command]
fn undo_batch(state: State<'_, AppState>, batch: String) -> Result<serde_json::Value, String> {
    let dir = trash_root().join(&batch);
    if !dir.join("manifest.json").exists() {
        return Err(format!("批次 {batch} 不存在"));
    }
    let restored = undo_core(&batch, &state.trashed);
    Ok(serde_json::json!({ "restored": restored }))
}

#[tauri::command]
fn purge_batch(batch: String) -> Result<serde_json::Value, String> {
    let dir = trash_root().join(&batch);
    if !dir.exists() {
        return Err(format!("批次 {batch} 不存在"));
    }
    let bytes = dir_size(&dir);
    std::fs::remove_dir_all(&dir).map_err(|e| format!("删除失败: {e}"))?;
    crate::logging::log(&format!("[purge] batch={batch} bytes={bytes}"));
    Ok(serde_json::json!({ "bytes": bytes }))
}

#[tauri::command]
fn purge_all() -> Result<serde_json::Value, String> {
    let root = trash_root();
    let mut files = 0usize;
    let mut bytes = 0u64;
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                bytes += dir_size(&p);
                if std::fs::remove_dir_all(&p).is_ok() {
                    files += 1;
                }
            }
        }
    }
    crate::logging::log(&format!("[purge-all] batches={files} bytes={bytes}"));
    Ok(serde_json::json!({ "batches": files, "bytes": bytes }))
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(m) = p.metadata() {
                total += m.len();
            }
        }
    }
    total
}

#[tauri::command]
fn trash_stats() -> serde_json::Value {
    let root = trash_root();
    let mut batches = 0usize;
    let mut count = 0usize;
    let mut bytes = 0u64;
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            batches += 1;
            let size = dir_size(&p);
            bytes += size;
            if let Ok(txt) = std::fs::read_to_string(p.join("manifest.json")) {
                if let Ok(m) = serde_json::from_str::<TrashManifest>(&txt) {
                    count += m.entries.len();
                }
            }
        }
    }
    serde_json::json!({ "batches": batches, "count": count, "bytes": bytes, "dir": root.to_string_lossy() })
}

#[tauri::command]
fn list_batches() -> Vec<serde_json::Value> {
    let root = trash_root();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let Ok(txt) = std::fs::read_to_string(p.join("manifest.json")) else {
                continue;
            };
            let Ok(m) = serde_json::from_str::<TrashManifest>(&txt) else {
                continue;
            };
            let bytes: u64 = m.entries.iter().map(|x| x.size).sum();
            out.push(serde_json::json!({
                "batch": m.batch,
                "created_ms": m.created_ms,
                "count": m.entries.len(),
                "bytes": bytes,
            }));
        }
    }
    out.sort_by_key(|v| std::cmp::Reverse(v["created_ms"].as_u64().unwrap_or(0)));
    out
}

#[tauri::command]
fn open_in_explorer(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err("文件不存在".to_string());
    }
    crate::media::cmd("explorer")
        .arg(format!("/select,{}", p.to_string_lossy()))
        .spawn()
        .map_err(|e| format!("无法打开资源管理器: {e}"))?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init();
    tauri::Builder::default()
        .manage(AppState {
            results: Mutex::new(std::collections::HashMap::new()),
            scope_sig: Mutex::new(String::new()),
            tool: Mutex::new("similar-videos".to_string()),
            trashed: Mutex::new(std::collections::HashSet::new()),
            scanning: AtomicBool::new(false),
            cancel: Arc::new(AtomicBool::new(false)),
            job_running: AtomicBool::new(false),
            job_cancel: Arc::new(AtomicBool::new(false)),
        })
        .setup(|app| {
            // 自检：隔离 / 恢复逻辑（不碰真实素材）
            if std::env::var("VIDEOCC_SELFTEST").is_ok() {
                let st = app.state::<AppState>();
                let tmp = std::env::temp_dir().join("vcc_selftest");
                let _ = std::fs::remove_dir_all(&tmp);
                let _ = std::fs::create_dir_all(&tmp);
                let mut paths = Vec::new();
                for i in 0..3 {
                    let p = tmp.join(format!("a{i}.txt"));
                    let _ = std::fs::write(&p, b"hello");
                    paths.push(p.to_string_lossy().to_string());
                }
                let (batch, moved, bytes) = trash_core(&paths, &st.trashed);
                let gone = paths.iter().all(|p| !PathBuf::from(p).exists());
                let listed = !trash_root().join(&batch).join("manifest.json").exists();
                let restored = undo_core(&batch, &st.trashed);
                let back = paths.iter().all(|p| PathBuf::from(p).exists());
                crate::logging::log(&format!(
                    "[selftest] moved={moved} bytes={bytes} gone={gone} manifest_missing={listed} restored={restored} back={back}"
                ));
                let _ = std::fs::remove_dir_all(&tmp);
            }

            // 开发用：VIDEOCC_AUTOSCAN=<目录> 启动即扫描（指定了 VIDEOCC_SCOPE 时交给前端）
            if let Ok(p) = std::env::var("VIDEOCC_AUTOSCAN") {
                let scope_mode = std::env::var("VIDEOCC_SCOPE").is_ok();
                let path = PathBuf::from(p);
                if path.is_dir() && !scope_mode {
                    let st = app.state::<AppState>();
                    st.scanning.store(true, Ordering::SeqCst);
                    st.cancel.store(false, Ordering::SeqCst);
                    let cancel = st.cancel.clone();
                    let sim = std::env::var("VIDEOCC_SIM")
                        .ok()
                        .and_then(|v| v.parse::<u8>().ok())
                        .unwrap_or_else(|| crate::config::default_sim());
                    let tool = std::env::var("VIDEOCC_TOOL")
                        .unwrap_or_else(|_| "similar-videos".to_string());
                    if let Ok(mut t) = st.tool.lock() {
                        *t = tool.clone();
                    }
                    // 与 scan_start_scope 保持同一套范围签名，避免切换工具时误清结果
                    if let Ok(mut s) = st.scope_sig.lock() {
                        *s = format!(
                            "{:?}|{:?}|{}",
                            vec![path.to_string_lossy().to_string()],
                            Vec::<String>::new(),
                            true
                        );
                    }
                    if tool == "similar-videos" {
                        spawn_scan(app.handle().clone(), vec![path], Vec::new(), true, sim, cancel);
                    } else if tool == "similar-images" {
                        spawn_image_scan(app.handle().clone(), vec![path], Vec::new(), true, crate::config::default_sim(), cancel);
                    } else {
                        spawn_tool_scan(app.handle().clone(), vec![path], Vec::new(), true, tool, cancel);
                    }
                }
            }
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            tool_list,
            ping,
            app_version,
            dev_flags,
            scan_start,
            scan_start_scope,
            pick_folders,
            check_paths,
            scan_cancel,
            is_scanning,
            get_groups,
            has_result,
            group_detail,
            get_items,
            get_tool,
            trash_files,
            undo_batch,
            list_batches,
            open_in_explorer,
            job_preview,
            job_start,
            job_cancel,
            job_finish,
            job_undo,
            get_config,
            set_config,
            cache_stats,
            clear_cache,
            open_path,
            open_logs,
            file_data_url,
            frame_data_url,
            purge_batch,
            purge_all,
            trash_stats
        ])
        .run(tauri::generate_context!())
        .expect("error while running VideoCC");
}
