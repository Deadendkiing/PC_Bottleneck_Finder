//! Turns recent measurements into plain-language findings:
//! *what* the bottleneck is, *which programs* cause it, and *what to do*.
//!
//! A resource only counts as a bottleneck when it stays under pressure for most of the
//! recent window. Short spikes (opening an app, a page load) are ignored.

use crate::knowledge::{self, Kind};
use crate::model::{GpuKind, ProcGroup, Snapshot, fmt_bytes, fmt_rate};
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
const GPU_BUSY: f32 = 90.0;
const VRAM_FULL: f32 = 90.0;
/// Integrated GPU this busy while the discrete one is nearly idle = app on the wrong GPU.
const IGPU_BUSY: f32 = 70.0;
const DGPU_IDLE: f32 = 15.0;
/// GPU use for a program to count as "a graphics-heavy app".
const GPU_HEAVY_APP: f64 = 40.0;

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
    Gpu,
    VideoMemory,
    WrongGpu,
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
        let severity = if pct.round() >= MEM_CRIT || heavy_paging {
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
            // Core Windows processes (e.g. 80 svchost.exe added together) always look big
            // but there's nothing the user can do about them.
            .filter(|(g, _)| knowledge::lookup(&g.name).kind != Kind::System)
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
        let severity = if pct.round() >= CPU_CRIT {
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
        let severity = if pct.round() >= DISK_CRIT {
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

    // ---- GPU ------------------------------------------------------------------------
    for (i, gpu) in latest.gpus.iter().enumerate() {
        let util = |s: &Snapshot| s.gpus.get(i).map_or(0.0, |g| g.util_pct);
        let on_this_gpu = |g: &ProcGroup| {
            if g.gpu_index == Some(i) {
                g.gpu_pct as f64
            } else {
                0.0
            }
        };

        // A laptop running a heavy app on the integrated GPU while the discrete one idles.
        let idle_dgpu = latest.gpus.iter().enumerate().find(|&(j, g)| {
            j != i
                && g.kind == GpuKind::Discrete
                && sustained(window, |s| {
                    s.gpus.get(j).is_some_and(|g| g.util_pct < DGPU_IDLE)
                })
        });
        if gpu.kind == GpuKind::Integrated
            && sustained(window, |s| util(s) >= IGPU_BUSY)
            && let Some((_, dgpu)) = idle_dgpu
            && let Some((g, v)) = top_groups(window, on_this_gpu).into_iter().find(|(g, v)| {
                *v >= GPU_HEAVY_APP && !knowledge::is_protected(&g.name, &g.pids, own_pid)
            })
        {
            let app = knowledge::display_name(&g.name);
            let igpu_pct = avg(window, |s| util(s) as f64);
            findings.push(Finding {
                resource: Resource::WrongGpu,
                severity: Severity::Warning,
                headline: format!("{app} is running on your integrated graphics"),
                detail: format!(
                    "{app} keeps {} at {igpu_pct:.0}% while your {} is almost idle. \
                     The integrated chip is much slower, so games and 3D apps get low frame rates.",
                    gpu.name, dgpu.name
                ),
                culprits: vec![culprit(
                    &g,
                    format!("{v:.0}% of {}", gpu.name),
                    format!(
                        "Open Settings > System > Display > Graphics, choose {app}, set it to High performance ({}), then restart {app}.",
                        dgpu.name
                    ),
                    own_pid,
                )],
                tip: Some("Also plug in your charger: many laptops hold back the discrete GPU on battery.".into()),
            });
            continue;
        }

        if sustained(window, |s| util(s) >= GPU_BUSY) {
            let pct = avg(window, |s| util(s) as f64);
            let culprits = top_groups(window, on_this_gpu)
                .into_iter()
                .filter(|(_, v)| *v >= 3.0)
                .take(MAX_CULPRITS)
                .map(|(g, v)| {
                    culprit(
                        &g,
                        format!("{v:.0}% of this GPU"),
                        gpu_advice(&g.name, v),
                        own_pid,
                    )
                })
                .collect();
            findings.push(Finding {
                resource: Resource::Gpu,
                severity: Severity::Warning,
                headline: format!("Your graphics card is the bottleneck ({})", gpu.name),
                detail: format!(
                    "It has averaged {pct:.0}% over the last {secs} seconds, mostly on its {} engine.",
                    engine_label(&gpu.top_engine)
                ),
                culprits,
                tip: Some(
                    "While gaming this is normal: the graphics card is running at full speed and the rest of your PC isn't holding it back."
                        .into(),
                ),
            });
        }

        let vram = |s: &Snapshot| s.gpus.get(i).and_then(|g| g.vram_pct()).unwrap_or(0.0);
        if gpu.vram_pct().is_some() && sustained(window, |s| vram(s) >= VRAM_FULL) {
            let culprits = top_groups(window, |g| {
                if g.gpu_index == Some(i) { g.gpu_mem as f64 } else { 0.0 }
            })
            .into_iter()
            .filter(|(_, v)| *v >= 200.0 * 1024.0 * 1024.0)
            .take(MAX_CULPRITS)
            .map(|(g, v)| {
                let d = knowledge::display_name(&g.name);
                culprit(
                    &g,
                    format!("{} of video memory", fmt_bytes(v as u64)),
                    format!("Lower the texture quality in {d}, or close other programs that use the graphics card."),
                    own_pid,
                )
            })
            .collect();
            findings.push(Finding {
                resource: Resource::VideoMemory,
                severity: Severity::Warning,
                headline: format!("Video memory on {} is almost full", gpu.name),
                detail: format!(
                    "{} of {} ({:.0}%) is in use. When it runs out, games stutter and textures load slowly.",
                    fmt_bytes(gpu.dedicated_used),
                    fmt_bytes(gpu.dedicated_total),
                    vram(latest)
                ),
                culprits,
                tip: None,
            });
        }
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
        Kind::WebView => "Other apps (like Widgets, Teams or Outlook) use this to show web content. Close the apps you aren't using rather than ending it, which can break them.".into(),
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
        Kind::WebView => "An app that shows web content (like Widgets, Teams or Outlook) is busy. Close or restart that app.".into(),
        Kind::Other => format!("Closing {d} would free about {pct:.0}% of your CPU."),
    }
}

fn engine_label(engine: &str) -> &str {
    match engine {
        "3D" => "3D (drawing graphics)",
        "VideoDecode" | "Video Decode" => "video playback",
        "VideoEncode" | "Video Encode" => "video recording and streaming",
        "Copy" => "copy (moving data)",
        e if e.starts_with("Compute") => "compute",
        "" => "main",
        e => e,
    }
}

fn gpu_advice(name: &str, pct: f64) -> String {
    let k = knowledge::lookup(name);
    let d = knowledge::display_name(name);
    match k.kind {
        Kind::Browser => format!("Video or animations in {d}. Closing tabs that play video will help."),
        Kind::System if knowledge::normalize(name) == "dwm" => {
            "Windows uses this to draw your screen. Many windows or high refresh-rate monitors add to it.".into()
        }
        Kind::System | Kind::MemoryCompression | Kind::WindowsUpdate => {
            "This is part of Windows and shouldn't be closed.".into()
        }
        _ => format!(
            "{d} is using {pct:.0}% of the GPU. Lowering its resolution or graphics quality will reduce the load."
        ),
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
        Kind::WebView => {
            "An app that shows web content is saving its cache. It usually settles down on its own."
                .into()
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
            ..Default::default()
        }
    }

    fn gpu_snap(igpu: f32, dgpu: f32, vram_used: u64, groups: Vec<ProcGroup>) -> Snapshot {
        use crate::model::Gpu;
        let mut s = snap(30.0, 50.0, 5.0, groups);
        s.gpus = vec![
            Gpu {
                name: "Intel(R) UHD Graphics".into(),
                kind: GpuKind::Integrated,
                util_pct: igpu,
                top_engine: "3D".into(),
                dedicated_total: 128 << 20,
                ..Default::default()
            },
            Gpu {
                name: "NVIDIA GeForce RTX 4060 Laptop GPU".into(),
                kind: GpuKind::Discrete,
                util_pct: dgpu,
                top_engine: "3D".into(),
                dedicated_used: vram_used,
                dedicated_total: 8 * GB,
                ..Default::default()
            },
        ];
        s
    }

    fn on_gpu(name: &str, gpu: usize, pct: f32, vram: u64) -> ProcGroup {
        ProcGroup {
            gpu_index: Some(gpu),
            gpu_pct: pct,
            gpu_mem: vram,
            ..group(name, 1, 10.0, GB, 0.0)
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
        // svchost (80 Windows services added together) isn't actionable, so it's left out
        assert!(f.culprits.iter().all(|c| c.name != "svchost.exe"));
        assert_eq!(f.culprits[1].display, "Visual Studio Code");
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

    #[test]
    fn game_on_integrated_gpu_points_to_the_right_setting() {
        let h = history(15, |_| {
            gpu_snap(95.0, 2.0, 0, vec![on_gpu("SpaceRacer.exe", 0, 88.0, 0)])
        });
        let d = diagnose(&h, 1);
        assert_eq!(d.findings.len(), 1, "{:?}", d.findings);
        let f = &d.findings[0];
        assert_eq!(f.resource, Resource::WrongGpu);
        assert!(f.headline.contains("SpaceRacer.exe"));
        assert!(
            f.culprits[0]
                .advice
                .contains("High performance (NVIDIA GeForce RTX 4060 Laptop GPU)")
        );
    }

    #[test]
    fn busy_integrated_gpu_without_idle_discrete_is_a_plain_gpu_bottleneck() {
        // Discrete GPU also busy: not the "wrong GPU" problem.
        let h = history(15, |_| {
            gpu_snap(95.0, 60.0, 0, vec![on_gpu("SpaceRacer.exe", 0, 88.0, 0)])
        });
        let f = &diagnose(&h, 1).findings[0];
        assert_eq!(f.resource, Resource::Gpu);
    }

    #[test]
    fn gaming_on_discrete_gpu_is_reported_as_gpu_bound() {
        let h = history(15, |_| {
            gpu_snap(
                5.0,
                99.0,
                3 * GB,
                vec![on_gpu("SpaceRacer.exe", 1, 97.0, 3 * GB)],
            )
        });
        let d = diagnose(&h, 1);
        let f = d
            .findings
            .iter()
            .find(|f| f.resource == Resource::Gpu)
            .unwrap();
        assert!(f.headline.contains("NVIDIA"));
        assert!(f.detail.contains("3D (drawing graphics)"));
        assert!(f.tip.as_ref().unwrap().contains("normal"));
        assert!(f.culprits[0].advice.contains("graphics quality"));
        assert!(
            d.findings
                .iter()
                .all(|f| f.resource != Resource::VideoMemory)
        );
    }

    #[test]
    fn full_video_memory_is_reported() {
        let h = history(15, |_| {
            gpu_snap(
                5.0,
                50.0,
                7 * GB + GB / 2,
                vec![on_gpu("SpaceRacer.exe", 1, 50.0, 6 * GB)],
            )
        });
        let f = diagnose(&h, 1)
            .findings
            .into_iter()
            .find(|f| f.resource == Resource::VideoMemory)
            .unwrap();
        assert!(f.detail.contains("7.5 GB of 8.0 GB"));
        assert_eq!(f.culprits[0].usage, "6.0 GB of video memory");
    }

    #[test]
    fn severity_matches_the_rounded_percentage_shown() {
        // 94.6% is displayed as "95%", so it must also be reported as critical.
        let h = history(15, |_| {
            snap(
                20.0,
                94.6,
                5.0,
                vec![group("msedge.exe", 14, 1.0, 3 * GB, 0.0)],
            )
        });
        let f = &diagnose(&h, 1).findings[0];
        assert!(f.detail.contains("(95%)"), "{}", f.detail);
        assert_eq!(f.severity, Severity::Critical);
    }

    #[test]
    fn webview2_advice_points_to_the_app_using_it() {
        let h = history(15, |_| {
            snap(
                20.0,
                93.0,
                5.0,
                vec![group("msedgewebview2.exe", 6, 1.0, GB, 0.0)],
            )
        });
        let c = &diagnose(&h, 1).findings[0].culprits[0];
        assert_eq!(c.display, "Microsoft Edge WebView2");
        assert!(
            c.advice.contains("Close the apps you aren't using"),
            "{}",
            c.advice
        );
    }
}
