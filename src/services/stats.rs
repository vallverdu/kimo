//! Process and memory figures: /proc on Linux, `ps`, `sysctl` and `vm_stat` on macOS.

#[cfg(target_os = "linux")]
use crate::core::procfs::parse_stat;
use crate::core::procfs::ProcTable;
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(not(target_os = "linux"))]
use std::process::Command;

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
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
pub fn self_rss_bytes() -> u64 {
    fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|l| parse_stat(&l))
        .map(|s| s.rss_pages * page_size())
        .unwrap_or(0)
}

/// (total, available) system memory in bytes, from /proc/meminfo.
#[cfg(target_os = "linux")]
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

#[cfg(not(target_os = "linux"))]
fn run(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Snapshot of every process on the machine (from `ps`).
#[cfg(not(target_os = "linux"))]
pub fn read_table() -> ProcTable {
    let page = page_size();
    let out = run("ps", &["-axo", "pid=,ppid=,rss=,time="]);
    ProcTable::new(
        out.lines()
            .filter_map(|l| crate::core::procfs::parse_ps_line(l, page)),
    )
}

/// Resident memory of kimo itself in bytes.
#[cfg(not(target_os = "linux"))]
pub fn self_rss_bytes() -> u64 {
    run("ps", &["-o", "rss=", "-p", &std::process::id().to_string()])
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        * 1024
}

/// (total, available) system memory in bytes: `sysctl hw.memsize` and `vm_stat`.
#[cfg(not(target_os = "linux"))]
pub fn system_memory() -> (u64, u64) {
    let total = run("sysctl", &["-n", "hw.memsize"])
        .trim()
        .parse()
        .unwrap_or(0);
    (total, parse_vm_stat(&run("vm_stat", &[])))
}

/// Free + inactive + speculative pages from `vm_stat`, in bytes.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub fn parse_vm_stat(out: &str) -> u64 {
    let page: u64 = out
        .lines()
        .next()
        .and_then(|l| l.split("page size of ").nth(1))
        .and_then(|r| r.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(4096);
    let pages = |key: &str| {
        out.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().trim_end_matches('.').parse::<u64>().ok())
            .unwrap_or(0)
    };
    (pages("Pages free") + pages("Pages inactive") + pages("Pages speculative")) * page
}

#[cfg(test)]
mod tests {
    #[test]
    fn vm_stat() {
        let out = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               10.\nPages active:                            99.\nPages inactive:                           5.\nPages speculative:                        1.\n";
        assert_eq!(super::parse_vm_stat(out), 16 * 16384);
    }
}
