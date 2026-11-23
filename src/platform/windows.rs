//! Windows Performance Data Helper (PDH) counters — the same data Task Manager and
//! Resource Monitor use — plus DXGI to name the GPUs.
//!
//! * `\PhysicalDisk(*)\% Idle Time`            → busy % per physical disk (100 − idle)
//! * `\Memory\Pages Input/sec`                 → pages read back from the page file
//! * `\GPU Engine(*)\Utilization Percentage`   → GPU usage per process, GPU and engine
//! * `\GPU Adapter Memory(*)\Dedicated Usage`  → video memory used per GPU
//! * `\GPU Adapter Memory(*)\Shared Usage`     → system RAM used by each GPU
//! * `\GPU Process Memory(*)\Dedicated Usage`  → video memory used per process

use super::Extra;
use crate::gpu::{self, AdapterDesc, Readings};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};
use windows::Win32::System::Performance::{
    PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery,
    PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

const OK: u32 = 0;

#[derive(Default)]
struct Handles {
    disk: Option<PDH_HCOUNTER>,
    paging: Option<PDH_HCOUNTER>,
    gpu_engine: Option<PDH_HCOUNTER>,
    gpu_dedicated: Option<PDH_HCOUNTER>,
    gpu_shared: Option<PDH_HCOUNTER>,
    gpu_process: Option<PDH_HCOUNTER>,
}

pub struct Counters {
    query: Option<PDH_HQUERY>,
    h: Handles,
    adapters: Vec<AdapterDesc>,
}

// PDH handles are only used from the sampler thread that owns this value.
unsafe impl Send for Counters {}

impl Counters {
    pub fn new() -> Self {
        let adapters = list_adapters();
        let mut query = PDH_HQUERY::default();
        // SAFETY: plain FFI calls with valid out-pointers; failures are checked.
        unsafe {
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != OK {
                return Counters {
                    query: None,
                    h: Handles::default(),
                    adapters,
                };
            }
            let add = |path: PCWSTR| {
                let mut c = PDH_HCOUNTER::default();
                (PdhAddEnglishCounterW(query, path, 0, &mut c) == OK).then_some(c)
            };
            let has_gpu = !adapters.is_empty();
            let h = Handles {
                disk: add(w!("\\PhysicalDisk(*)\\% Idle Time")),
                paging: add(w!("\\Memory\\Pages Input/sec")),
                gpu_engine: has_gpu
                    .then(|| add(w!("\\GPU Engine(*)\\Utilization Percentage")))
                    .flatten(),
                gpu_dedicated: has_gpu
                    .then(|| add(w!("\\GPU Adapter Memory(*)\\Dedicated Usage")))
                    .flatten(),
                gpu_shared: has_gpu
                    .then(|| add(w!("\\GPU Adapter Memory(*)\\Shared Usage")))
                    .flatten(),
                gpu_process: has_gpu
                    .then(|| add(w!("\\GPU Process Memory(*)\\Dedicated Usage")))
                    .flatten(),
            };
            // Rate counters need a first sample to compare against.
            PdhCollectQueryData(query);
            Counters {
                query: Some(query),
                h,
                adapters,
            }
        }
    }

    pub fn sample(&mut self) -> Extra {
        let Some(query) = self.query else {
            return Extra::default();
        };
        // SAFETY: the query and counters were created by `new` and are still open.
        unsafe {
            if PdhCollectQueryData(query) != OK {
                return Extra::default();
            }
            let read = |c: Option<PDH_HCOUNTER>| c.map(|c| read_array(c)).unwrap_or_default();
            let disk_busy = read(self.h.disk)
                .into_iter()
                .filter(|(name, _)| name != "_Total")
                .map(|(name, idle)| ((100.0 - idle).clamp(0.0, 100.0) as f32, name))
                .max_by(|a, b| a.0.total_cmp(&b.0));
            let (gpus, pid_gpu) = if self.adapters.is_empty() {
                Default::default()
            } else {
                let readings = Readings {
                    engine_util: read(self.h.gpu_engine),
                    adapter_dedicated: read(self.h.gpu_dedicated),
                    adapter_shared: read(self.h.gpu_shared),
                    process_dedicated: read(self.h.gpu_process),
                };
                gpu::aggregate(&self.adapters, &readings)
            };
            Extra {
                disk_busy,
                paging_per_sec: self.h.paging.and_then(|c| single_value(c)),
                gpus,
                pid_gpu,
            }
        }
    }
}

impl Drop for Counters {
    fn drop(&mut self) {
        if let Some(q) = self.query.take() {
            // SAFETY: closing a query we opened.
            unsafe {
                PdhCloseQuery(q);
            }
        }
    }
}

/// Real GPUs as reported by DirectX, skipping the "Microsoft Basic Render Driver".
fn list_adapters() -> Vec<AdapterDesc> {
    let mut out = Vec::new();
    // SAFETY: COM calls on interfaces DXGI hands us; errors end the loop.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return out;
        };
        for i in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(i) else {
                break;
            };
            let Ok(d) = adapter.GetDesc1() else { continue };
            if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let len = d
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(d.Description.len());
            let luid = (d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart);
            if out.iter().any(|a: &AdapterDesc| a.luid == luid) {
                continue;
            }
            out.push(AdapterDesc {
                luid,
                name: String::from_utf16_lossy(&d.Description[..len])
                    .trim()
                    .to_string(),
                vendor_id: d.VendorId,
                dedicated_total: d.DedicatedVideoMemory as u64,
                shared_total: d.SharedSystemMemory as u64,
            });
        }
    }
    gpu::sort_like_task_manager(&mut out);
    out
}

unsafe fn single_value(counter: PDH_HCOUNTER) -> Option<f64> {
    let mut v = PDH_FMT_COUNTERVALUE::default();
    let status = unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, None, &mut v) };
    // The union is zero-initialised, so reading it is always defined.
    let value = unsafe { v.Anonymous.doubleValue };
    (status == OK && v.CStatus == PDH_CSTATUS_VALID_DATA).then_some(value)
}

/// Every instance of a wildcard counter, as (instance name, value).
unsafe fn read_array(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
    let mut size = 0u32;
    let mut count = 0u32;
    // First call asks how big the buffer must be.
    let status = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
    };
    if status != PDH_MORE_DATA || size == 0 {
        return Vec::new();
    }
    // The buffer holds the items followed by their name strings; u64 keeps it aligned.
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
    let status = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items))
    };
    if status != OK {
        return Vec::new();
    }
    let items = unsafe { std::slice::from_raw_parts(items, count as usize) };
    items
        .iter()
        .filter(|it| it.FmtValue.CStatus == PDH_CSTATUS_VALID_DATA)
        .filter_map(|it| {
            let name = unsafe { it.szName.to_string() }.ok()?;
            Some((name, unsafe { it.FmtValue.Anonymous.doubleValue }))
        })
        .collect()
}
