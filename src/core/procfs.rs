//! Parsing `/proc/<pid>/stat` and summing a process tree's resources (pure, Linux format).

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcStat {
    pub pid: i32,
    pub ppid: i32,
    pub state: char,
    /// utime + stime in clock ticks.
    pub cpu_ticks: u64,
    pub rss_pages: u64,
}

/// Parse one `/proc/<pid>/stat` line. The command name may contain spaces and parentheses,
/// so fields are read after the last `)`.
pub fn parse_stat(line: &str) -> Option<ProcStat> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let pid = line[..open].trim().parse().ok()?;
    let rest: Vec<&str> = line[close + 1..].split_whitespace().collect();
    // rest[0] = state (field 3), so field n is rest[n - 3].
    let field = |n: usize| rest.get(n - 3).copied();
    Some(ProcStat {
        pid,
        state: field(3)?.chars().next()?,
        ppid: field(4)?.parse().ok()?,
        cpu_ticks: field(14)?.parse::<u64>().ok()? + field(15)?.parse::<u64>().ok()?,
        rss_pages: field(24)?.parse().ok()?,
    })
}

/// Sum of a process and all its descendants.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeUsage {
    pub processes: u32,
    pub cpu_ticks: u64,
    pub rss_pages: u64,
}

pub struct ProcTable {
    by_pid: HashMap<i32, ProcStat>,
    children: HashMap<i32, Vec<i32>>,
}

impl ProcTable {
    pub fn new(stats: impl IntoIterator<Item = ProcStat>) -> Self {
        let mut by_pid = HashMap::new();
        let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
        for s in stats {
            children.entry(s.ppid).or_default().push(s.pid);
            by_pid.insert(s.pid, s);
        }
        Self { by_pid, children }
    }

    pub fn tree_usage(&self, root: i32) -> TreeUsage {
        let mut usage = TreeUsage::default();
        let mut stack = vec![root];
        while let Some(pid) = stack.pop() {
            if let Some(s) = self.by_pid.get(&pid) {
                usage.processes += 1;
                usage.cpu_ticks += s.cpu_ticks;
                usage.rss_pages += s.rss_pages;
            }
            if let Some(kids) = self.children.get(&pid) {
                stack.extend(kids);
            }
        }
        usage
    }
}

/// CPU percentage (100 = one full core) between two tick samples.
pub fn cpu_percent(prev_ticks: u64, now_ticks: u64, elapsed_secs: f64, ticks_per_sec: f64) -> f64 {
    if elapsed_secs <= 0.0 || ticks_per_sec <= 0.0 {
        return 0.0;
    }
    now_ticks.saturating_sub(prev_ticks) as f64 / ticks_per_sec / elapsed_secs * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "4242 (my (odd) cmd) S 100 4242 4242 34816 4242 4194304 1 0 0 0 \
        25 15 0 0 20 0 1 0 12345 123456789 2048 18446744073709551615";

    #[test]
    fn parses_stat_with_tricky_name() {
        let s = parse_stat(LINE).unwrap();
        assert_eq!(
            (s.pid, s.ppid, s.state, s.cpu_ticks, s.rss_pages),
            (4242, 100, 'S', 40, 2048)
        );
        assert!(parse_stat("garbage").is_none());
    }

    #[test]
    fn sums_descendants_only() {
        let p = |pid, ppid, rss| ProcStat {
            pid,
            ppid,
            state: 'S',
            cpu_ticks: 1,
            rss_pages: rss,
        };
        let t = ProcTable::new([p(10, 1, 100), p(11, 10, 20), p(12, 11, 3), p(20, 1, 999)]);
        assert_eq!(
            t.tree_usage(10),
            TreeUsage {
                processes: 3,
                cpu_ticks: 3,
                rss_pages: 123
            }
        );
        assert_eq!(t.tree_usage(77).processes, 0);
    }

    #[test]
    fn cpu_percent_of_one_core() {
        assert_eq!(cpu_percent(100, 200, 1.0, 100.0), 100.0);
        assert_eq!(cpu_percent(200, 100, 1.0, 100.0), 0.0);
        assert_eq!(cpu_percent(0, 50, 0.0, 100.0), 0.0);
    }
}
