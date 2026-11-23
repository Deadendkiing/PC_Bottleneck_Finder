//! Windows Performance Data Helper (PDH) counters — the same data Task Manager and
//! Resource Monitor use.
//!
//! * `\PhysicalDisk(*)\% Idle Time` → busy % per physical disk (100 − idle).
//! * `\Memory\Pages Input/sec`      → pages read back from the page file per second.

use super::Extra;
use windows::Win32::System::Performance::{
    PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery,
    PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

const OK: u32 = 0;

pub struct Counters {
    query: Option<PDH_HQUERY>,
    disk: Option<PDH_HCOUNTER>,
    paging: Option<PDH_HCOUNTER>,
}

// PDH handles are only used from the sampler thread that owns this value.
unsafe impl Send for Counters {}

impl Counters {
    pub fn new() -> Self {
        let mut query = PDH_HQUERY::default();
        // SAFETY: plain FFI calls with valid out-pointers; failures are checked.
        unsafe {
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != OK {
                return Counters {
                    query: None,
                    disk: None,
                    paging: None,
                };
            }
            let add = |path: PCWSTR| {
                let mut c = PDH_HCOUNTER::default();
                (PdhAddEnglishCounterW(query, path, 0, &mut c) == OK).then_some(c)
            };
            let disk = add(w!("\\PhysicalDisk(*)\\% Idle Time"));
            let paging = add(w!("\\Memory\\Pages Input/sec"));
            // Rate counters need a first sample to compare against.
            PdhCollectQueryData(query);
            Counters {
                query: Some(query),
                disk,
                paging,
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
            Extra {
                disk_busy: self.disk.and_then(|c| busiest_disk(c)),
                paging_per_sec: self.paging.and_then(|c| single_value(c)),
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

unsafe fn single_value(counter: PDH_HCOUNTER) -> Option<f64> {
    let mut v = PDH_FMT_COUNTERVALUE::default();
    let status = unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, None, &mut v) };
    // The union is zero-initialised, so reading it is always defined.
    let value = unsafe { v.Anonymous.doubleValue };
    (status == OK && v.CStatus == PDH_CSTATUS_VALID_DATA).then_some(value)
}

/// Returns (busy %, disk name) for the busiest physical disk, skipping `_Total`.
unsafe fn busiest_disk(counter: PDH_HCOUNTER) -> Option<(f32, String)> {
    let mut size = 0u32;
    let mut count = 0u32;
    // First call asks how big the buffer must be.
    let status = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
    };
    if status != PDH_MORE_DATA || size == 0 {
        return None;
    }
    // The buffer holds the items followed by their name strings; u64 keeps it aligned.
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
    let status = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items))
    };
    if status != OK {
        return None;
    }
    let items = unsafe { std::slice::from_raw_parts(items, count as usize) };
    items
        .iter()
        .filter(|it| it.FmtValue.CStatus == PDH_CSTATUS_VALID_DATA)
        .filter_map(|it| {
            let name = unsafe { it.szName.to_string() }.ok()?;
            if name == "_Total" {
                return None;
            }
            let idle = unsafe { it.FmtValue.Anonymous.doubleValue };
            Some(((100.0 - idle).clamp(0.0, 100.0) as f32, name))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
}
