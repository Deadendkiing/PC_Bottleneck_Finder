# Roadmap

## v1.0: required before release

- [ ] **Game test on a dual-GPU laptop.** The game should show on the right GPU in the Programs list, and the GPU and integrated-graphics findings should appear when expected.
- [ ] **Code signing.** Sign the `.exe` so Windows SmartScreen stops showing "Unknown publisher". Pick a provider, then add a signing step to the GitHub Actions build so every release is signed automatically. Options:
  - SignPath Foundation: free for open-source projects, after an application and review.
  - Microsoft Trusted Signing (Azure): about $10/month, with a one-time identity check. Check whether it's available to individuals in your country.
  - A traditional certificate (Sectigo, DigiCert…): about $200–400/year, with a hardware key.
- [ ] **New logo.** The current three rising bars look like a network/signal icon. The new one should say "finding what's slowing the PC down", for example a magnifier over a chart, or a gauge with a warning mark. Update `assets/icon.*`, the sidebar logo in `src/ui/widgets.rs`, and the README.
- [ ] Set the version to `1.0.0`, merge into `main`, tag `v1.0.0`, and write the release notes (with a screenshot and SmartScreen guidance).

## After v1.0

- [ ] Reduce the app's own memory use (about 150 MB on Windows, mostly the graphics driver).
- [ ] Startup programs that slow down boot.
- [ ] History log ("what was slow an hour ago?").
- [ ] Temperatures and thermal throttling.
- [ ] Per-program network usage.
