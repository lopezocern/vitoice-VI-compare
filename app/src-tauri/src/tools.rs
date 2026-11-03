use crate::model::Item;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::{Path, PathBuf};

const RESERVED: &[&str] = &["_optimized", "_clean", "_backup"];

fn is_reserved(p: &std::path::Path) -> bool {
    p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        RESERVED.contains(&s.as_str())
    })
}


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

fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").to_lowercase().trim_end_matches('\\').to_string()
}

fn excluded(p: &Path, excludes: &[String]) -> bool {
    if excludes.is_empty() {
        return false;
    }
    let s = norm(p);
    excludes
        .iter()
        .filter(|e| !e.is_empty())
        .any(|e| s == *e || s.starts_with(&format!("{e}\\")))
}

fn walkdir_all(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<(PathBuf, bool)> {
    let mut out = Vec::new();
    let mut n = 0usize;
    for root in roots {
        let mut walker = walkdir::WalkDir::new(root).follow_links(false);
        if !recursive {
            walker = walker.max_depth(1);
        }
        for e in walker
            .into_iter()
            .filter_map(|r| r.ok())
        {
            n += 1;
            if n % 256 == 0 && cancel.load(Ordering::Relaxed) {
                return out;
            }
            let p = e.path().to_path_buf();
            if is_reserved(&p) || excluded(&p, excludes) {
                continue;
            }
            let is_dir = e.file_type().is_dir();
            out.push((p, is_dir));
        }
    }
    out
}

fn walk_files(roots: &[PathBuf], excludes: &[String], recursive: bool, cancel: &AtomicBool) -> Vec<PathBuf> {
    walkdir_all(roots, excludes, recursive, cancel)
        .into_iter()
        .filter(|(_, d)| !*d)
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

    let mut out = Vec::new();
    let mut group_no = 0;
    for (size, paths) in by_size {
        if paths.len() < 2 {
            continue;
        }
        let mut by_hash: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for p in paths {
            if let Some(h) = hash_file(&p) {
                by_hash.entry(h).or_default().push(p);
            }
        }
        for (_, mut same) in by_hash {
            if same.len() < 2 {
                continue;
            }
            same.sort();
            group_no += 1;
            let n = same.len();
            for (i, p) in same.iter().enumerate() {
                let tag = if i == 0 { "（建议保留）" } else { "" };
                out.push(item(
                    p,
                    format!("重复组 {group_no} · 共 {n} 个副本 · {} {tag}", human_size(size)),
                    "duplicate",
                ));
            }
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
            crate::media::cmd("ffprobe")
                .args(["-v", "quiet", "-show_format"])
                .arg(&p)
                .output()
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
