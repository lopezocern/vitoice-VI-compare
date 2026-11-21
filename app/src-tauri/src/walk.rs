use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// 这些目录是本工具自己的输出，扫描时必须跳过
pub const RESERVED: &[&str] = &["_optimized", "_clean", "_backup"];

pub fn is_reserved(p: &Path) -> bool {
    p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        RESERVED.contains(&s.as_str())
    })
}

pub fn norm(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .to_lowercase()
        .trim_end_matches('\\')
        .to_string()
}

pub fn is_excluded(p: &Path, excludes: &[String]) -> bool {
    if excludes.is_empty() {
        return false;
    }
    let s = norm(p);
    excludes
        .iter()
        .filter(|e| !e.is_empty())
        .any(|e| s == *e || s.starts_with(&format!("{e}\\")))
}

/// 统一遍历入口：
/// - 多根目录，嵌套包含自动去重；
/// - 跳过排除目录与 `_optimized` / `_clean` / `_backup`；
/// - 按规范化路径去重；
/// - `include_dirs = false` 时只返回文件（符号链接仍保留，交给各工具判断）。
pub fn walk(
    roots: &[PathBuf],
    excludes: &[String],
    recursive: bool,
    include_dirs: bool,
    cancel: Option<&AtomicBool>,
) -> Vec<(PathBuf, bool)> {
    // 父目录覆盖子目录，避免同一个目录被遍历多次
    let mut sorted: Vec<&PathBuf> = roots.iter().filter(|p| p.is_dir()).collect();
    sorted.sort_by_key(|p| p.components().count());

    let mut effective: Vec<&PathBuf> = Vec::new();
    for r in sorted {
        let rn = norm(r);
        let covered = effective.iter().any(|e| {
            let en = norm(e);
            rn == en || rn.starts_with(&format!("{en}\\"))
        });
        if !covered {
            effective.push(r);
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let mut count = 0usize;

    for root in effective {
        let mut w = walkdir::WalkDir::new(root).follow_links(false);
        if !recursive {
            w = w.max_depth(1);
        }
        for e in w.into_iter().filter_map(|r| r.ok()) {
            count += 1;
            if count % 256 == 0 {
                if let Some(c) = cancel {
                    if c.load(Ordering::Relaxed) {
                        return out;
                    }
                }
            }

            let p = e.path();
            if is_reserved(p) || is_excluded(p, excludes) {
                continue;
            }
            let is_dir = e.file_type().is_dir();
            if is_dir && !include_dirs {
                continue;
            }
            if seen.insert(norm(p)) {
                out.push((p.to_path_buf(), is_dir));
            }
        }
    }
    out
}
