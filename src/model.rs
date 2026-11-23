//! Plain data types shared by the sampler, the diagnosis engine and the UI.

/// All processes that share an executable name (e.g. every `chrome.exe`), added together.
#[derive(Clone, Debug, Default)]
pub struct ProcGroup {
    /// Executable name as reported by the OS, e.g. `chrome.exe`.
    pub name: String,
    pub pids: Vec<u32>,
    /// Share of the *whole* CPU (0–100), summed over the group.
    pub cpu_pct: f32,
    /// Highest usage of a single process in the group, in % of *one core*
    /// (100 = one core fully busy). Used to spot single-threaded programs.
    pub max_core_pct: f32,
    /// Resident memory in bytes, summed over the group.
    pub mem: u64,
    /// Disk read + write in bytes per second, summed over the group.
    pub disk_bps: f64,
    /// GPU usage (0–100) on the GPU this program uses most.
    pub gpu_pct: f32,
    /// Index into [`Snapshot::gpus`] of that GPU.
    pub gpu_index: Option<usize>,
    /// Dedicated video memory in bytes, summed over the group.
    pub gpu_mem: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuKind {
    /// Built into the processor (Intel UHD/Iris, AMD Radeon Graphics). Shares system RAM.
    Integrated,
    /// A separate graphics card with its own video memory (NVIDIA GeForce, AMD Radeon RX, Intel Arc).
    Discrete,
    #[default]
    Unknown,
}

impl GpuKind {
    pub fn label(self) -> &'static str {
        match self {
            GpuKind::Integrated => "Integrated",
            GpuKind::Discrete => "Discrete",
            GpuKind::Unknown => "GPU",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Gpu {
    pub name: String,
    pub kind: GpuKind,
    /// Usage of its busiest engine, 0–100 (same as Task Manager).
    pub util_pct: f32,
    /// Busiest engine, e.g. `3D`, `VideoDecode`, `Copy`.
    pub top_engine: String,
    pub dedicated_used: u64,
    pub dedicated_total: u64,
    pub shared_used: u64,
    pub shared_total: u64,
}

impl Gpu {
    /// Dedicated video memory use, 0–100. `None` for GPUs without their own memory.
    pub fn vram_pct(&self) -> Option<f32> {
        (self.dedicated_total >= 512 * 1024 * 1024)
            .then(|| (self.dedicated_used as f64 / self.dedicated_total as f64 * 100.0) as f32)
    }
}

/// One measurement of the whole system, taken about once per second.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Total CPU usage, 0–100.
    pub cpu_pct: f32,
    /// Usage of each logical core, 0–100.
    pub core_pcts: Vec<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    /// How busy the busiest physical disk is, 0–100. `None` if unavailable.
    pub disk_busy_pct: Option<f32>,
    /// Name of the busiest disk, e.g. `0 C:`.
    pub disk_name: Option<String>,
    /// Pages read from the page file per second (hard faults). `None` if unavailable.
    pub paging_per_sec: Option<f64>,
    pub disk_bps: f64,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub gpus: Vec<Gpu>,
    pub groups: Vec<ProcGroup>,
}

impl Snapshot {
    pub fn mem_pct(&self) -> f32 {
        if self.mem_total == 0 {
            0.0
        } else {
            (self.mem_used as f64 / self.mem_total as f64 * 100.0) as f32
        }
    }
}

pub fn fmt_bytes(b: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = b as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else {
        format!("{:.0} KB", b / 1024.0)
    }
}

pub fn fmt_rate(bps: f64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if bps >= MB {
        format!("{:.1} MB/s", bps / MB)
    } else {
        format!("{:.0} KB/s", bps / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes() {
        assert_eq!(
            fmt_bytes(5 * 1024 * 1024 * 1024 + 200 * 1024 * 1024),
            "5.2 GB"
        );
        assert_eq!(fmt_bytes(300 * 1024 * 1024), "300 MB");
        assert_eq!(fmt_rate(2.5 * 1024.0 * 1024.0), "2.5 MB/s");
        assert_eq!(fmt_rate(512.0 * 1024.0), "512 KB/s");
    }
}
