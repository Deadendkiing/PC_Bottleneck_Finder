//! Simulated data for `--demo`: a gaming laptop with integrated + discrete GPUs, where
//! RAM is nearly full and a game is accidentally running on the integrated GPU.
//! Used to preview the interface and for screenshots on machines without these problems.

use crate::model::{Gpu, GpuKind, ProcGroup, Snapshot};
use crate::sampler::Source;

const GB: u64 = 1024 * 1024 * 1024;
const MB: u64 = 1024 * 1024;

struct Proc {
    name: &'static str,
    count: usize,
    cpu: f32,
    mem: u64,
    disk: f64,
    gpu: Option<(usize, f32, u64)>,
}

pub struct Demo {
    t: f64,
    seed: u64,
    procs: Vec<Proc>,
}

impl Demo {
    pub fn new() -> Self {
        let p = |name, count, cpu, mem, disk, gpu| Proc {
            name,
            count,
            cpu,
            mem,
            disk,
            gpu,
        };
        Demo {
            t: 0.0,
            seed: 7,
            procs: vec![
                p(
                    "chrome.exe",
                    38,
                    9.0,
                    4 * GB + 900 * MB,
                    0.4e6,
                    Some((0, 4.0, 0)),
                ),
                p(
                    "SpaceRacer.exe",
                    1,
                    22.0,
                    2 * GB + 900 * MB,
                    1.5e6,
                    Some((0, 88.0, 0)),
                ),
                p("svchost.exe", 84, 2.5, GB + 400 * MB, 0.2e6, None),
                p("Code.exe", 9, 1.5, GB + 200 * MB, 0.05e6, Some((0, 0.5, 0))),
                p("ms-teams.exe", 6, 2.0, 900 * MB, 0.02e6, Some((0, 1.0, 0))),
                p("Memory Compression", 1, 0.8, 640 * MB, 0.0, None),
                p("explorer.exe", 1, 0.6, 310 * MB, 0.01e6, None),
                p("dwm.exe", 1, 1.8, 220 * MB, 0.0, Some((0, 6.0, 0))),
                p("MsMpEng.exe", 1, 1.2, 280 * MB, 0.6e6, None),
                p("OneDrive.exe", 1, 0.3, 180 * MB, 0.3e6, None),
                p("Discord.exe", 7, 0.9, 610 * MB, 0.01e6, Some((0, 0.6, 0))),
                p("Spotify.exe", 5, 0.7, 420 * MB, 0.02e6, None),
                p(
                    "nvcontainer.exe",
                    3,
                    0.1,
                    95 * MB,
                    0.0,
                    Some((1, 1.5, 300 * MB)),
                ),
                p("SearchIndexer.exe", 1, 0.2, 85 * MB, 0.1e6, None),
                p("steam.exe", 1, 0.4, 210 * MB, 0.05e6, None),
                p(
                    "steamwebhelper.exe",
                    8,
                    0.6,
                    520 * MB,
                    0.02e6,
                    Some((0, 0.4, 0)),
                ),
                p("RuntimeBroker.exe", 6, 0.1, 110 * MB, 0.0, None),
                p("csrss.exe", 2, 0.2, 12 * MB, 0.0, None),
                p("lsass.exe", 1, 0.1, 25 * MB, 0.0, None),
                p("audiodg.exe", 1, 0.4, 18 * MB, 0.0, None),
            ],
        }
    }

    /// Deterministic noise in -1..1.
    fn noise(&mut self) -> f32 {
        self.seed = self
            .seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.seed >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }
}

impl Source for Demo {
    fn sample(&mut self) -> Snapshot {
        self.t += 1.0;
        let t = self.t as f32;
        let mut groups = Vec::new();
        let mut gpu_util = [0.0f32; 2];
        let mut pid = 1000u32;
        for i in 0..self.procs.len() {
            let wave = 1.0 + 0.25 * (t * 0.4 + i as f32).sin() + 0.1 * self.noise();
            let p = &self.procs[i];
            let pids: Vec<u32> = (pid..pid + p.count as u32).collect();
            pid += p.count as u32 + 10;
            let (gpu_index, gpu_pct, gpu_mem) = match p.gpu {
                Some((a, u, m)) => {
                    let u = (u * (1.0 + 0.04 * (t * 0.7 + i as f32).sin())).min(100.0);
                    gpu_util[a] += u;
                    (Some(a), u, m)
                }
                None => (None, 0.0, 0),
            };
            groups.push(ProcGroup {
                name: p.name.into(),
                pids,
                cpu_pct: p.cpu * wave,
                max_core_pct: (p.cpu * wave * 4.0).min(100.0),
                mem: p.mem + (p.mem as f32 * 0.01 * (t * 0.2 + i as f32).sin()) as u64,
                disk_bps: p.disk * wave as f64,
                gpu_pct,
                gpu_index,
                gpu_mem,
            });
        }
        let cpu: f32 = groups.iter().map(|g| g.cpu_pct).sum::<f32>() + 3.0;
        let mem_total = 16 * GB;
        let mem_used = (groups.iter().map(|g| g.mem).sum::<u64>()).min(mem_total - 200 * MB);
        let disk_bps = groups.iter().map(|g| g.disk_bps).sum();
        let core_pcts = (0..16)
            .map(|c| {
                (cpu * (0.6 + 0.8 * ((c as f32 * 1.7 + t * 0.5).sin() * 0.5 + 0.5))).min(100.0)
            })
            .collect();
        let gpus = vec![
            Gpu {
                name: "Intel(R) UHD Graphics".into(),
                kind: GpuKind::Integrated,
                util_pct: gpu_util[0].min(100.0),
                top_engine: "3D".into(),
                dedicated_used: 0,
                dedicated_total: 128 * MB,
                shared_used: 2 * GB + 300 * MB,
                shared_total: 8 * GB,
            },
            Gpu {
                name: "NVIDIA GeForce RTX 4060 Laptop GPU".into(),
                kind: GpuKind::Discrete,
                util_pct: gpu_util[1],
                top_engine: "3D".into(),
                dedicated_used: 420 * MB,
                dedicated_total: 8 * GB,
                shared_used: 40 * MB,
                shared_total: 8 * GB,
            },
        ];
        let n = self.noise();
        Snapshot {
            cpu_pct: cpu.min(100.0),
            core_pcts,
            mem_used,
            mem_total,
            disk_busy_pct: Some((22.0 + 12.0 * (t * 0.3).sin() + 4.0 * n).clamp(0.0, 100.0)),
            disk_name: Some("0 C:".into()),
            paging_per_sec: Some(250.0),
            disk_bps,
            net_rx_bps: 1.8e6 * (1.0 + 0.5 * (t * 0.25).sin()) as f64,
            net_tx_bps: 0.2e6,
            gpus,
            groups,
        }
    }

    fn kill(&mut self, pids: &[u32]) -> usize {
        // Remove the demo program whose process IDs were passed in.
        let mut pid = 1000u32;
        let mut remove = None;
        for (i, p) in self.procs.iter().enumerate() {
            if pids.first() == Some(&pid) {
                remove = Some(i);
            }
            pid += p.count as u32 + 10;
        }
        if let Some(i) = remove {
            self.procs.remove(i);
        }
        0
    }
}
