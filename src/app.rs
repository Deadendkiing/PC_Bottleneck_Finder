//! The window: diagnosis at the top, live graphs in the middle, processes below.

use crate::diagnosis::{self, Diagnosis, Finding, Severity};
use crate::knowledge;
use crate::model::{ProcGroup, Snapshot, fmt_bytes, fmt_rate};
use crate::sampler::Sampler;
use eframe::egui::{self, Color32, RichText, Stroke};
use egui_plot::{Line, Plot};
use std::collections::VecDeque;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_secs(1);
/// Seconds of history kept for the graphs.
const HISTORY: usize = 60;
const TABLE_ROWS: usize = 30;

const RED: Color32 = Color32::from_rgb(230, 80, 80);
const ORANGE: Color32 = Color32::from_rgb(240, 160, 50);
const BLUE: Color32 = Color32::from_rgb(90, 160, 240);
const GREEN: Color32 = Color32::from_rgb(80, 190, 120);
const PURPLE: Color32 = Color32::from_rgb(170, 120, 230);

#[derive(Default)]
struct Shared {
    history: VecDeque<Snapshot>,
    diagnosis: Diagnosis,
    message: Option<String>,
    error: Option<String>,
}

enum Command {
    Close { display: String, pids: Vec<u32> },
}

#[derive(Clone, Copy, PartialEq)]
enum SortBy {
    Cpu,
    Memory,
    Disk,
}

struct PendingClose {
    display: String,
    pids: Vec<u32>,
}

pub struct App {
    shared: Arc<Mutex<Shared>>,
    commands: Sender<Command>,
    sort: SortBy,
    pending: Option<PendingClose>,
    own_pid: u32,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = mpsc::channel();
        let own_pid = std::process::id();
        let ctx = cc.egui_ctx.clone();
        let thread_shared = shared.clone();

        // All measuring happens on this thread so the window never stutters.
        std::thread::spawn(move || {
            let mut sampler = match Sampler::new() {
                Ok(s) => s,
                Err(e) => {
                    thread_shared.lock().unwrap().error = Some(e);
                    ctx.request_repaint();
                    return;
                }
            };
            let mut next = Instant::now() + TICK;
            loop {
                // Wait for the next tick, but handle "Close" clicks immediately.
                match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Ok(Command::Close { display, pids }) => {
                        let failed = sampler.kill(&pids);
                        let msg = if failed == 0 {
                            format!("Closed {display}.")
                        } else {
                            format!(
                                "Couldn't close {failed} of {} {display} processes. Try running the app as administrator.",
                                pids.len()
                            )
                        };
                        thread_shared.lock().unwrap().message = Some(msg);
                        ctx.request_repaint();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                next = Instant::now() + TICK;
                let snap = sampler.sample();
                let mut s = thread_shared.lock().unwrap();
                s.history.push_back(snap);
                while s.history.len() > HISTORY {
                    s.history.pop_front();
                }
                let hist: Vec<Snapshot> = s.history.iter().cloned().collect();
                s.diagnosis = diagnosis::diagnose(&hist, own_pid);
                drop(s);
                ctx.request_repaint();
            }
        });

        App {
            shared,
            commands: tx,
            sort: SortBy::Cpu,
            pending: None,
            own_pid,
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let (latest, diag, series, message, error) = {
            let s = self.shared.lock().unwrap();
            (
                s.history.back().cloned(),
                s.diagnosis.clone(),
                Series::from(&s.history),
                s.message.clone(),
                s.error.clone(),
            )
        };

        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("PC Bottleneck Finder").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(own) = latest.as_ref().and_then(|s| own_usage(s, self.own_pid)) {
                        ui.weak(format!("This app: {own}"));
                    }
                });
            });
            ui.add_space(4.0);
        });

        if message.is_some()
            || error.is_some()
            || latest.as_ref().is_some_and(|s| s.disk_busy_pct.is_none())
        {
            egui::Panel::bottom("status").show(ui, |ui| {
                ui.add_space(4.0);
                if let Some(e) = &error {
                    ui.colored_label(RED, e);
                }
                if let Some(m) = &message {
                    ui.horizontal(|ui| {
                        ui.label(m);
                        if ui.small_button("Dismiss").clicked() {
                            self.shared.lock().unwrap().message = None;
                        }
                    });
                }
                if latest.as_ref().is_some_and(|s| s.disk_busy_pct.is_none()) {
                    ui.weak("Disk activity counters are unavailable, so disk bottlenecks can't be detected.");
                }
                ui.add_space(4.0);
            });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.verdict(ui, &diag);
                ui.add_space(10.0);
                if let Some(s) = &latest {
                    gauges(ui, s, &series);
                    ui.add_space(10.0);
                    self.process_table(ui, s);
                }
            });
        });

        self.confirm_dialog(ui);
    }
}

impl App {
    fn verdict(&mut self, ui: &mut egui::Ui, diag: &Diagnosis) {
        if !diag.ready {
            card(ui, BLUE, |ui| {
                // Static text on purpose: an animated spinner would redraw the window
                // every frame and make the app itself use CPU.
                ui.label(
                    RichText::new("Measuring your system... a verdict appears in a few seconds.")
                        .size(16.0),
                );
            });
            return;
        }
        if diag.findings.is_empty() {
            card(ui, GREEN, |ui| {
                ui.label(
                    RichText::new("No bottleneck right now")
                        .size(18.0)
                        .strong()
                        .color(GREEN),
                );
                ui.label("CPU, memory and disk are all keeping up.");
            });
            return;
        }
        for f in &diag.findings {
            self.finding(ui, f);
            ui.add_space(6.0);
        }
    }

    fn finding(&mut self, ui: &mut egui::Ui, f: &Finding) {
        let (color, tag) = match f.severity {
            Severity::Critical => (RED, "CRITICAL"),
            Severity::Warning => (ORANGE, "WARNING"),
            Severity::Info => (BLUE, "NOTE"),
        };
        card(ui, color, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tag).small().strong().color(color));
                ui.label(RichText::new(&f.headline).size(18.0).strong().color(color));
            });
            ui.label(&f.detail);
            if !f.culprits.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("Biggest users right now").strong());
                for c in &f.culprits {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&c.display).strong())
                            .on_hover_text(&c.name);
                        ui.label(format!("· {}", c.usage));
                        if c.can_close {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Close").clicked() {
                                        self.pending = Some(PendingClose {
                                            display: c.display.clone(),
                                            pids: c.pids.clone(),
                                        });
                                    }
                                },
                            );
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(12.0);
                        ui.label(RichText::new(format!("What to do: {}", c.advice)).italics());
                    });
                }
            }
            if let Some(tip) = &f.tip {
                ui.add_space(4.0);
                ui.weak(tip);
            }
        });
    }

    fn process_table(&mut self, ui: &mut egui::Ui, s: &Snapshot) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Programs").strong().size(16.0));
            ui.add_space(12.0);
            ui.label("Sort by");
            ui.selectable_value(&mut self.sort, SortBy::Cpu, "CPU");
            ui.selectable_value(&mut self.sort, SortBy::Memory, "Memory");
            ui.selectable_value(&mut self.sort, SortBy::Disk, "Disk");
        });
        let mut rows: Vec<&ProcGroup> = s.groups.iter().collect();
        match self.sort {
            SortBy::Cpu => rows.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct)),
            SortBy::Memory => rows.sort_by_key(|g| std::cmp::Reverse(g.mem)),
            SortBy::Disk => rows.sort_by(|a, b| b.disk_bps.total_cmp(&a.disk_bps)),
        }
        egui::Grid::new("procs")
            .striped(true)
            .num_columns(6)
            .min_col_width(90.0)
            .show(ui, |ui| {
                for h in ["Name", "Processes", "CPU", "Memory", "Disk", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for g in rows.into_iter().take(TABLE_ROWS) {
                    let display = knowledge::display_name(&g.name);
                    ui.label(&display).on_hover_text(&g.name);
                    ui.label(g.pids.len().to_string());
                    ui.label(format!("{:.1}%", g.cpu_pct));
                    ui.label(fmt_bytes(g.mem));
                    ui.label(fmt_rate(g.disk_bps));
                    if g.pids.contains(&self.own_pid) {
                        ui.weak("This app");
                    } else if knowledge::is_protected(&g.name, &g.pids, self.own_pid) {
                        ui.weak("Windows")
                            .on_hover_text("Part of Windows; closing it could crash your PC.");
                    } else if ui.small_button("Close").clicked() {
                        self.pending = Some(PendingClose {
                            display,
                            pids: g.pids.clone(),
                        });
                    }
                    ui.end_row();
                }
            });
    }

    fn confirm_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(p) = &self.pending else { return };
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("confirm-close")).show(ui.ctx(), |ui| {
            ui.set_width(360.0);
            ui.heading(format!("Close {}?", p.display));
            let n = p.pids.len();
            ui.label(if n > 1 {
                format!("This ends all {n} of its processes.")
            } else {
                "This ends the program.".to_string()
            });
            ui.label(RichText::new("Any unsaved work in it will be lost.").color(ORANGE));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Close it").clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
        match decision {
            Some(true) => {
                let p = self.pending.take().unwrap();
                let _ = self.commands.send(Command::Close {
                    display: p.display,
                    pids: p.pids,
                });
            }
            Some(false) => self.pending = None,
            None => {}
        }
    }
}

fn card(ui: &mut egui::Ui, color: Color32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(8.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// Graph data copied out of the shared history.
struct Series {
    cpu: Vec<[f64; 2]>,
    mem: Vec<[f64; 2]>,
    disk: Vec<[f64; 2]>,
    net: Vec<[f64; 2]>,
}

impl Series {
    fn from(h: &VecDeque<Snapshot>) -> Self {
        let pts = |f: &dyn Fn(&Snapshot) -> f64| h.iter().map(|s| [s.t, f(s)]).collect();
        Series {
            cpu: pts(&|s| s.cpu_pct as f64),
            mem: pts(&|s| s.mem_pct() as f64),
            disk: pts(&|s| s.disk_busy_pct.unwrap_or(0.0) as f64),
            net: pts(&|s| (s.net_rx_bps + s.net_tx_bps) / (1024.0 * 1024.0)),
        }
    }
}

fn gauges(ui: &mut egui::Ui, s: &Snapshot, series: &Series) {
    let busiest_core = s.core_pcts.iter().cloned().fold(0.0f32, f32::max);
    ui.columns(4, |cols| {
        gauge(
            &mut cols[0],
            "CPU",
            format!("{:.0}%", s.cpu_pct),
            format!("{} cores · busiest {:.0}%", s.core_pcts.len(), busiest_core),
            &series.cpu,
            BLUE,
            true,
        );
        gauge(
            &mut cols[1],
            "Memory",
            format!("{:.0}%", s.mem_pct()),
            format!("{} of {}", fmt_bytes(s.mem_used), fmt_bytes(s.mem_total)),
            &series.mem,
            PURPLE,
            true,
        );
        let disk_value = s
            .disk_busy_pct
            .map(|d| format!("{d:.0}%"))
            .unwrap_or_else(|| "n/a".into());
        let disk_sub = match &s.disk_name {
            Some(n) => format!("busiest: {n} · {}", fmt_rate(s.disk_bps)),
            None => fmt_rate(s.disk_bps),
        };
        gauge(
            &mut cols[2],
            "Disk busy",
            disk_value,
            disk_sub,
            &series.disk,
            GREEN,
            true,
        );
        gauge(
            &mut cols[3],
            "Network",
            fmt_rate(s.net_rx_bps + s.net_tx_bps),
            format!(
                "in {} · out {}",
                fmt_rate(s.net_rx_bps),
                fmt_rate(s.net_tx_bps)
            ),
            &series.net,
            ORANGE,
            false,
        );
    });
}

fn gauge(
    ui: &mut egui::Ui,
    title: &str,
    value: String,
    sub: String,
    pts: &[[f64; 2]],
    color: Color32,
    percent: bool,
) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(8.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.weak(title);
            ui.label(RichText::new(value).size(22.0).strong().color(color));
            ui.weak(sub);
            let mut plot = Plot::new(format!("plot-{title}"))
                .height(48.0)
                .show_axes(false)
                .show_grid(false)
                .show_background(false)
                .show_x(false)
                .show_y(false)
                .allow_drag(false)
                .allow_zoom(false)
                .allow_scroll(false)
                .allow_boxed_zoom(false)
                .allow_double_click_reset(false)
                .include_y(0.0);
            if percent {
                plot = plot.include_y(100.0);
            }
            plot.show(ui, |p| {
                p.line(
                    Line::new(title.to_string(), pts.to_vec())
                        .color(color)
                        .fill(0.0),
                );
            });
        });
}

fn own_usage(s: &Snapshot, own_pid: u32) -> Option<String> {
    let g = s.groups.iter().find(|g| g.pids.contains(&own_pid))?;
    Some(format!("{:.1}% CPU · {}", g.cpu_pct, fmt_bytes(g.mem)))
}
