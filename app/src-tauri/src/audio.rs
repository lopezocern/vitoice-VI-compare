use crate::model::Item;
use crate::walk;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const AUDIO_EXT: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma", "aiff", "ape"];

const RATE: usize = 8000;
const WIN: usize = 2000; // 0.25s

pub struct AudioFp {
    pub path: PathBuf,
    pub duration: f32,
    pub rms: Vec<f32>,
}

pub fn collect(roots: &[PathBuf], excludes: &[String], recursive: bool) -> Vec<PathBuf> {
    walk::walk(roots, excludes, recursive, false, None)
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| {
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            AUDIO_EXT.contains(&ext.as_str())
        })
        .collect()
}

/// 解码成 8k 单声道，按 0.25s 窗口取 RMS 包络
pub fn fingerprint(path: &Path) -> Option<AudioFp> {
    let out = crate::media::output_timeout(
        {
            let mut c = crate::media::cmd("ffmpeg");
            c.args(["-hide_banner", "-loglevel", "error", "-i"])
                .arg(path)
                .args(["-map", "a:0", "-ac", "1", "-ar", &RATE.to_string(), "-f", "s16le", "-"]);
            c
        },
        120,
    )
    .ok()?;

    if !out.status.success() || out.stdout.len() < WIN * 2 * 8 {
        return None;
    }

    let bytes = out.stdout;
    let samples: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();

    let duration = samples.len() as f32 / RATE as f32;
    let mut rms = Vec::with_capacity(samples.len() / WIN + 1);
    for chunk in samples.chunks(WIN) {
        if chunk.len() < WIN / 2 {
            break;
        }
        let sum: f64 = chunk.iter().map(|&s| (s as f64) * (s as f64)).sum();
        rms.push(((sum / chunk.len() as f64).sqrt() / 32768.0) as f32);
    }

    // 归一化包络（消除音量差异）
    let mean = rms.iter().sum::<f32>() / rms.len().max(1) as f32;
    let var = rms.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / rms.len().max(1) as f32;
    let std = var.sqrt().max(1e-6);
    for v in rms.iter_mut() {
        *v = (*v - mean) / std;
    }

    Some(AudioFp { path: path.to_path_buf(), duration, rms })
}

fn corr(a: &[f32], b: &[f32], lag: isize) -> (f32, usize) {
    let n = a.len().min(b.len()) as isize;
    let start = lag.max(0);
    let end = (n + lag.min(0)).max(0);
    if end - start < 8 {
        return (-1.0, 0);
    }
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    let mut i = start;
    while i < end {
        let j = i - lag;
        let x = a[i as usize] as f64;
        let y = b[j as usize] as f64;
        dot += x * y;
        na += x * x;
        nb += y * y;
        i += 1;
    }
    let denom = (na * nb).sqrt();
    if denom < 1e-9 {
        return (0.0, (end - start) as usize);
    }
    ((dot / denom) as f32, (end - start) as usize)
}

/// 最大互相关（允许 ±2 秒偏移）
fn similarity(a: &AudioFp, b: &AudioFp) -> f32 {
    let max_lag = 8isize; // 0.25s * 8 = 2s
    let mut best = -1.0f32;
    for lag in -max_lag..=max_lag {
        let (c, len) = corr(&a.rms, &b.rms, lag);
        if len < 8 {
            continue;// 重叠太短
        }
        if c > best {
            best = c;
        }
    }
    best
}

pub fn group(fps: &[AudioFp]) -> Vec<(Vec<usize>, u8)> {
    let n = fps.len();
    if n < 2 {
        return Vec::new();
    }

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

    // 按时长分桶，避免全量两两比较
    let mut buckets: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, f) in fps.iter().enumerate() {
        buckets.entry(f.duration.round() as i32).or_default().push(i);
    }

    // 单桶候选上限；超过时按相近时长窗口比较，而不是整桶跳过
    const MAX_POOL: usize = 400;

    for (&sec, idxs) in buckets.clone().iter() {
        let mut pool: Vec<usize> = idxs.clone();
        for d in [sec - 1, sec + 1] {
            if let Some(v) = buckets.get(&d) {
                pool.extend(v);
            }
        }
        pool.sort();
        pool.dedup();
        if pool.len() < 2 {
            continue;
        }

        fn consider(a: usize, b: usize, fps: &[AudioFp], parent: &mut Vec<usize>) {
            if fps[a].duration.max(1.0) / fps[b].duration.max(1.0) > 1.03 {
                return;
            }
            if similarity(&fps[a], &fps[b]) > 0.95 {
                let ra = find(&mut *parent, a);
                let rb = find(&mut *parent, b);
                if ra != rb {
                    parent[rb] = ra;
                }
            }
        }

        if pool.len() <= MAX_POOL {
            for i in 0..pool.len() {
                for j in (i + 1)..pool.len() {
                    consider(pool[i], pool[j], fps, &mut parent);
                }
            }
        } else {
            pool.sort_by(|&x, &y| {
                fps[x]
                    .duration
                    .partial_cmp(&fps[y].duration)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for i in 0..pool.len() {
                let end = (i + MAX_POOL + 1).min(pool.len());
                for j in (i + 1)..end {
                    consider(pool[i], pool[j], fps, &mut parent);
                }
            }
        }
    }

    let mut by_root: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        by_root.entry(r).or_default().push(i);
    }

    let mut out: Vec<(Vec<usize>, u8)> = Vec::new();
    for (_, mut members) in by_root {
        if members.len() < 2 {
            continue;
        }
        members.sort();
        let base = &fps[members[0]];
        let mut acc = 0f32;
        for &m in members.iter().skip(1) {
            acc += similarity(base, &fps[m]).max(0.0);
        }
        let rate = acc / (members.len() - 1) as f32 * 100.0;
        out.push((members, rate.round().clamp(1.0, 100.0) as u8));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.len().cmp(&a.0.len())));
    out
}

fn fmt_dur(secs: f32) -> String {
    let s = secs.round() as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn to_items(fps: &[AudioFp], groups: &[(Vec<usize>, u8)]) -> Vec<Item> {
    let mut out = Vec::new();
    for (gi, (members, sim)) in groups.iter().enumerate() {
        let n = members.len();
        for (k, &m) in members.iter().enumerate() {
            let f = &fps[m];
            let size = std::fs::metadata(&f.path).map(|x| x.len()).unwrap_or(0);
            let tag = if k == 0 { "（建议保留）" } else { "" };
            out.push(Item {
                path: f.path.to_string_lossy().to_string(),
                name: f
                    .path
                    .file_name()
                    .and_then(|x| x.to_str())
                    .unwrap_or("")
                    .to_string(),
                size,
                detail: format!(
                    "音频组 {} · 共 {} 个 · {} · 相似度 {}% {}",
                    gi + 1,
                    n,
                    fmt_dur(f.duration),
                    sim,
                    tag
                ),
                kind: "audio-dup".to_string(),
            });
        }
    }
    out
}

pub fn run(roots: &[PathBuf], excludes: &[String], recursive: bool) -> Vec<Item> {
    let files = collect(roots, excludes, recursive);
    let mut fps: Vec<AudioFp> = files.iter().filter_map(|p| fingerprint(p)).collect();
    if fps.is_empty() {
        return Vec::new();
    }
    fps.sort_by(|a, b| a.path.cmp(&b.path));
    let groups = group(&fps);
    to_items(&fps, &groups)
}
