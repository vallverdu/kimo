//! Reading process resources from /proc.

use crate::core::procfs::{parse_stat, ProcTable};
use std::fs;

pub fn page_size() -> u64 {
    // SAFETY: sysconf has no preconditions.
    let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if v > 0 {
        v as u64
    } else {
        4096
    }
}

pub fn ticks_per_sec() -> f64 {
    // SAFETY: sysconf has no preconditions.
    let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if v > 0 {
        v as f64
    } else {
        100.0
    }
}

/// Snapshot of every process on the machine.
pub fn read_table() -> ProcTable {
    let stats = fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
        })
        .filter_map(|e| fs::read_to_string(e.path().join("stat")).ok())
        .filter_map(|line| parse_stat(&line));
    ProcTable::new(stats)
}

/// Resident memory of kimo itself in bytes.
pub fn self_rss_bytes() -> u64 {
    fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|l| parse_stat(&l))
        .map(|s| s.rss_pages * page_size())
        .unwrap_or(0)
}

/// (total, available) system memory in bytes, from /proc/meminfo.
pub fn system_memory() -> (u64, u64) {
    let text = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = |key: &str| {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    (kb("MemTotal:"), kb("MemAvailable:"))
}
