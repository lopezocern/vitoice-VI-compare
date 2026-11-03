//! 召回基准测试：测量「同一素材的不同版本」能否被判为同一组。
//!
//! 用法：
//!   cargo run --release --bin bench -- <fixtures_dir>
//!
//! 目录里的文件名约定为 `<源编号>__<变体名>.mp4`，例如 `s01__orig.mp4`、
//! `s01__720p.mp4`：源编号相同的任意两两为「正样本」，编号不同的为「负样本」。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use vitoicecompare_lib::media;
use vitoicecompare_lib::scanner;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("用法: bench <fixtures_dir>");
        std::process::exit(2);
    });
    let dir = PathBuf::from(dir);

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("读取目录失败")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .map(|x| matches!(x.to_ascii_lowercase().as_str(), "mp4" | "mov" | "mkv"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();

    if files.is_empty() {
        eprintln!("目录里没有视频");
        std::process::exit(2);
    }

    let tmp = std::env::temp_dir().join("vcc_bench");
    let _ = std::fs::remove_dir_all(&tmp);

    println!("抽帧哈希：{} 个文件", files.len());
    let mut items: Vec<(String, Vec<u64>, f32)> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let src = name.split("__").next().unwrap_or("?").to_string();

        let probe = match media::probe(f) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("  跳过 {name}: {e}");
                continue;
            }
        };
        let out = tmp.join(format!("f{i}"));
        let ex = match media::extract(f, &out, probe.duration, 8, 48, 2.0) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("  跳过 {name}: {e}");
                continue;
            }
        };
        items.push((src, ex.hashes, probe.duration));
        print!("\r  {}/{}", i + 1, files.len());
    }
    println!();

    // 正负样本对
    let mut pos: Vec<(usize, usize)> = Vec::new();
    let mut neg: Vec<(usize, usize)> = Vec::new();
    for i in 0..items.len() {
        for j in (i + 1)..items.len() {
            if items[i].0 == items[j].0 {
                pos.push((i, j));
            } else {
                neg.push((i, j));
            }
        }
    }
    println!("正样本 {} 对，负样本 {} 对\n", pos.len(), neg.len());

    println!("{:>4} {:>6} {:>8} {:>8} {:>8} {:>8}", "tol", "阈值", "召回", "精确", "F1", "FP");
    let mut best: Option<(u32, f32, f32, f32)> = None;

    for tol in [8u32, 12, 16, 20, 24, 28] {
        // 先把所有样本对的匹配率算出来，再扫阈值
        let mut pos_rates: Vec<f32> = Vec::with_capacity(pos.len());
        for &(i, j) in &pos {
            let s = scanner::score_with_tol(&items[i].1, &items[j].1, tol);
            pos_rates.push(if s.total > 0 { s.matched as f32 / s.total as f32 } else { 0.0 });
        }
        let mut neg_rates: Vec<f32> = Vec::with_capacity(neg.len());
        for &(i, j) in &neg {
            let s = scanner::score_with_tol(&items[i].1, &items[j].1, tol);
            neg_rates.push(if s.total > 0 { s.matched as f32 / s.total as f32 } else { 0.0 });
        }

        for thr in [0.60f32, 0.70, 0.75, 0.80, 0.85, 0.90, 0.95] {
            let tp = pos_rates.iter().filter(|r| **r >= thr).count();
            let fn_ = pos_rates.len() - tp;
            let fp = neg_rates.iter().filter(|r| **r >= thr).count();
            let recall = tp as f32 / (tp + fn_).max(1) as f32;
            let precision = if tp + fp > 0 { tp as f32 / (tp + fp) as f32 } else { 0.0 };
            let f1 = if precision + recall > 0.0 {
                2.0 * precision * recall / (precision + recall)
            } else {
                0.0
            };
            println!(
                "{:>4} {:>6.2} {:>7.1}% {:>7.1}% {:>7.3} {:>8}",
                tol,
                thr,
                recall * 100.0,
                precision * 100.0,
                f1,
                fp
            );
            if best.map(|b| f1 > b.3).unwrap_or(true) {
                best = Some((tol, thr, recall, f1));
            }
        }
        println!();
    }

    if let Some((tol, thr, recall, f1)) = best {
        println!("最佳: tol={tol} 阈值={thr:.2} 召回={:.1}% F1={f1:.3}", recall * 100.0);
    }

    // 分组视角：把匹配关系并查集后，看每个源是否被完整合并
    let tol = best.map(|b| b.0).unwrap_or(16);
    let thr = best.map(|b| b.1).unwrap_or(0.75);
    let mut parent: Vec<usize> = (0..items.len()).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        r
    }
    for i in 0..items.len() {
        for j in (i + 1)..items.len() {
            let s = scanner::score_with_tol(&items[i].1, &items[j].1, tol);
            let rate = if s.total > 0 { s.matched as f32 / s.total as f32 } else { 0.0 };
            if rate >= thr {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[b] = a;
                }
            }
        }
    }
    let mut clusters: HashMap<usize, Vec<String>> = HashMap::new();
    for i in 0..items.len() {
        let r = find(&mut parent, i);
        clusters.entry(r).or_default().push(items[i].0.clone());
    }
    let mut pure = 0;
    let mut mixed = 0;
    for (_, members) in &clusters {
        let mut uniq: Vec<&String> = members.iter().collect();
        uniq.sort();
        uniq.dedup();
        if uniq.len() == 1 {
            pure += 1;
        } else {
            mixed += 1;
        }
    }
    let total_sources = {
        let mut s: Vec<&String> = items.iter().map(|x| &x.0).collect();
        s.sort();
        s.dedup();
        s.len()
    };
    println!(
        "\n分组视角: {} 组纯净 / {} 组混入了不同素材 / 应有 {} 个素材",
        pure, mixed, total_sources
    );
}
