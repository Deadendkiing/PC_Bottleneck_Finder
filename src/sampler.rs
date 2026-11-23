//! Collects one [`Snapshot`] of the whole system per call.

use crate::model::{ProcGroup, Snapshot};
use crate::platform::Counters;
use std::collections::HashMap;
use std::time::Instant;
use sysinfo::{Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

pub struct Sampler {
    sys: System,
    nets: Option<Networks>,
    counters: Counters,
    start: Instant,
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
            start: now,
            last: now,
        })
    }

    pub fn sample(&mut self) -> Snapshot {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64().max(0.001);
        self.last = now;

        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());

        let cores = self.sys.cpus().len().max(1) as f32;
        let mut groups: HashMap<String, ProcGroup> = HashMap::new();
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
        }
        let groups: Vec<ProcGroup> = groups.into_values().collect();

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

        let extra = self.counters.sample();
        Snapshot {
            t: now.duration_since(self.start).as_secs_f64(),
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
            groups,
        }
    }

    /// Ends every process in `pids`. Returns how many could not be ended.
    pub fn kill(&mut self, pids: &[u32]) -> usize {
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
