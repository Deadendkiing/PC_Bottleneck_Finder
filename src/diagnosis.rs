//! Turns recent measurements into plain-language findings:
//! *what* the bottleneck is, *which programs* cause it, and *what to do*.
//!
//! A resource only counts as a bottleneck when it stays under pressure for most of the
//! recent window. Short spikes (opening an app, a page load) are ignored.

use crate::knowledge::{self, Kind};
use crate::model::{ProcGroup, Snapshot, fmt_bytes, fmt_rate};
use std::collections::HashMap;

/// Number of recent samples (≈ seconds) the diagnosis looks at.
pub const WINDOW: usize = 15;
/// Samples needed before giving a verdict.
pub const MIN_SAMPLES: usize = 5;
/// Fraction of the window a condition must hold to count as sustained.
const SUSTAINED: f64 = 0.7;

const CPU_WARN: f32 = 85.0;
const CPU_CRIT: f32 = 95.0;
const MEM_WARN: f32 = 90.0;
const MEM_WARN_WITH_PAGING: f32 = 85.0;
const MEM_CRIT: f32 = 95.0;
const DISK_WARN: f32 = 90.0;
const DISK_CRIT: f32 = 98.0;
/// Page-file reads per second that mean RAM is really short (~4 MB/s of 4 KB pages).
const HEAVY_PAGING: f64 = 1000.0;
const SINGLE_CORE_BUSY: f32 = 90.0;
const SINGLE_CORE_MAX_TOTAL: f32 = 60.0;
const MAX_CULPRITS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Memory,
    Cpu,
    Disk,
    SingleCore,
}

#[derive(Clone, Debug)]
pub struct Culprit {
    /// Executable name, e.g. `chrome.exe`.
    pub name: String,
    /// Friendly name, e.g. `Google Chrome`.
    pub display: String,
    pub pids: Vec<u32>,
    /// How much it uses, e.g. `5.2 GB across 38 processes`.
    pub usage: String,
    pub advice: String,
    pub can_close: bool,
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub resource: Resource,
    pub severity: Severity,
    pub headline: String,
    pub detail: String,
    pub culprits: Vec<Culprit>,
    /// Longer-term suggestion, e.g. an upgrade.
    pub tip: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Diagnosis {
    /// Most important first. Empty means no bottleneck.
    pub findings: Vec<Finding>,
    /// False while too few samples have been collected.
    pub ready: bool,
}

pub fn diagnose(history: &[Snapshot], own_pid: u32) -> Diagnosis {
    if history.len() < MIN_SAMPLES {
        return Diagnosis {
            findings: vec![],
            ready: false,
        };
    }
    let window = &history[history.len().saturating_sub(WINDOW)..];
    let latest = window.last().expect("window is non-empty");
    let secs = window.len();
    let mut findings = Vec::new();

    // ---- Memory -------------------------------------------------------------------
    let paging = |s: &Snapshot| s.paging_per_sec.unwrap_or(0.0) >= HEAVY_PAGING;
    let heavy_paging = sustained(window, paging);
    let mem_pressure = sustained(window, |s| {
        s.mem_pct() >= MEM_WARN || (s.mem_pct() >= MEM_WARN_WITH_PAGING && paging(s))
    });
    if mem_pressure {
        let pct = avg(window, |s| s.mem_pct() as f64) as f32;
        let severity = if pct >= MEM_CRIT || heavy_paging {
            Severity::Critical
        } else {
            Severity::Warning
        };
        let tail = if heavy_paging {
            ", and Windows is moving data to the page file on disk, which is much slower than RAM."
        } else {
            "."
        };
        let culprits = top_groups(window, |g| g.mem as f64)
            .into_iter()
            .filter(|(_, v)| *v >= 100.0 * 1024.0 * 1024.0)
            .take(MAX_CULPRITS)
            .map(|(g, v)| {
                let usage = with_count(fmt_bytes(v as u64), &g);
                culprit(&g, usage, memory_advice(&g.name, v as u64), own_pid)
            })
            .collect();
        findings.push(Finding {
            resource: Resource::Memory,
            severity,
            headline: "Memory (RAM) is your bottleneck".into(),
            detail: format!(
                "{} of {} ({pct:.0}%) is in use{tail}",
                fmt_bytes(latest.mem_used),
                fmt_bytes(latest.mem_total)
            ),
            culprits,
            tip: Some(
                "If this happens often, adding more RAM is the most effective upgrade.".into(),
            ),
        });
    }

    // ---- CPU ----------------------------------------------------------------------
    if sustained(window, |s| s.cpu_pct >= CPU_WARN) {
        let pct = avg(window, |s| s.cpu_pct as f64) as f32;
        let severity = if pct >= CPU_CRIT {
            Severity::Critical
        } else {
            Severity::Warning
        };
        let culprits = top_groups(window, |g| g.cpu_pct as f64)
            .into_iter()
            .filter(|(_, v)| *v >= 3.0)
            .take(MAX_CULPRITS)
            .map(|(g, v)| {
                let usage = with_count(format!("{v:.0}% of CPU"), &g);
                culprit(&g, usage, cpu_advice(&g.name, v), own_pid)
            })
            .collect();
        findings.push(Finding {
            resource: Resource::Cpu,
            severity,
            headline: "CPU is your bottleneck".into(),
            detail: format!("Your processor has averaged {pct:.0}% over the last {secs} seconds."),
            culprits,
            tip: None,
        });
    }

    // ---- Disk ---------------------------------------------------------------------
    if sustained(window, |s| s.disk_busy_pct.unwrap_or(0.0) >= DISK_WARN) {
        let pct = avg(window, |s| s.disk_busy_pct.unwrap_or(0.0) as f64) as f32;
        let severity = if pct >= DISK_CRIT {
            Severity::Critical
        } else {
            Severity::Warning
        };
        let disk = latest
            .disk_name
            .as_deref()
            .map(|n| format!("Disk {n}"))
            .unwrap_or_else(|| "Your disk".into());
        let mut detail = format!(
            "{disk} has been busy {pct:.0}% of the time over the last {secs} seconds, so programs have to wait for it."
        );
        if mem_pressure && heavy_paging {
            detail.push_str(
                " Part of this is Windows paging memory to disk because RAM is full, so fixing the memory problem will help here too.",
            );
        }
        let culprits = top_groups(window, |g| g.disk_bps)
            .into_iter()
            .filter(|(_, v)| *v >= 512.0 * 1024.0)
            .take(MAX_CULPRITS)
            .map(|(g, v)| {
                let usage = with_count(fmt_rate(v), &g);
                culprit(&g, usage, disk_advice(&g.name), own_pid)
            })
            .collect();
        findings.push(Finding {
            resource: Resource::Disk,
            severity,
            headline: "Your disk is the bottleneck".into(),
            detail,
            culprits,
            tip: Some(
                "If this is a hard drive (HDD), switching to an SSD makes the biggest difference."
                    .into(),
            ),
        });
    }

    // ---- Single-threaded program ----------------------------------------------------
    // Only meaningful when the CPU as a whole is mostly idle.
    let cores = latest.core_pcts.len();
    let cpu_found = findings.iter().any(|f| f.resource == Resource::Cpu);
    if cores >= 4 && !cpu_found && sustained(window, |s| s.cpu_pct < SINGLE_CORE_MAX_TOTAL) {
        let busy = top_groups(window, |g| g.max_core_pct as f64)
            .into_iter()
            .find(|(g, v)| {
                *v >= SINGLE_CORE_BUSY as f64
                    && !knowledge::is_protected(&g.name, &g.pids, own_pid)
                    && sustained(window, |s| {
                        s.groups
                            .iter()
                            .any(|x| x.name == g.name && x.max_core_pct >= SINGLE_CORE_BUSY)
                    })
            });
        if let Some((g, core)) = busy {
            let total = avg(window, |s| s.cpu_pct as f64);
            let name = knowledge::display_name(&g.name);
            findings.push(Finding {
                resource: Resource::SingleCore,
                severity: Severity::Info,
                headline: format!("{name} is limited by a single CPU core"),
                detail: format!(
                    "Your CPU is only {total:.0}% busy overall, but {name} keeps one core at {core:.0}%. \
                     It can't spread its work over your other {} cores, so it runs only as fast as one core allows. \
                     This is a limit of the program, not a problem with your PC.",
                    cores - 1
                ),
                culprits: vec![culprit(
                    &g,
                    format!("{core:.0}% of one core"),
                    "Nothing to fix on your PC. If it's your own code, multithreading or multiprocessing would let it use more cores.".into(),
                    own_pid,
                )],
                tip: None,
            });
        }
    }

    findings.sort_by_key(|f| std::cmp::Reverse(f.severity));
    Diagnosis {
        findings,
        ready: true,
    }
}

fn sustained(window: &[Snapshot], cond: impl Fn(&Snapshot) -> bool) -> bool {
    let hits = window.iter().filter(|s| cond(s)).count();
    hits as f64 >= (window.len() as f64 * SUSTAINED).ceil()
}

fn avg(window: &[Snapshot], f: impl Fn(&Snapshot) -> f64) -> f64 {
    window.iter().map(f).sum::<f64>() / window.len() as f64
}

/// Groups ranked by their average `metric` over the window (absent = 0).
/// Returns the most recent copy of each group along with its average.
fn top_groups(window: &[Snapshot], metric: impl Fn(&ProcGroup) -> f64) -> Vec<(ProcGroup, f64)> {
    let mut acc: HashMap<&str, (f64, &ProcGroup)> = HashMap::new();
    for s in window {
        for g in &s.groups {
            let e = acc.entry(g.name.as_str()).or_insert((0.0, g));
            e.0 += metric(g);
            e.1 = g;
        }
    }
    let n = window.len() as f64;
    let mut v: Vec<(ProcGroup, f64)> = acc
        .into_values()
        .map(|(sum, g)| (g.clone(), sum / n))
        .collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v
}

fn with_count(usage: String, g: &ProcGroup) -> String {
    if g.pids.len() > 1 {
        format!("{usage} across {} processes", g.pids.len())
    } else {
        usage
    }
}

fn culprit(g: &ProcGroup, usage: String, advice: String, own_pid: u32) -> Culprit {
    Culprit {
        name: g.name.clone(),
        display: knowledge::display_name(&g.name),
        pids: g.pids.clone(),
        usage,
        advice,
        can_close: !knowledge::is_protected(&g.name, &g.pids, own_pid),
    }
}

fn memory_advice(name: &str, bytes: u64) -> String {
    let k = knowledge::lookup(name);
    let d = knowledge::display_name(name);
    let amount = fmt_bytes(bytes);
    match k.kind {
        Kind::Browser => format!(
            "Close tabs you aren't using. Closing {d} completely would free about {amount}."
        ),
        Kind::MemoryCompression => "Windows is compressing memory because RAM is full. This is a symptom, not the cause; it shrinks when you close other programs.".into(),
        Kind::Antivirus => "This is your antivirus. Don't close it.".into(),
        Kind::System | Kind::WindowsUpdate => "This is part of Windows and shouldn't be closed.".into(),
        Kind::SearchIndexer => "Windows is indexing your files for search. It usually settles down on its own.".into(),
        Kind::Other => format!("Closing {d} would free about {amount}."),
    }
}

fn cpu_advice(name: &str, pct: f64) -> String {
    let k = knowledge::lookup(name);
    let d = knowledge::display_name(name);
    match k.kind {
        Kind::Browser => format!(
            "A tab or extension is busy. Press Shift+Esc in {d} to see which one, and close it."
        ),
        Kind::Antivirus => "Microsoft Defender is scanning. This is usually temporary; let it finish.".into(),
        Kind::WindowsUpdate => "Windows Update is installing updates. Let it finish; it stops on its own.".into(),
        Kind::SearchIndexer => "Windows is indexing your files for search. It usually finishes on its own.".into(),
        Kind::MemoryCompression => "Windows is compressing memory because RAM is low. Free up memory to reduce this.".into(),
        Kind::System => "This is part of Windows. If it stays high for a long time, restarting your PC usually helps.".into(),
        Kind::Other => format!("Closing {d} would free about {pct:.0}% of your CPU."),
    }
}

fn disk_advice(name: &str) -> String {
    let k = knowledge::lookup(name);
    let d = knowledge::display_name(name);
    match k.kind {
        Kind::Browser => {
            format!("{d} is writing its cache or a download. Pausing downloads will help.")
        }
        Kind::Antivirus => {
            "Microsoft Defender is scanning files. This is usually temporary; let it finish.".into()
        }
        Kind::WindowsUpdate => {
            "Windows Update is installing updates. Let it finish; it stops on its own.".into()
        }
        Kind::SearchIndexer => {
            "Windows is indexing your files for search. It usually finishes on its own.".into()
        }
        Kind::MemoryCompression | Kind::System => {
            "This is part of Windows and shouldn't be closed.".into()
        }
        Kind::Other => format!("Closing {d} would reduce the load on your disk."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn group(name: &str, n: usize, cpu: f32, mem: u64, disk: f64) -> ProcGroup {
        ProcGroup {
            name: name.into(),
            pids: (1000..1000 + n as u32).collect(),
            cpu_pct: cpu,
            max_core_pct: cpu,
            mem,
            disk_bps: disk,
        }
    }

    fn snap(cpu: f32, mem_pct: f32, disk: f32, groups: Vec<ProcGroup>) -> Snapshot {
        let total = 16 * GB;
        Snapshot {
            cpu_pct: cpu,
            core_pcts: vec![cpu; 8],
            mem_total: total,
            mem_used: (total as f64 * mem_pct as f64 / 100.0) as u64,
            disk_busy_pct: Some(disk),
            disk_name: Some("0 C:".into()),
            groups,
            ..Default::default()
        }
    }

    fn history(n: usize, f: impl Fn(usize) -> Snapshot) -> Vec<Snapshot> {
        (0..n).map(f).collect()
    }

    #[test]
    fn waits_for_enough_samples() {
        let h = history(3, |_| snap(100.0, 99.0, 100.0, vec![]));
        assert!(!diagnose(&h, 1).ready);
    }

    #[test]
    fn healthy_system_has_no_findings() {
        let h = history(15, |_| {
            snap(
                20.0,
                50.0,
                10.0,
                vec![group("chrome.exe", 10, 5.0, 2 * GB, 0.0)],
            )
        });
        let d = diagnose(&h, 1);
        assert!(d.ready);
        assert!(d.findings.is_empty());
    }

    #[test]
    fn memory_bottleneck_blames_chrome() {
        let h = history(15, |_| {
            snap(
                30.0,
                94.0,
                10.0,
                vec![
                    group("chrome.exe", 38, 5.0, 5 * GB + GB / 5, 0.0),
                    group("Code.exe", 6, 2.0, GB, 0.0),
                    group("svchost.exe", 80, 1.0, 2 * GB, 0.0),
                ],
            )
        });
        let d = diagnose(&h, 1);
        let f = &d.findings[0];
        assert_eq!(f.resource, Resource::Memory);
        assert_eq!(f.severity, Severity::Warning);
        let top = &f.culprits[0];
        assert_eq!(top.display, "Google Chrome");
        assert_eq!(top.usage, "5.2 GB across 38 processes");
        assert!(top.advice.contains("Close tabs"), "{}", top.advice);
        assert!(top.can_close);
        // svchost is ranked but must never be closable
        let svc = f.culprits.iter().find(|c| c.name == "svchost.exe").unwrap();
        assert!(!svc.can_close);
    }

    #[test]
    fn short_spike_is_ignored() {
        // CPU at 100% for 3 of 15 seconds: not sustained.
        let h = history(15, |i| {
            snap(if i < 3 { 100.0 } else { 20.0 }, 50.0, 5.0, vec![])
        });
        assert!(diagnose(&h, 1).findings.is_empty());
    }

    #[test]
    fn cpu_bottleneck_is_critical_and_ranks_culprits() {
        let h = history(15, |_| {
            snap(
                98.0,
                50.0,
                5.0,
                vec![
                    group("blender.exe", 1, 70.0, GB, 0.0),
                    group("MsMpEng.exe", 1, 20.0, GB / 4, 0.0),
                ],
            )
        });
        let f = &diagnose(&h, 1).findings[0];
        assert_eq!(f.resource, Resource::Cpu);
        assert_eq!(f.severity, Severity::Critical);
        assert_eq!(f.culprits[0].name, "blender.exe");
        assert!(f.culprits[0].advice.contains("70%"));
        assert!(f.culprits[1].advice.contains("Defender"));
        assert!(!f.culprits[1].can_close);
    }

    #[test]
    fn disk_bottleneck_links_to_memory_paging() {
        let h = history(15, |_| {
            let mut s = snap(
                30.0,
                96.0,
                100.0,
                vec![group("SearchIndexer.exe", 1, 2.0, GB / 2, 20e6)],
            );
            s.paging_per_sec = Some(3000.0);
            s
        });
        let d = diagnose(&h, 1);
        let mem = d
            .findings
            .iter()
            .find(|f| f.resource == Resource::Memory)
            .unwrap();
        assert_eq!(mem.severity, Severity::Critical);
        assert!(mem.detail.contains("page file"));
        let disk = d
            .findings
            .iter()
            .find(|f| f.resource == Resource::Disk)
            .unwrap();
        assert!(disk.detail.contains("Disk 0 C:"));
        assert!(disk.detail.contains("paging memory"));
        assert!(disk.culprits[0].advice.contains("indexing"));
    }

    #[test]
    fn detects_single_threaded_program() {
        let h = history(15, |_| {
            let mut g = group("python.exe", 1, 12.5, GB / 2, 0.0);
            g.max_core_pct = 100.0;
            snap(15.0, 40.0, 5.0, vec![g])
        });
        let f = &diagnose(&h, 1).findings[0];
        assert_eq!(f.resource, Resource::SingleCore);
        assert_eq!(f.severity, Severity::Info);
        assert!(f.headline.contains("Python"));
    }

    #[test]
    fn most_severe_finding_comes_first() {
        let h = history(15, |_| snap(88.0, 97.0, 5.0, vec![]));
        let d = diagnose(&h, 1);
        assert_eq!(d.findings[0].resource, Resource::Memory); // critical
        assert_eq!(d.findings[1].resource, Resource::Cpu); // warning
    }
}
