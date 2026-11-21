use crate::cache::{self, CachedVideo, CACHE_VER};
use crate::media;
use crate::scanner::{self, VideoRecord};
use crate::walk;
use std::path::{Path, PathBuf};

pub const EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"];

fn is_image(p: &Path) -> bool {
    let ext = p
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    EXT.contains(&ext.as_str())
}

pub fn collect(roots: &[PathBuf], excludes: &[String], recursive: bool) -> Vec<PathBuf> {
    walk::walk(roots, excludes, recursive, false, None)
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| is_image(p))
        .collect()
}

/// 解析一张图：生成缩略图 + 64bit pHash，并带缓存
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
    // 单个倒排桶过大通常意味着大量纯色/近似图；提高上限并记录告警，避免静默丢弃
    const MAX_BUCKET: usize = 20_000;
    for members in buckets.values() {
        if members.len() < 2 {
            continue;
        }
        if members.len() > MAX_BUCKET {
            crate::logging::log(&format!(
                "[images] 桶内候选 {} 个，超过上限 {MAX_BUCKET}，该桶跳过比对",
                members.len()
            ));
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
