use crate::images;
use crate::scanner;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JobFile {
    pub path: String,
    pub name: String,
    pub size: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct JobPreview {
    pub tool: String,
    pub files: Vec<JobFile>,
    pub total_bytes: u64,
    pub est_bytes: u64,
    pub est_secs: u32,
    pub notes: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ManifestEntry {
    pub src: String,
    pub out: String,
    pub in_bytes: u64,
    pub out_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JobManifest {
    pub tool: String,
    pub preset: String,
    pub created_ms: u64,
    pub out_dir: String,
    pub entries: Vec<ManifestEntry>,
}

fn preset_ratio(preset: &str) -> f32 {
    match preset {
        "social" => 0.32,
        "archive" => 0.50,
        "extreme" => 0.24,
        _ => 0.35,
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn preview(
    tool: &str,
    roots: &[PathBuf],
    excludes: &[String],
    recursive: bool,
) -> JobPreview {
    let mut notes = Vec::new();

    let (paths, total_bytes): (Vec<PathBuf>, u64) = if tool == "video-optimizer" {
        let paths = scanner::collect_videos_scoped(roots, excludes, recursive);
        let total = paths
            .iter()
            .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
            .sum();
        if paths.len() > 200 {
            notes.push(format!(
                "共 {} 个视频，预估值为抽样推算，实际可能偏差 20% 以上",
                paths.len()
            ));
        }
        (paths, total)
    } else {
        let paths = images::collect(roots, excludes, recursive);
        let total = paths
            .iter()
            .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
            .sum();
        (paths, total)
    };

    let files: Vec<JobFile> = paths
        .iter()
        .map(|p| JobFile {
            path: p.to_string_lossy().to_string(),
            name: p
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_string(),
            size: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
        })
        .collect();

    let n = files.len();
    let est_bytes = if tool == "video-optimizer" {
        (total_bytes as f64 * preset_ratio("social") as f64) as u64
    } else {
        total_bytes
    };

    let est_secs = if tool == "video-optimizer" {
        (total_bytes / (4 * 1024 * 1024)).max(1) as u32
    } else {
        (n as f32 * 0.12).max(1.0) as u32
    };

    if n == 0 {
        notes.push("当前扫描范围内没有可用文件".to_string());
    }

    JobPreview {
        tool: tool.to_string(),
        files,
        total_bytes,
        est_bytes,
        est_secs,
        notes,
    }
}

fn out_root(tool: &str, first_root: &Path) -> PathBuf {
    let sub = if tool == "video-optimizer" {
        "_optimized"
    } else {
        "_clean"
    };
    first_root.join(sub)
}

fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut i = 2;
    while p.exists() {
        p = dir.join(format!("{stem}_{i}.{ext}"));
        i += 1;
    }
    p
}

fn run_ffmpeg(args: &[String], timeout_secs: u64) -> Result<(), String> {
    let out = crate::media::output_timeout(
        {
            let mut c = crate::media::cmd("ffmpeg");
            c.args(args);
            c
        },
        timeout_secs,
    )?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr);
        Err(msg.lines().next().unwrap_or("未知错误").to_string())
    }
}

fn video_args(src: &Path, dst: &Path, preset: &str) -> Vec<String> {
    let (codec, crf, cap) = match preset {
        "social" => ("libx264", "23", "1080"),
        "archive" => ("libx265", "20", "0"),
        "extreme" => ("libx264", "28", "720"),
        _ => ("libx264", "23", "1080"),
    };

    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        src.to_string_lossy().to_string(),
    ];

    if cap == "0" {
        args.extend(["-c:v".into(), codec.into(), "-crf".into(), crf.into()]);
    } else {
        let vf = format!("scale='min({cap},iw)':-2");
        args.extend([
            "-vf".into(),
            vf,
            "-c:v".into(),
            codec.into(),
            "-crf".into(),
            crf.into(),
        ]);
    }

    if codec == "libx265" {
        args.extend(["-tag:v".into(), "hvc1".into()]);
    }

    args.extend([
        "-preset".into(),
        "veryfast".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "128k".into(),
        "-movflags".into(),
        "+faststart".into(),
        dst.to_string_lossy().to_string(),
    ]);
    args
}

fn exif_args(src: &Path, dst: &Path) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        src.to_string_lossy().to_string(),
        "-map_metadata".into(),
        "-1".into(),
        "-c".into(),
        "copy".into(),
        dst.to_string_lossy().to_string(),
    ]
}

pub fn run(
    app: AppHandle,
    tool: String,
    preset: String,
    files: Vec<JobFile>,
    first_root: PathBuf,
    cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let out_dir = out_root(&tool, &first_root);
        let _ = std::fs::create_dir_all(&out_dir);

        let total = files.len();
        let mut entries: Vec<ManifestEntry> = Vec::new();
        let mut failed: Vec<String> = Vec::new();

        for (i, f) in files.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }

            let src = PathBuf::from(&f.path);
            let _ = app.emit(
                "job://progress",
                serde_json::json!({
                    "done": i, "total": total,
                    "current": f.name,
                    "pct": if total > 0 { (i * 100 / total) as u32 } else { 0 },
                }),
            );

            let stem = src
                .file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or("file")
                .to_string();

            let (dst, args) = if tool == "video-optimizer" {
                let d = unique_path(&out_dir, &format!("{stem}_opt"), "mp4");
                let a = video_args(&src, &d, &preset);
                (d, a)
            } else {
                let ext = src
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.to_ascii_lowercase())
                    .unwrap_or_else(|| "jpg".to_string());
                let d = unique_path(&out_dir, &format!("{stem}_clean"), &ext);
                let a = exif_args(&src, &d);
                (d, a)
            };

            match run_ffmpeg(&args, if tool == "video-optimizer" { 3600 } else { 120 }) {
                Ok(_) => {
                    let out_bytes = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
                    entries.push(ManifestEntry {
                        src: f.path.clone(),
                        out: dst.to_string_lossy().to_string(),
                        in_bytes: f.size,
                        out_bytes,
                    });
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&dst);
                    failed.push(format!("{}: {}", f.name, e));
                }
            }
        }

        let manifest = JobManifest {
            tool: tool.clone(),
            preset: preset.clone(),
            created_ms: now_ms(),
            out_dir: out_dir.to_string_lossy().to_string(),
            entries: entries.clone(),
        };
        if let Ok(txt) = serde_json::to_string_pretty(&manifest) {
            let _ = std::fs::write(out_dir.join("manifest.json"), txt);
        }

        let in_bytes: u64 = entries.iter().map(|e| e.in_bytes).sum();
        let out_bytes: u64 = entries.iter().map(|e| e.out_bytes).sum();

        let _ = app.emit(
            "job://finished",
            serde_json::json!({
                "ok": entries.len(),
                "failed": failed.len(),
                "errors": failed.iter().take(5).collect::<Vec<_>>(),
                "in_bytes": in_bytes,
                "out_bytes": out_bytes,
                "out_dir": manifest.out_dir,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "cancelled": cancel.load(Ordering::Relaxed),
            }),
        );

        crate::logging::log(&format!(
            "[job] {} preset={} ok={} failed={} elapsed={:.1}s",
            tool,
            preset,
            entries.len(),
            failed.len(),
            started.elapsed().as_secs_f64()
        ));
        for e in failed.iter().take(10) {
            crate::logging::error(&format!("[job] {e}"));
        }
    });
}

pub fn undo(out_dir: &Path) -> Result<usize, String> {
    let txt = std::fs::read_to_string(out_dir.join("manifest.json"))
        .map_err(|_| "找不到该批次的 manifest".to_string())?;
    let m: JobManifest = serde_json::from_str(&txt).map_err(|e| e.to_string())?;

    let mut removed = 0usize;
    for e in &m.entries {
        if std::fs::remove_file(&e.out).is_ok() {
            removed += 1;
        }
    }
    let _ = std::fs::remove_file(out_dir.join("manifest.json"));
    Ok(removed)
}
