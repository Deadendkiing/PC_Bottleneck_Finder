# PC Bottleneck Finder

A small, fast Windows app that tells you **what is slowing your PC down, which program is causing it, and what to do about it**, in plain language.

> **Memory (RAM) is your bottleneck**
> 15.1 GB of 16 GB (94%) is in use, and Windows is moving data to the page file on disk, which is much slower than RAM.
>
> **Google Chrome** · 5.2 GB across 38 processes
> *What to do: Close tabs you aren't using. Closing Google Chrome completely would free about 5.2 GB.* `[Close]`

## Download

Get `pc-bottleneck-finder.exe` from the [Releases](../../releases) page, or from the latest [Build](../../actions/workflows/build.yml) run (under *Artifacts*).
It's a single file: no installer and no runtime needed. Just double-click it.

> Windows SmartScreen may warn about an unknown publisher because the app isn't code-signed. Click **More info → Run anyway**.

## What it detects

| Bottleneck | How it's detected | Example advice |
|---|---|---|
| **Memory** | RAM ≥ 90% used, or ≥ 85% while Windows is paging to disk | "Close tabs you aren't using…" |
| **CPU** | Total CPU ≥ 85% | "A tab or extension is busy. Press Shift+Esc in Microsoft Edge to see which one." |
| **Disk** | The busiest physical disk is ≥ 90% busy | "Windows is indexing your files for search. It usually finishes on its own." |
| **Single-threaded program** | One program keeps a single core at 100% while the CPU overall is mostly idle | "This is a limit of the program, not a problem with your PC." |

- A problem is only reported when it lasts for **most of the last 15 seconds**, so short spikes like opening an app are ignored.
- The app knows common Windows processes, such as Defender, Windows Update, the search indexer and Memory Compression, and explains them instead of blaming them.
- When memory pressure causes disk activity (paging), the disk finding says so.
- **It never offers to close Windows system processes.** Closing any other program always asks for confirmation first.

It also shows live CPU, memory, disk and network graphs, and a list of programs (all processes with the same name grouped together) that can be sorted by CPU, memory or disk.

## Why Rust

A monitoring tool shouldn't become a bottleneck itself. The app is written in Rust and compiles to a single ~5.6 MB native `.exe`:

- Measuring runs on a background thread once per second.
- The window only redraws when new data arrives or you interact with it.
- No runtime or garbage collector.

## How it works

| File | Role |
|---|---|
| `src/sampler.rs` | Reads CPU, memory, network and per-process usage every second ([`sysinfo`](https://crates.io/crates/sysinfo)) |
| `src/platform/windows.rs` | Reads Windows performance counters (the same data Task Manager uses): disk busy % per physical disk, and page-file reads |
| `src/diagnosis.rs` | The rules that turn measurements into findings, culprits and advice |
| `src/knowledge.rs` | Friendly names for common programs, and which processes must never be closed |
| `src/app.rs` | The window ([`egui`](https://github.com/emilk/egui)) |

## Building from source

Install [Rust](https://rustup.rs), then:

```bash
cargo test
cargo build --release
```

The app is in `target/release/pc-bottleneck-finder.exe`.

Every push is built and tested on Windows by GitHub Actions. Pushing a tag like `v0.1.0` also publishes a release with the `.exe` attached.

## Notes

- Some processes can only be closed when the app runs **as administrator** (right-click → *Run as administrator*).
- Network usage is shown for the whole PC. Per-program network usage isn't available yet.
- The rules are heuristics. They catch the common cases reliably (RAM full, disk at 100%, a CPU hog), but they're a well-informed guess, not a guarantee.

## Roadmap

- GPU usage per program
- Startup programs that slow down boot
- History log ("what was slow an hour ago?")
- App icon and code signing
