//! OS-specific measurements that `sysinfo` doesn't provide:
//! how busy each disk is, and how much Windows is paging memory to disk.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::Counters;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use self::linux::Counters;

#[cfg(not(any(windows, target_os = "linux")))]
pub struct Counters;
#[cfg(not(any(windows, target_os = "linux")))]
impl Counters {
    pub fn new() -> Self {
        Counters
    }
    pub fn sample(&mut self) -> Extra {
        Extra::default()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Extra {
    /// Busy % of the busiest disk and its name.
    pub disk_busy: Option<(f32, String)>,
    /// Pages read from the page file / swap per second.
    pub paging_per_sec: Option<f64>,
}
