//! Collects one [`Snapshot`] of the whole system per call.

use crate::model::{ProcGroup, Snapshot};
use crate::platform::Counters;
use std::collections::HashMap;
use std::time::Instant;
use sysinfo::{Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Anything that can produce snapshots: the real system, or demo data.
pub trait Source: Send {
    fn sample(&mut self) -> Snapshot;
    /// Ends every process in `pids`. Returns how many could not be ended.
    fn kill(&mut self, pids: &[u32]) -> usize;
}

pub struct Sampler {
    sys: System,
    nets: Option<Networks>,
    counters: Counters,
    last: Instant,
}

impl Sampler {
    pub fn new() -> Result<Self, String> {
        let mut sys = System::new().map_err(|e| format!("Can't read system information: {e:?}"))?;
        sys.refresh_cpu_usage();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
        let now = Instant::now();
        Ok(Sampler {
            sys,
            nets: Networks::new_with_refreshed_list().ok(),
            counters: Counters::new(),
            last: now,
        })
    }
}

impl Source for Sampler {
    fn sample(&mut self) -> Snapshot {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64().max(0.001);
        self.last = now;

        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());

        let extra = self.counters.sample();
        let cores = self.sys.cpus().len().max(1) as f32;
        let mut groups: HashMap<String, ProcGroup> = HashMap::new();
        // Highest single-process GPU use seen per group, to pick the group's GPU.
        let mut best_gpu: HashMap<String, f32> = HashMap::new();
        for (pid, p) in self.sys.processes() {
            // Skip kernel threads listed as processes and the idle pseudo-process.
            if p.thread_kind().is_some() || pid.as_u32() == 0 {
                continue;
            }
            let name = p.name().to_string_lossy().into_owned();
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "idle" | "system idle process"
            ) {
                continue;
            }
            let du = p.disk_usage();
            let g = groups.entry(name.clone()).or_insert_with(|| ProcGroup {
                name,
                ..Default::default()
            });
            let cpu = p.cpu_usage();
            g.pids.push(pid.as_u32());
            g.cpu_pct += cpu / cores;
            g.max_core_pct = g.max_core_pct.max(cpu);
            g.mem += p.memory();
            g.disk_bps += (du.read_bytes + du.written_bytes) as f64 / dt;
            if let Some(pg) = extra.pid_gpu.get(&pid.as_u32()) {
                g.gpu_pct += pg.util;
                g.gpu_mem += pg.dedicated;
                let best = best_gpu.entry(g.name.clone()).or_insert(-1.0);
                if pg.util > *best {
                    *best = pg.util;
                    g.gpu_index = Some(pg.adapter);
                }
            }
        }
        let groups: Vec<ProcGroup> = groups
            .into_values()
            .map(|mut g| {
                g.gpu_pct = g.gpu_pct.min(100.0);
                g
            })
            .collect();

        let (mut rx, mut tx) = (0u64, 0u64);
        if let Some(nets) = self.nets.as_mut() {
            nets.refresh(true);
            for (name, data) in nets.iter() {
                if name != "lo" {
                    rx += data.received();
                    tx += data.transmitted();
                }
            }
        }

        Snapshot {
            cpu_pct: self.sys.global_cpu_usage(),
            core_pcts: self.sys.cpus().iter().map(|c| c.usage()).collect(),
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            disk_busy_pct: extra.disk_busy.as_ref().map(|d| d.0),
            disk_name: extra.disk_busy.map(|d| d.1),
            paging_per_sec: extra.paging_per_sec,
            disk_bps: groups.iter().map(|g| g.disk_bps).sum(),
            net_rx_bps: rx as f64 / dt,
            net_tx_bps: tx as f64 / dt,
            gpus: extra.gpus,
            groups,
        }
    }

    fn kill(&mut self, pids: &[u32]) -> usize {
        pids.iter()
            .filter(|&&pid| {
                !self
                    .sys
                    .process(Pid::from_u32(pid))
                    .map(|p| p.kill())
                    .unwrap_or(true)
            })
            .count()
    }
}

fn refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_disk_usage()
}
