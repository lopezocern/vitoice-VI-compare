use crate::cache::{self, CachedVideo, CACHE_VER};
use crate::media;
use crate::model::Item;
use crate::scanner::{self, VideoRecord};
use std::path::{Path, PathBuf};

const RESERVED: &[&str] = &["_optimized", "_clean", "_backup"];

fn is_reserved(p: &std::path::Path) -> bool {
    p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        RESERVED.contains(&s.as_str())
    })
}


pub const EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"];

fn is_image(p: &Path) -> bool {
    let ext = p
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    EXT.contains(&ext.as_str())
}

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

pub fn collect(roots: &[PathBuf], excludes: &[String], recursive: bool) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for root in roots {
        let mut walker = walkdir::WalkDir::new(root).follow_links(false);
        if !recursive {
            walker = walker.max_depth(1);
        }
        for e in walker.into_iter().filter_map(|r| r.ok()) {
            if !e.file_type().is_file() {
                continue;
            }
            let p = e.path();
            if is_reserved(p) || excluded(p, excludes) || !is_image(p) {
                continue;
            }
            if seen.insert(norm(p)) {
                out.push(p.to_path_buf());
            }
        }
    }
    out
}

/// 解析一张图：生成 720px 缩略图 + 64bit dHash，并带缓存
pub fn process(file: &Path) -> Option<VideoRecord> {
    let dir = crate::thumb_root().join(scanner::path_key(file));

    if let Some(c) = cache::load(&dir, file) {
        return Some(VideoRecord {
            path: file.to_path_buf(),
            meta: media::Probe {
                width: c.width,
                height: c.height,
                duration: 0.0,
                bitrate: 0,
                size: c.size,
            },
            hashes: c.hashes,
            frames: c.frames.iter().map(|f| dir.join(f)).collect(),
            times: vec![0.0],
        });
    }

    let img = image::open(file).ok()?;
    let (w, h) = (img.width(), img.height());

    std::fs::create_dir_all(&dir).ok()?;
    let thumb_name = "img.jpg";
    let ts = crate::config::thumb_size();
    let thumb = img.thumbnail(ts, ts);
    thumb.save(dir.join(thumb_name)).ok()?;

    let small = img.thumbnail_exact(32, 32).to_luma8();
    let hash = media::phash64(small.as_raw());

    let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    cache::save(
        &dir,
        &CachedVideo {
            ver: CACHE_VER,
            path: file.to_string_lossy().to_string(),
            size,
            mtime: cache::mtime(file),
            width: w,
            height: h,
            duration: 0.0,
            bitrate: 0,
            hashes: vec![hash],
            times: vec![0.0],
            frames: vec![thumb_name.to_string()],
        },
    );

    Some(VideoRecord {
        path: file.to_path_buf(),
        meta: media::Probe { width: w, height: h, duration: 0.0, bitrate: 0, size },
        hashes: vec![hash],
        frames: vec![dir.join(thumb_name)],
        times: vec![0.0],
    })
}

pub fn scan(roots: &[PathBuf], excludes: &[String], recursive: bool) -> Vec<VideoRecord> {
    collect(roots, excludes, recursive)
        .iter()
        .filter_map(|p| process(p))
        .collect()
}

fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 单哈希分组：4×16bit band 建倒排，再按汉明距离合并
pub fn group(records: &[VideoRecord], sim: u8) -> Vec<(Vec<usize>, u8)> {
    let n = records.len();
    if n == 0 {
        return Vec::new();
    }
    let tol = (((100u32.saturating_sub(sim as u32)) * 64) / 100).max(3);

    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut Vec<usize>, x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut c = x;
        while p[c] != r {
            let nx = p[c];
            p[c] = r;
            c = nx;
        }
        r
    }
    fn union(p: &mut Vec<usize>, a: usize, b: usize) {
        let (ra, rb) = (find(p, a), find(p, b));
        if ra != rb {
            p[rb] = ra;
        }
    }

    // 倒排：低 32 位分两段做候选
    let mut buckets: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for (i, r) in records.iter().enumerate() {
        let h = r.hashes.first().copied().unwrap_or(0);
        buckets.entry((h & 0xFFFFFFFF) as u32).or_default().push(i as u32);
        buckets.entry(((h >> 32) & 0xFFFFFFFF) as u32).or_default().push(i as u32);
    }

    let mut seen = std::collections::HashSet::new();
    for members in buckets.values() {
        if members.len() < 2 || members.len() > 4000 {
            continue;
        }
        for i in 0..members.len() {
            for j in (i + 1)..members.len() {
                let a = members[i] as usize;
                let b = members[j] as usize;
                if !seen.insert((a.min(b), a.max(b))) {
                    continue;
                }
                let ha = records[a].hashes.first().copied().unwrap_or(0);
                let hb = records[b].hashes.first().copied().unwrap_or(0);
                if hamming(ha, hb) <= tol {
                    union(&mut parent, a, b);
                }
            }
        }
    }

    let mut by_root: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        by_root.entry(r).or_default().push(i);
    }

    let mut out = Vec::new();
    for (_, mut members) in by_root {
        if members.len() < 2 {
            continue;
        }
        members.sort();
        let base = records[members[0]].hashes.first().copied().unwrap_or(0);
        let mut acc = 0f32;
        for &m in members.iter().skip(1) {
            let h = records[m].hashes.first().copied().unwrap_or(0);
            let dist = hamming(base, h) as f32;
            acc += 100.0 - (dist / 64.0 * 100.0);
        }
        let rate = if members.len() > 1 {
            acc / (members.len() - 1) as f32
        } else {
            100.0
        };
        out.push((members, rate.round().clamp(1.0, 100.0) as u8));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.len().cmp(&a.0.len())));
    out
}

pub fn to_item(p: &Path) -> Item {
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    Item {
        path: p.to_string_lossy().to_string(),
        name: p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string(),
        size,
        detail: "图片".to_string(),
        kind: "image".to_string(),
    }
}
