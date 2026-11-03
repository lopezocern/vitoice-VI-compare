//! 轻量日志：写入 %LOCALAPPDATA%\VitoiceCompare\logs\app.log，超过 2MB 轮转一次。
//! 同时输出到 stderr，方便从终端启动时直接观察。

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static WRITER: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

pub fn log_dir() -> PathBuf {
    crate::config::data_root().join("logs")
}

#[cfg(windows)]
fn local_parts(epoch: i64) -> Option<(i32, i32, i32, i32, i32, i32)> {
    #[repr(C)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
    }
    unsafe extern "C" {
        fn _localtime64_s(tm: *mut Tm, time: *const i64) -> i32;
    }
    let mut tm = Tm {
        sec: 0,
        min: 0,
        hour: 0,
        mday: 1,
        mon: 0,
        year: 70,
        wday: 0,
        yday: 0,
        isdst: 0,
    };
    let t = epoch;
    let r = unsafe { _localtime64_s(&mut tm, &t) };
    if r == 0 {
        Some((tm.year + 1900, tm.mon + 1, tm.mday, tm.hour, tm.min, tm.sec))
    } else {
        None
    }
}

#[cfg(not(windows))]
fn local_parts(_epoch: i64) -> Option<(i32, i32, i32, i32, i32, i32)> {
    None
}

fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;

    if let Some((y, mo, d, h, mi, s)) = local_parts(secs) {
        return format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}");
    }

    // 以本地时区偏移估算（Windows 上按 UTC+8 处理会不准，这里只求可读）
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    // civil_from_days：把 1970-01-01 起的天数换成 年月日
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if mo <= 2 { y + 1 } else { y };

    format!("{year:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

pub fn init() {
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);

    let path = dir.join("app.log");
    if let Ok(md) = std::fs::metadata(&path) {
        if md.len() > 2 * 1024 * 1024 {
            let _ = std::fs::rename(&path, dir.join("app.1.log"));
        }
    }

    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = WRITER.set(Mutex::new(f));
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log_raw(&format!("PANIC {info}"));
        default_hook(info);
    }));

    log("=== VitoiceCompare 启动 ===");
}

fn log_raw(msg: &str) {
    let line = format!("[{}] {}\n", stamp(), msg);
    if let Some(w) = WRITER.get() {
        if let Ok(mut f) = w.lock() {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
    }
    eprint!("{line}");
}

/// 写一条普通日志
pub fn log(msg: &str) {
    log_raw(msg);
}

/// 写一条错误日志（前缀便于 grep）
pub fn error(msg: &str) {
    log_raw(&format!("ERROR {msg}"));
}

#[cfg(test)]
mod tests {
    #[test]
    fn date_format_looks_sane() {
        // 只验证格式形状，不依赖具体日期
        let s = super::stamp();
        assert_eq!(s.len(), 19, "格式应为 YYYY-MM-DD HH:MM:SS，实际 {s}");
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[10..11], " ");
    }
}
