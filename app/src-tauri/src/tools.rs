use crate::model::Item;
use crate::walk;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::path::{Path, PathBuf};

pub const IMPLEMENTED: &[&str] = &[
    "duplicates",
    "empty-folders",
    "biggest-files",
    "empty-files",
    "temporary",
    "similar-images",
    "similar-videos",
    "same-music",
    "invalid-symlinks",
    "broken-files",
    "bad-extensions",
    "bad-names",
    "exif-remover",
    "video-optimizer",
];

fn walkdir_all(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<(PathBuf, bool)> {
    walk::walk(roots, excludes, recursive, true, Some(cancel))
}

fn walk_files(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<PathBuf> {
    walk::walk(roots, excludes, recursive, false, Some(cancel))
        .into_iter()
        .map(|(p, _)| p)
        .collect()
}

fn item(p: &Path, detail: String, kind: &str) -> Item {
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    Item {
        path: p.to_string_lossy().to_string(),
        name: p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string(),
        size,
        detail,
        kind: kind.to_string(),
    }
}

fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

fn hash_file(p: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hasher.finalize().to_hex().to_string())
}

/// 并发计算整文件哈希；结果按输入顺序返回，取消时剩余项为 None。
fn hash_files_parallel(files: &[PathBuf], cancel: &AtomicBool) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = Vec::new();
    out.resize_with(files.len(), || None);
    if files.is_empty() {
        return out;
    }

    let workers = crate::config::workers().min(files.len()).max(1);
    let next = AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<(usize, Option<String>)>();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let next = &next;
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= files.len() || cancel.load(Ordering::Relaxed) {
                    break;
                }
                let _ = tx.send((i, hash_file(&files[i])));
            });
        }
        drop(tx);
        for (i, h) in rx {
            out[i] = h;
        }
    });
    out
}

pub fn run(tool: &str, roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<Item> {
    match tool {
        "duplicates" => duplicates(roots, excludes, recursive, cancel),
        "biggest-files" => biggest(roots, excludes, recursive, cancel),
        "empty-files" => empty_files(roots, excludes, recursive, cancel),
        "empty-folders" => empty_folders(roots, excludes, recursive, cancel),
        "temporary" => temporary(roots, excludes, recursive, cancel),
        "invalid-symlinks" => invalid_symlinks(roots, excludes, recursive, cancel),
        "bad-extensions" => bad_extensions(roots, excludes, recursive, cancel),
        "broken-files" => broken_files(roots, excludes, recursive, cancel),
        "bad-names" => bad_names(roots, excludes, recursive, cancel),
        _ => Vec::new(),
    }
}

fn duplicates(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<Item> {
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for f in walk_files(roots, excludes, recursive, cancel) {
        if let Ok(m) = std::fs::metadata(&f) {
            if m.len() > 0 {
                by_size.entry(m.len()).or_default().push(f);
            }
        }
    }

    // 只对同大小的候选算哈希，并发执行
    let mut candidates: Vec<PathBuf> = by_size
        .into_values()
        .filter(|v| v.len() >= 2)
        .flatten()
        .collect();
    candidates.sort();

    let mut by_hash: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let hashes = hash_files_parallel(&candidates, cancel);
    for (p, h) in candidates.into_iter().zip(hashes) {
        if let Some(h) = h {
            by_hash.entry(h).or_default().push(p);
        }
    }

    let mut out = Vec::new();
    let mut group_no = 0;
    for (_, mut same) in by_hash {
        if same.len() < 2 {
            continue;
        }
        same.sort();
        group_no += 1;
        let n = same.len();
        let size = std::fs::metadata(&same[0]).map(|m| m.len()).unwrap_or(0);
        for (i, p) in same.iter().enumerate() {
            let tag = if i == 0 { "（建议保留）" } else { "" };
            out.push(item(
                p,
                format!("重复组 {group_no} · 共 {n} 个副本 · {} {tag}", human_size(size)),
                "duplicate",
            ));
        }
    }
    out.sort_by(|a, b| a.detail.cmp(&b.detail));
    out
}

fn biggest(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<Item> {
    let mut files = walk_files(roots, excludes, recursive, cancel);
    files.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)));
    files
        .into_iter()
        .take(500)
        .map(|p| {
            let s = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            item(&p, human_size(s), "big")
        })
        .collect()
}

fn empty_files(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<Item> {
    walk_files(roots, excludes, recursive, cancel)
        .into_iter()
        .filter(|p| std::fs::metadata(p).map(|m| m.len() == 0).unwrap_or(false))
        .map(|p| item(&p, "0 B".to_string(), "empty"))
        .collect()
}

fn empty_folders(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<Item> {
    let mut out = Vec::new();
    for (p, is_dir) in walkdir_all(roots, excludes, recursive, cancel) {
        if !is_dir {
            continue;
        }
        let empty = std::fs::read_dir(&p)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        if empty {
            out.push(Item {
                path: p.to_string_lossy().to_string(),
                name: p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string(),
                size: 0,
                detail: "空文件夹".to_string(),
                kind: "empty-dir".to_string(),
            });
        }
    }
    out
}

fn temporary(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<Item> {
    const EXT: &[&str] = &["tmp", "temp", "bak", "old", "part", "crdownload", "download"];
    walk_files(roots, excludes, recursive, cancel)
        .into_iter()
        .filter(|p| {
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
            let name = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
            EXT.contains(&ext.as_str()) || name.ends_with('~')
        })
        .map(|p| {
            let s = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            item(&p, human_size(s), "temp")
        })
        .collect()
}

fn invalid_symlinks(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<Item> {
    let mut out = Vec::new();
    for (p, _) in walkdir_all(roots, excludes, recursive, cancel) {
        let Ok(md) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if md.file_type().is_symlink() && std::fs::metadata(&p).is_err() {
            out.push(Item {
                path: p.to_string_lossy().to_string(),
                name: p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string(),
                size: 0,
                detail: "目标不存在".to_string(),
                kind: "broken-link".to_string(),
            });
        }
    }
    out
}

fn ext_of(p: &Path) -> String {
    p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn bad_extensions(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<Item> {
    let mut out = Vec::new();
    for p in walk_files(roots, excludes, recursive, cancel) {
        let Ok(Some(kind)) = infer::get_from_path(&p) else {
            continue;
        };
        let real = kind.extension().to_ascii_lowercase();
        let ext = ext_of(&p);
        if ext.is_empty() {
            continue;
        }
        let same = ext == real
            || (real == "jpg" && ext == "jpeg")
            || (real == "jpeg" && ext == "jpg")
            || (real == "mp4" && (ext == "m4v" || ext == "mov"))
            || (real == "mov" && ext == "mp4");
        if !same {
            out.push(item(&p, format!("扩展名 .{ext}，实际是 {real}"), "bad-ext"));
        }
    }
    out
}

fn broken_files(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<Item> {
    let mut out = Vec::new();
    for p in walk_files(roots, excludes, recursive, cancel) {
        let s = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        if s == 0 {
            continue;
        }
        let ext = ext_of(&p);
        if !matches!(ext.as_str(), "mp4" | "mov" | "mkv" | "avi" | "webm" | "jpg" | "jpeg" | "png" | "gif") {
            continue;
        }
        let ok = if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif") {
            std::fs::File::open(&p).is_ok()
        } else {
            crate::media::output_timeout(
                {
                    let mut c = crate::media::cmd("ffprobe");
                    c.args(["-v", "quiet", "-show_format"]).arg(&p);
                    c
                },
                30,
            )
            .map(|o| o.status.success())
            .unwrap_or(false)
        };
        if !ok {
            out.push(item(&p, "无法解析 / 已损坏".to_string(), "broken"));
        }
    }
    out
}

fn bad_names(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<Item> {
    let mut out = Vec::new();
    for p in walk_files(roots, excludes, recursive, cancel) {
        let name = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let mut issues: Vec<&str> = Vec::new();
        if name.starts_with(' ') || name.ends_with(' ') {
            issues.push("首尾空格");
        }
        if name.ends_with('.') {
            issues.push("以点结尾");
        }
        if name.matches('.').count() > 2 {
            issues.push("多重扩展名");
        }
        if name.chars().any(|c| c.is_control()) {
            issues.push("控制字符");
        }
        if !issues.is_empty() {
            out.push(item(&p, format!("问题：{}", issues.join("、")), "bad-name"));
        }
    }
    out
}
