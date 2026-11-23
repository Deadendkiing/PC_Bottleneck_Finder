//! Linux equivalents, mainly so the app can be developed and tested off Windows.
//!
//! * `/proc/diskstats` field 13 (ms spent doing I/O) → busy % per disk.
//! * `/proc/vmstat` `pswpin` → pages swapped in per second.

use super::Extra;
use std::collections::HashMap;
use std::fs;
use std::time::Instant;

pub struct Counters {
    last: Instant,
    io_ms: HashMap<String, u64>,
    swap_in: Option<u64>,
}

impl Counters {
    pub fn new() -> Self {
        Counters {
            last: Instant::now(),
            io_ms: read_io_ms(),
            swap_in: read_swap_in(),
        }
    }

    pub fn sample(&mut self) -> Extra {
        let now = Instant::now();
        let ms = now.duration_since(self.last).as_secs_f64() * 1000.0;
        self.last = now;

        let io = read_io_ms();
        let disk_busy = io
            .iter()
            .filter_map(|(name, &v)| {
                let prev = *self.io_ms.get(name)?;
                let busy = (v.saturating_sub(prev) as f64 / ms * 100.0).clamp(0.0, 100.0) as f32;
                Some((busy, name.clone()))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0));
        self.io_ms = io;

        let swap = read_swap_in();
        let paging_per_sec = match (self.swap_in, swap) {
            (Some(a), Some(b)) => Some(b.saturating_sub(a) as f64 / (ms / 1000.0)),
            _ => None,
        };
        self.swap_in = swap;

        Extra {
            disk_busy,
            paging_per_sec,
        }
    }
}

/// Milliseconds spent doing I/O for each whole disk (partitions and virtual devices skipped).
fn read_io_ms() -> HashMap<String, u64> {
    let Ok(text) = fs::read_to_string("/proc/diskstats") else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let name = *f.get(2)?;
            let virtual_dev = ["loop", "ram", "zram", "fd", "sr"]
                .iter()
                .any(|p| name.starts_with(p));
            let whole_disk = fs::metadata(format!("/sys/block/{name}")).is_ok();
            if virtual_dev || !whole_disk {
                return None;
            }
            Some((name.to_string(), f.get(12)?.parse().ok()?))
        })
        .collect()
}

fn read_swap_in() -> Option<u64> {
    fs::read_to_string("/proc/vmstat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("pswpin ")?.trim().parse().ok())
}
