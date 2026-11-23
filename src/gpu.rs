//! GPU measurement logic shared by all platforms.
//!
//! Windows reports GPU activity through performance counters whose *instance names*
//! encode which process and which GPU they belong to, for example:
//!
//! ```text
//! \GPU Engine(pid_1234_luid_0x00000000_0x0000C1A2_phys_0_eng_0_engtype_3D)\Utilization Percentage
//! \GPU Adapter Memory(luid_0x00000000_0x0000C1A2_phys_0)\Dedicated Usage
//! \GPU Process Memory(pid_1234_luid_0x00000000_0x0000C1A2_phys_0)\Dedicated Usage
//! ```
//!
//! The LUID identifies the GPU; DXGI tells us its name and memory size. This module
//! parses those names and adds the numbers up the same way Task Manager does:
//! a GPU's usage is its busiest engine, and so is a process's.

// Only the Windows build reads GPU counters; tests cover the logic everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::model::{Gpu, GpuKind};
use std::collections::HashMap;

pub type Luid = (u32, u32);

/// A GPU as reported by DXGI.
#[derive(Clone, Debug)]
pub struct AdapterDesc {
    pub luid: Luid,
    pub name: String,
    pub vendor_id: u32,
    pub dedicated_total: u64,
    pub shared_total: u64,
}

/// A process's use of one GPU.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PidGpu {
    pub adapter: usize,
    pub util: f32,
    pub dedicated: u64,
}

const VENDOR_INTEL: u32 = 0x8086;
const VENDOR_AMD: u32 = 0x1002;
const VENDOR_NVIDIA: u32 = 0x10DE;

/// Orders GPUs the way Task Manager numbers them (GPU 0, GPU 1, ...).
///
/// DXGI lists the "high performance" GPU first on laptops, but Task Manager numbers
/// GPUs in the order Windows registered them at startup, which is the order of their
/// LUIDs (adapter IDs handed out in sequence).
pub fn sort_like_task_manager(adapters: &mut [AdapterDesc]) {
    adapters.sort_by_key(|a| ((a.luid.0 as u64) << 32) | a.luid.1 as u64);
}

/// Brand for compact labels: "Intel(R) UHD Graphics" -> "Intel".
pub fn short_name(name: &str) -> String {
    let first = name.split_whitespace().next().unwrap_or("GPU");
    let brand = first.split('(').next().unwrap_or(first);
    if brand.is_empty() {
        "GPU".into()
    } else {
        brand.to_string()
    }
}

pub fn classify(vendor_id: u32, name: &str, dedicated_total: u64) -> GpuKind {
    let lower = name.to_ascii_lowercase();
    match vendor_id {
        VENDOR_NVIDIA => GpuKind::Discrete,
        // Intel's integrated chips report ~128 MB of dedicated memory; Arc cards report GBs.
        // (Names don't help: both can be called "Intel(R) Arc(TM) ... Graphics".)
        VENDOR_INTEL if dedicated_total >= 2 * 1024 * 1024 * 1024 => GpuKind::Discrete,
        VENDOR_INTEL => GpuKind::Integrated,
        // AMD integrated chips are named "AMD Radeon(TM) Graphics" or "Radeon 780M Graphics".
        VENDOR_AMD if lower.contains("rx ") || lower.contains("pro w") => GpuKind::Discrete,
        VENDOR_AMD if lower.contains("graphics") || lower.contains("vega") => GpuKind::Integrated,
        _ if dedicated_total >= 1024 * 1024 * 1024 => GpuKind::Discrete,
        _ if dedicated_total > 0 => GpuKind::Integrated,
        _ => GpuKind::Unknown,
    }
}

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?, 16).ok()
}

/// Reads `luid_0xHIGH_0xLOW` starting at `parts[i]`.
fn luid_at(parts: &[&str], i: usize) -> Option<Luid> {
    if *parts.get(i)? != "luid" {
        return None;
    }
    Some((hex(parts.get(i + 1)?)?, hex(parts.get(i + 2)?)?))
}

#[derive(Debug, PartialEq)]
pub struct EngineInstance {
    pub pid: u32,
    pub luid: Luid,
    pub phys: u32,
    pub eng: u32,
    pub engtype: String,
}

/// Parses `pid_1234_luid_0x0_0xC1A2_phys_0_eng_0_engtype_3D`.
pub fn parse_engine(name: &str) -> Option<EngineInstance> {
    let (head, engtype) = name.split_once("_engtype_")?;
    let parts: Vec<&str> = head.split('_').collect();
    if parts.first() != Some(&"pid")
        || parts.get(5) != Some(&"phys")
        || parts.get(7) != Some(&"eng")
    {
        return None;
    }
    Some(EngineInstance {
        pid: parts.get(1)?.parse().ok()?,
        luid: luid_at(&parts, 2)?,
        phys: parts.get(6)?.parse().ok()?,
        eng: parts.get(8)?.parse().ok()?,
        engtype: engtype.to_string(),
    })
}

/// Parses `luid_0x0_0xC1A2_phys_0` (GPU Adapter Memory).
pub fn parse_adapter(name: &str) -> Option<Luid> {
    let parts: Vec<&str> = name.split('_').collect();
    luid_at(&parts, 0)
}

/// Parses `pid_1234_luid_0x0_0xC1A2_phys_0` (GPU Process Memory).
pub fn parse_process(name: &str) -> Option<(u32, Luid)> {
    let parts: Vec<&str> = name.split('_').collect();
    if parts.first() != Some(&"pid") {
        return None;
    }
    Some((parts.get(1)?.parse().ok()?, luid_at(&parts, 2)?))
}

/// Raw counter readings: (instance name, value).
#[derive(Default)]
pub struct Readings {
    pub engine_util: Vec<(String, f64)>,
    pub adapter_dedicated: Vec<(String, f64)>,
    pub adapter_shared: Vec<(String, f64)>,
    pub process_dedicated: Vec<(String, f64)>,
}

/// Combines DXGI adapters and counter readings into per-GPU and per-process figures.
pub fn aggregate(adapters: &[AdapterDesc], r: &Readings) -> (Vec<Gpu>, HashMap<u32, PidGpu>) {
    let index: HashMap<Luid, usize> = adapters
        .iter()
        .enumerate()
        .map(|(i, a)| (a.luid, i))
        .collect();
    let mut gpus: Vec<Gpu> = adapters
        .iter()
        .map(|a| Gpu {
            name: a.name.clone(),
            kind: classify(a.vendor_id, &a.name, a.dedicated_total),
            dedicated_total: a.dedicated_total,
            shared_total: a.shared_total,
            ..Default::default()
        })
        .collect();

    // Sum every process's share of each engine; remember each process's busiest engine per GPU.
    let mut engines: HashMap<(usize, u32, u32), (f64, String)> = HashMap::new();
    let mut per_pid: HashMap<(u32, usize), f64> = HashMap::new();
    for (name, value) in &r.engine_util {
        let Some(e) = parse_engine(name) else {
            continue;
        };
        let Some(&i) = index.get(&e.luid) else {
            continue;
        };
        let entry = engines
            .entry((i, e.phys, e.eng))
            .or_insert((0.0, e.engtype.clone()));
        entry.0 += value;
        let p = per_pid.entry((e.pid, i)).or_insert(0.0);
        *p = p.max(*value);
    }
    for ((i, _, _), (util, engtype)) in engines {
        let util = util.clamp(0.0, 100.0) as f32;
        if util > gpus[i].util_pct || gpus[i].top_engine.is_empty() {
            gpus[i].util_pct = util;
            gpus[i].top_engine = engtype;
        }
    }

    for (name, value) in &r.adapter_dedicated {
        if let Some(&i) = parse_adapter(name).and_then(|l| index.get(&l)) {
            gpus[i].dedicated_used += *value as u64;
        }
    }
    for (name, value) in &r.adapter_shared {
        if let Some(&i) = parse_adapter(name).and_then(|l| index.get(&l)) {
            gpus[i].shared_used += *value as u64;
        }
    }

    let mut pids: HashMap<u32, PidGpu> = HashMap::new();
    for ((pid, i), util) in per_pid {
        let e = pids.entry(pid).or_insert(PidGpu {
            adapter: i,
            util: 0.0,
            dedicated: 0,
        });
        if util as f32 > e.util {
            e.util = (util as f32).min(100.0);
            e.adapter = i;
        }
    }
    for (name, value) in &r.process_dedicated {
        if let Some((pid, luid)) = parse_process(name)
            && let Some(&i) = index.get(&luid)
        {
            let e = pids.entry(pid).or_insert(PidGpu {
                adapter: i,
                util: 0.0,
                dedicated: 0,
            });
            e.dedicated += *value as u64;
        }
    }
    pids.retain(|_, p| p.util > 0.0 || p.dedicated > 0);
    (gpus, pids)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn laptop() -> Vec<AdapterDesc> {
        vec![
            AdapterDesc {
                luid: (0, 0xC1A2),
                name: "Intel(R) UHD Graphics".into(),
                vendor_id: VENDOR_INTEL,
                dedicated_total: 128 * 1024 * 1024,
                shared_total: 8 * GB,
            },
            AdapterDesc {
                luid: (0, 0xD3F0),
                name: "NVIDIA GeForce RTX 4060 Laptop GPU".into(),
                vendor_id: VENDOR_NVIDIA,
                dedicated_total: 8 * GB,
                shared_total: 8 * GB,
            },
        ]
    }

    #[test]
    fn parses_instance_names() {
        let e =
            parse_engine("pid_1234_luid_0x00000000_0x0000C1A2_phys_0_eng_3_engtype_VideoDecode")
                .unwrap();
        assert_eq!(
            e,
            EngineInstance {
                pid: 1234,
                luid: (0, 0xC1A2),
                phys: 0,
                eng: 3,
                engtype: "VideoDecode".into()
            }
        );
        assert_eq!(
            parse_engine("pid_8_luid_0x00000001_0x0000ABCD_phys_0_eng_0_engtype_")
                .unwrap()
                .engtype,
            ""
        );
        assert_eq!(
            parse_adapter("luid_0x00000000_0x0000D3F0_phys_0"),
            Some((0, 0xD3F0))
        );
        assert_eq!(
            parse_process("pid_42_luid_0x00000000_0x0000D3F0_phys_0"),
            Some((42, (0, 0xD3F0)))
        );
        assert_eq!(parse_engine("_Total"), None);
        assert_eq!(parse_adapter("garbage"), None);
    }

    #[test]
    fn classifies_gpus() {
        assert_eq!(
            classify(VENDOR_INTEL, "Intel(R) UHD Graphics 770", 128 << 20),
            GpuKind::Integrated
        );
        assert_eq!(
            classify(VENDOR_INTEL, "Intel(R) Iris(R) Xe Graphics", 128 << 20),
            GpuKind::Integrated
        );
        assert_eq!(
            classify(VENDOR_INTEL, "Intel(R) Arc(TM) Graphics", 128 << 20),
            GpuKind::Integrated
        );
        assert_eq!(
            classify(VENDOR_INTEL, "Intel(R) Arc(TM) A770 Graphics", 16 * GB),
            GpuKind::Discrete
        );
        assert_eq!(
            classify(VENDOR_AMD, "AMD Radeon 780M Graphics", 2 * GB),
            GpuKind::Integrated
        );
        assert_eq!(
            classify(VENDOR_NVIDIA, "NVIDIA GeForce RTX 4060 Laptop GPU", 8 * GB),
            GpuKind::Discrete
        );
        assert_eq!(
            classify(VENDOR_AMD, "AMD Radeon(TM) Graphics", 512 << 20),
            GpuKind::Integrated
        );
        assert_eq!(
            classify(VENDOR_AMD, "AMD Radeon RX 7600M XT", 8 * GB),
            GpuKind::Discrete
        );
    }

    #[test]
    fn numbers_gpus_like_task_manager() {
        // DXGI returned the NVIDIA card first; its LUID is higher, so it becomes GPU 1.
        let mut a = laptop();
        a.reverse();
        assert!(a[0].name.contains("NVIDIA"));
        sort_like_task_manager(&mut a);
        assert!(a[0].name.contains("Intel"));
        assert!(a[1].name.contains("NVIDIA"));
    }

    #[test]
    fn short_gpu_names() {
        assert_eq!(short_name("Intel(R) UHD Graphics"), "Intel");
        assert_eq!(short_name("NVIDIA GeForce RTX 4060 Laptop GPU"), "NVIDIA");
        assert_eq!(short_name("AMD Radeon(TM) Graphics"), "AMD");
        assert_eq!(short_name(""), "GPU");
    }

    #[test]
    fn aggregates_like_task_manager() {
        let r = Readings {
            engine_util: vec![
                // game on the integrated GPU: 3D engine 70% + 20% from the browser = 90%
                (
                    "pid_100_luid_0x00000000_0x0000C1A2_phys_0_eng_0_engtype_3D".into(),
                    70.0,
                ),
                (
                    "pid_100_luid_0x00000000_0x0000C1A2_phys_0_eng_1_engtype_Copy".into(),
                    10.0,
                ),
                (
                    "pid_200_luid_0x00000000_0x0000C1A2_phys_0_eng_0_engtype_3D".into(),
                    20.0,
                ),
                (
                    "pid_200_luid_0x00000000_0x0000C1A2_phys_0_eng_3_engtype_VideoDecode".into(),
                    35.0,
                ),
                // discrete GPU nearly idle
                (
                    "pid_300_luid_0x00000000_0x0000D3F0_phys_0_eng_0_engtype_3D".into(),
                    2.0,
                ),
                // unknown GPU (e.g. software renderer) is ignored
                (
                    "pid_300_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D".into(),
                    99.0,
                ),
            ],
            adapter_dedicated: vec![("luid_0x00000000_0x0000D3F0_phys_0".into(), (2 * GB) as f64)],
            adapter_shared: vec![("luid_0x00000000_0x0000C1A2_phys_0".into(), GB as f64)],
            process_dedicated: vec![(
                "pid_300_luid_0x00000000_0x0000D3F0_phys_0".into(),
                GB as f64,
            )],
        };
        let (gpus, pids) = aggregate(&laptop(), &r);
        assert_eq!(gpus[0].kind, GpuKind::Integrated);
        assert_eq!(gpus[0].util_pct, 90.0);
        assert_eq!(gpus[0].top_engine, "3D");
        assert_eq!(gpus[0].shared_used, GB);
        assert_eq!(gpus[1].kind, GpuKind::Discrete);
        assert_eq!(gpus[1].util_pct, 2.0);
        assert_eq!(gpus[1].dedicated_used, 2 * GB);
        assert_eq!(gpus[1].vram_pct(), Some(25.0));
        assert_eq!(gpus[0].vram_pct(), None);

        assert_eq!(
            pids[&100],
            PidGpu {
                adapter: 0,
                util: 70.0,
                dedicated: 0
            }
        );
        assert_eq!(pids[&200].util, 35.0); // busiest engine: video decode
        assert_eq!(
            pids[&300],
            PidGpu {
                adapter: 1,
                util: 2.0,
                dedicated: GB
            }
        );
    }
}
