use crate::media::{self, Probe};
use crate::walk;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct VideoRecord {
    pub path: PathBuf,
    pub meta: Probe,
    pub hashes: Vec<u64>,
    pub frames: Vec<PathBuf>,
    pub times: Vec<f32>,
}

const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "mkv", "avi", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "ts",
];

/// 多根目录 + 排除集；自动去重（嵌套包含不会重复遍历）
/// `recursive = false` 时只扫描根目录第一层的文件
pub fn collect_videos_scoped(roots: &[PathBuf], excludes: &[String], recursive: bool) -> Vec<PathBuf> {
    walk::walk(roots, excludes, recursive, false, None)
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| {
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            VIDEO_EXTS.contains(&ext.as_str())
        })
        .collect()
}

pub fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn hamming_tol() -> u32 {
    use std::sync::OnceLock;
    static T: OnceLock<u32> = OnceLock::new();
    *T.get_or_init(|| {
        std::env::var("VIDEOCC_TOL")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(20)
            .clamp(4, 32)
    })
}
const MAX_OFFSET: isize = 4;

#[derive(Clone, Copy)]
pub struct Score {
    pub matched: usize,
    pub total: usize,
    pub longest_run: usize,
    pub avg_hamming: f32,
}

fn score_at(a: &[u64], b: &[u64], offset: isize, tol: u32) -> Score {
    let len_a = a.len() as isize;
    let len_b = b.len() as isize;
    let start = offset.max(0);
    let end = (len_a.min(len_b + offset)).max(0);
    if end <= start {
        return Score { matched: 0, total: 0, longest_run: 0, avg_hamming: 64.0 };
    }

    let mut matched = 0usize;
    let mut run = 0usize;
    let mut longest = 0usize;
    let mut sum = 0u32;
    let mut i = start;
    while i < end {
        let j = i - offset;
        if j < 0 || j >= len_b {
            i += 1;
            continue;
        }
        let h = media::hamming(a[i as usize], b[j as usize]);
        sum += h;
        if h <= tol {
            matched += 1;
            run += 1;
            if run > longest {
                longest = run;
            }
        } else {
            run = 0;
        }
        i += 1;
    }

    let total = (end - start) as usize;
    Score {
        matched,
        total,
        longest_run: longest,
        avg_hamming: if total > 0 { sum as f32 / total as f32 } else { 64.0 },
    }
}

/// 允许 ±MAX_OFFSET 帧的时间偏移（应对头尾被裁剪），取匹配率最高的一组
pub fn score(a: &[u64], b: &[u64]) -> Score {
    score_with_tol(a, b, hamming_tol())
}

/// 指定逐帧容差（0..64）的评分，供基准测试扫描参数空间
pub fn score_with_tol(a: &[u64], b: &[u64], tol: u32) -> Score {
    let mut best = score_at(a, b, 0, tol);
    let mut best_rate = if best.total > 0 { best.matched as f32 / best.total as f32 } else { 0.0 };

    let mut off = 1;
    while off <= MAX_OFFSET {
        for o in [off, -off] {
            let s = score_at(a, b, o, tol);
            let rate = if s.total > 0 { s.matched as f32 / s.total as f32 } else { 0.0 };
            if rate > best_rate || (rate == best_rate && s.total > best.total) {
                best = s;
                best_rate = rate;
            }
        }
        off += 1;
    }
    best
}

/// 候选策略：按时长分桶（±5 秒），桶内两两比对。
/// 不再依赖 band 精确命中——pHash 下重编码会让哈希漂移，分桶反而漏检。
pub fn group_records(records: &[VideoRecord], sim: u8) -> Vec<(Vec<usize>, u8)> {
    let n = records.len();
    if n < 2 {
        return Vec::new();
    }
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        let mut cur = x;
        while parent[cur] != r {
            let next = parent[cur];
            parent[cur] = r;
            cur = next;
        }
        r
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[rb] = ra;
        }
    }

    let bucket_of = |d: f32| -> i32 { (d / 5.0).floor() as i32 };

    let mut buckets: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, r) in records.iter().enumerate() {
        buckets.entry(bucket_of(r.meta.duration)).or_default().push(i);
    }

    let threshold = sim as f32 / 100.0;
    let mut seen: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    // 单桶候选上限；超过时改为按时长排序后与邻近候选比较，而不是整桶跳过
    const MAX_POOL: usize = 700;

    for (&b, idxs) in buckets.iter() {
        let mut pool: Vec<usize> = idxs.clone();
        for d in [b - 1, b + 1] {
            if let Some(v) = buckets.get(&d) {
                pool.extend(v);
            }
        }
        pool.sort();
        pool.dedup();
        if pool.len() < 2 {
            continue;
        }

        fn consider(
            a: usize,
            c: usize,
            records: &[VideoRecord],
            parent: &mut [usize],
            seen: &mut std::collections::HashSet<(usize, usize)>,
            threshold: f32,
        ) {
            let da = records[a].meta.duration;
            let db = records[c].meta.duration;
            if da > 0.0 && db > 0.0 {
                let ratio = da.max(db) / da.min(db);
                if ratio > 1.6 {
                    return;
                }
            }
            let pair = (a.min(c), a.max(c));
            if !seen.insert(pair) {
                return;
            }
            let s = score(&records[a].hashes, &records[c].hashes);
            if s.total < 4 || s.matched < 4 {
                return;
            }
            let rate = s.matched as f32 / s.total as f32;
            // 整段高相似，或存在足够长的连续匹配段（应对局部相同的剪辑）
            let run_ok = s.longest_run >= 6
                && (s.longest_run as f32) >= (s.total as f32) * 0.35;
            if rate >= threshold || run_ok {
                union(parent, a, c);
            }
        }

        if pool.len() <= MAX_POOL {
            for i in 0..pool.len() {
                for j in (i + 1)..pool.len() {
                    consider(pool[i], pool[j], records, &mut parent, &mut seen, threshold);
                }
            }
        } else {
            // 大桶按时长排序，同源版本时长高度接近，比较邻近窗口即可覆盖
            pool.sort_by(|&x, &y| {
                records[x]
                    .meta
                    .duration
                    .partial_cmp(&records[y].meta.duration)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for i in 0..pool.len() {
                let end = (i + MAX_POOL + 1).min(pool.len());
                for j in (i + 1)..end {
                    consider(pool[i], pool[j], records, &mut parent, &mut seen, threshold);
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
        let base = &records[members[0]].hashes;
        let mut acc = 0f32;
        let mut cnt = 0f32;
        for &m in members.iter().skip(1) {
            let s = score(base, &records[m].hashes);
            if s.total > 0 {
                acc += s.matched as f32 / s.total as f32;
                cnt += 1.0;
            }
        }
        let rate = if cnt > 0.0 { acc / cnt } else { 1.0 };
        out.push((members, (rate * 100.0).round().clamp(1.0, 100.0) as u8));
    }

    out.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.len().cmp(&a.0.len())));
    out
}
pub fn evidence_for(a: &[u64], b: &[u64]) -> Score {
    score(a, b)
}




#[cfg(test)]
mod tests {
    use super::*;

    /// splitmix64：生成互相独立的伪随机哈希，避免测试数据自带结构
    fn mix(mut z: u64) -> u64 {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn series(seed: u64, n: usize) -> Vec<u64> {
        (0..n).map(|i| mix(seed ^ (i as u64))).collect()
    }

    fn rec(dur: f32, hashes: Vec<u64>) -> VideoRecord {
        VideoRecord {
            path: PathBuf::from("x.mp4"),
            meta: media::Probe {
                width: 0,
                height: 0,
                duration: dur,
                bitrate: 0,
                size: 0,
            },
            hashes,
            frames: Vec::new(),
            times: Vec::new(),
        }
    }

    #[test]
    fn identical_hashes_score_full() {
        let h = series(7919, 12);
        let s = score_with_tol(&h, &h, 16);
        assert_eq!(s.matched, s.total);
        assert_eq!(s.longest_run, s.total);
    }

    #[test]
    fn score_tolerates_frame_shift() {
        let a = series(104729, 20);
        let mut b = vec![0u64; 3];
        b.extend(a.clone());
        let s = score_with_tol(&a, &b, 16);
        let rate = s.matched as f32 / s.total as f32;
        assert!(rate > 0.95, "错开 3 帧应仍能对齐，实际匹配率 {rate}");
    }

    #[test]
    fn unrelated_hashes_score_low() {
        let a = series(0x9E37_79B9, 20);
        let b = series(0xD6E8_FEB8, 20);
        let s = score_with_tol(&a, &b, 20);
        let rate = s.matched as f32 / s.total as f32;
        assert!(rate < 0.2, "无关素材匹配率应接近 0，实际 {rate}");
    }

    #[test]
    fn groups_merge_same_source_only() {
        let base = series(31337, 10);
        let mut near = base.clone();
        near[0] ^= 0x0F; // 4 位扰动，远小于容差
        let other = series(0xDEAD_BEEF, 10);

        let recs = vec![rec(10.0, base.clone()), rec(10.0, near), rec(10.0, other)];
        let groups = group_records(&recs, 85);
        assert_eq!(groups.len(), 1, "应只有一组");
        assert_eq!(groups[0].0, vec![0, 1], "同源两段应合并，第三段不参与");
    }

    #[test]
    fn groups_ignore_duration_mismatch() {
        let h = series(555, 10);
        let recs = vec![rec(10.0, h.clone()), rec(60.0, h)];
        assert!(group_records(&recs, 85).is_empty(), "时长差 6 倍不应判为同一段");
    }
}
