use std::path::{Path, PathBuf};

/// 创建子进程：GUI 程序里必须加 CREATE_NO_WINDOW，否则每次调 ffmpeg 都会闪一个黑框
pub fn cmd(program: &str) -> std::process::Command {
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}
pub struct Probe {
    pub width: u32,
    pub height: u32,
    pub duration: f32,
    pub bitrate: u32,
    pub size: u64,
}

pub struct Extracted {
    pub hashes: Vec<u64>,
    pub frames: Vec<PathBuf>,
    pub times: Vec<f32>,
}

pub fn probe(path: &Path) -> Result<Probe, String> {
    let out = crate::media::cmd("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("ffprobe 无法启动: {e}"))?;

    if !out.status.success() {
        return Err("ffprobe 解析失败".to_string());
    }

    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe 输出解析失败: {e}"))?;

    let stream = v["streams"]
        .as_array()
        .and_then(|a| a.iter().find(|s| s["codec_type"] == "video"))
        .ok_or_else(|| "没有视频流".to_string())?;

    let width = stream["width"].as_u64().unwrap_or(0) as u32;
    let height = stream["height"].as_u64().unwrap_or(0) as u32;

    let duration = v["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f32>().ok())
        .or_else(|| stream["duration"].as_str().and_then(|s| s.parse::<f32>().ok()))
        .unwrap_or(0.0);

    let bitrate = v["format"]["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    Ok(Probe { width, height, duration, bitrate, size })
}

/// 32×32 灰度 → pHash（DCT 低频 8×8，中位数二值化）
/// 相比 dHash（相邻像素比较），对重编码 / 缩放 / 调色鲁棒得多
pub fn phash64(gray: &[u8]) -> u64 {
    use std::sync::OnceLock;
    static COS: OnceLock<[[f32; 32]; 8]> = OnceLock::new();
    let cos = COS.get_or_init(|| {
        let mut t = [[0f32; 32]; 8];
        for (u, row) in t.iter_mut().enumerate() {
            let cu = if u == 0 { 1.0 / 2f32.sqrt() } else { 1.0 };
            for (x, v) in row.iter_mut().enumerate() {
                let a = (2.0 * x as f32 + 1.0) * u as f32 * std::f32::consts::PI / 64.0;
                *v = cu * a.cos();
            }
        }
        t
    });

    let mut rows = [[0f32; 8]; 32];
    for y in 0..32 {
        for (u, c) in cos.iter().enumerate() {
            let mut s = 0f32;
            for x in 0..32 {
                s += gray[y * 32 + x] as f32 * c[x];
            }
            rows[y][u] = s;
        }
    }

    let mut coeff = [[0f32; 8]; 8];
    for (v, c) in cos.iter().enumerate() {
        for (u, slot) in coeff[v].iter_mut().enumerate() {
            let mut s = 0f32;
            for y in 0..32 {
                s += rows[y][u] * c[y];
            }
            *slot = s;
        }
    }

    let mut vals: Vec<f32> = Vec::with_capacity(63);
    for v in 0..8 {
        for u in 0..8 {
            if v == 0 && u == 0 {
                continue;
            }
            vals.push(coeff[v][u]);
        }
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = vals[vals.len() / 2];

    let mut h = 0u64;
    let mut k = 0u32;
    for v in 0..8 {
        for u in 0..8 {
            if v == 0 && u == 0 {
                k += 1;
                continue;
            }
            if coeff[v][u] > median {
                h |= 1u64 << (63 - k);
            }
            k += 1;
        }
    }
    h
}

/// 一次解码同时产出：8 帧展示用 JPEG + 每帧 9x8 灰度原图（用于 dHash）。
pub fn extract(
    input: &Path,
    out_dir: &Path,
    duration: f32,
    min_frames: usize,
    max_frames: usize,
    sec_per_frame: f32,
) -> Result<Extracted, String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("创建缓存目录失败: {e}"))?;

    let dur = if duration.is_finite() && duration > 0.0 { duration } else { 0.0 };
    let est = if dur > 0.0 {
        (dur / sec_per_frame).round() as usize
    } else {
        min_frames
    };
    let n = est.clamp(min_frames.max(1), max_frames.max(min_frames));
    let interval = if dur > 0.0 { dur / n as f32 } else { sec_per_frame };
    let fps = 1.0 / interval.max(0.05);

    let jpg_pat = out_dir.join("f_%03d.jpg");
    let raw_path = out_dir.join("raw.bin");
    let _ = std::fs::remove_file(&raw_path);

    let filter = format!(
        "[0:v]fps={fps:.6},scale=160:90:force_original_aspect_ratio=decrease,pad=160:90:(ow-iw)/2:(oh-ih)/2:color=black[jpg];\
         [0:v]fps={fps:.6},scale=32:32,format=gray[raw]"
    );

    let status = crate::media::cmd("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-threads", "2"])
        .arg("-i")
        .arg(input)
        .args(["-filter_complex", &filter])
        .args(["-map", "[jpg]", "-q:v", "5"])
        .arg(&jpg_pat)
        .args(["-map", "[raw]", "-f", "rawvideo", "-pix_fmt", "gray"])
        .arg(&raw_path)
        .output()
        .map_err(|e| format!("ffmpeg 无法启动: {e}"))?;

    if !status.status.success() {
        let msg = String::from_utf8_lossy(&status.stderr);
        return Err(format!("ffmpeg 抽帧失败: {}", msg.lines().next().unwrap_or("未知错误")));
    }

    let raw = std::fs::read(&raw_path).map_err(|e| format!("读取哈希数据失败: {e}"))?;
    let frame_size = 32 * 32;
    let count = raw.len() / frame_size;

    let mut hashes = Vec::with_capacity(count);
    for i in 0..count {
        hashes.push(phash64(&raw[i * frame_size..(i + 1) * frame_size]));
    }

    let mut frames: Vec<PathBuf> = std::fs::read_dir(out_dir)
        .map_err(|e| format!("读取缓存目录失败: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "jpg").unwrap_or(false))
        .collect();
    frames.sort();

    let times: Vec<f32> = (0..hashes.len()).map(|i| (i as f32 + 0.5) * interval).collect();

    let _ = std::fs::remove_file(&raw_path);

    Ok(Extracted { hashes, frames, times })
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn hamming(a: u64, b: u64) -> u32 {
        (a ^ b).count_ones()
    }

    /// 类似真实画面的图案：两个方向的正弦叠加 + 一个亮斑
    fn picture(px_shift: f32, bright: i32) -> Vec<u8> {
        (0..32 * 32)
            .map(|i| {
                let x = (i % 32) as f32;
                let y = (i / 32) as f32;
                let mut v = 120.0
                    + (x * 0.31 + px_shift).sin() * 45.0
                    + (y * 0.22).cos() * 30.0;
                let d = ((x - 16.0).powi(2) + (y - 16.0).powi(2)).sqrt();
                if d < 8.0 {
                    v += bright as f32;
                }
                v.clamp(0.0, 255.0) as u8
            })
            .collect()
    }

    #[test]
    fn phash_is_deterministic() {
        let g = picture(0.0, 60);
        assert_eq!(phash64(&g), phash64(&g));
    }

    #[test]
    fn phash_survives_compression_noise() {
        let a = picture(0.4, 55);
        // 模拟压缩噪声：±6 灰度抖动 + 轻微亮度偏移
        let b: Vec<u8> = a
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let n = ((i * 37) % 13) as i32 - 6;
                (*v as i32 + n + 3).clamp(0, 255) as u8
            })
            .collect();
        let d = hamming(phash64(&a), phash64(&b));
        assert!(d <= 12, "压缩噪声下的 pHash 距离应很小，实际 {d}");
    }

    #[test]
    fn phash_separates_different_pictures() {
        let a = picture(0.0, 70);
        let b = picture(2.2, -50);
        let d = hamming(phash64(&a), phash64(&b));
        assert!(d >= 12, "不同画面应相差较多位，实际 {d}");
    }

    #[test]
    fn hamming_counts_differing_bits() {
        assert_eq!(hamming(0, 0), 0);
        assert_eq!(hamming(0b1011, 0b0010), 2);
        assert_eq!(hamming(u64::MAX, 0), 64);
    }
}
