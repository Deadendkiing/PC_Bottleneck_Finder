# Roadmap

## v1.0: required before release

- [ ] **Game test on a dual-GPU laptop.** The game should show on the right GPU in the Programs list, and the GPU and integrated-graphics findings should appear when expected.
- [ ] **Code signing.** Sign the `.exe` so Windows SmartScreen stops showing "Unknown publisher". Pick a provider, then add a signing step to the GitHub Actions build so every release is signed automatically. Options:
  - SignPath Foundation: free for open-source projects, after an application and review.
  - Microsoft Trusted Signing (Azure): about $10/month, with a one-time identity check. Check whether it's available to individuals in your country.
  - A traditional certificate (Sectigo, DigiCert…): about $200–400/year, with a hardware key.
- [ ] **New logo.** The current three rising bars look like a network/signal icon. The new one should say "finding what's slowing the PC down", for example a magnifier over a chart, or a gauge with a warning mark. Update `assets/icon.*`, the sidebar logo in `src/ui/widgets.rs`, and the README.
- [ ] **History log.** Record every bottleneck the app detects: when it started and ended, what it was, how bad it got, and which programs were responsible. Show it on a new **History** page ("Yesterday 21:14–21:32: Memory 96%, Microsoft Edge 4.1 GB"), keep it between app restarts in a small file under `%LOCALAPPDATA%`, and allow exporting it (CSV). Also write a separate error log, so problems on other PCs can be diagnosed.
- [ ] **Temperatures** for CPU, GPU, RAM and SSD, shown on the resource cards, with a new finding when a part gets hot enough to slow itself down (thermal throttling). How hard each one is on Windows:
  - **GPU:** easy. Windows reports it (Task Manager shows it), and NVIDIA's driver library adds more detail.
  - **SSD:** moderate. NVMe drives report their temperature to Windows; it may need administrator rights.
  - **CPU:** hard. Windows has no reliable built-in way; tools like HWiNFO use a kernel driver. We need to find a safe approach, because the commonly used driver (WinRing0) is now blocked by Microsoft Defender as a security risk.
  - **RAM:** hardest. Only some memory sticks have a sensor (most DDR5, few laptop DDR4), and reading it also needs a kernel driver. Show it where possible, "not available" otherwise.
- [ ] Set the version to `1.0.0`, merge into `main`, tag `v1.0.0`, and write the release notes (with a screenshot and SmartScreen guidance).

## After v1.0

- [ ] Reduce the app's own memory use (about 150 MB on Windows, mostly the graphics driver).
- [ ] Startup programs that slow down boot.
- [ ] Per-program network usage.
