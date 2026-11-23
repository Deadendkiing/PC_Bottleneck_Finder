//! The window: a sidebar, an overview (diagnosis + live usage), a programs list and an
//! about page. All drawing is custom so the app looks the same everywhere.

mod theme;
mod widgets;

use crate::diagnosis::{self, Diagnosis, Finding, Resource, Severity};
use crate::knowledge;
use crate::model::{GpuKind, ProcGroup, Snapshot, fmt_bytes, fmt_rate};
use crate::sampler::Source;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, Frame, Layout, Margin, Painter, Rect,
    RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2,
};
use std::collections::VecDeque;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use theme::*;
use widgets::*;

const TICK: Duration = Duration::from_secs(1);
/// Seconds of history kept for the graphs.
const HISTORY: usize = 60;
const TOAST_FOR: Duration = Duration::from_secs(6);

#[derive(Default)]
struct Shared {
    history: VecDeque<Snapshot>,
    diagnosis: Diagnosis,
    toast: Option<(String, bool, Instant)>,
    error: Option<String>,
}

enum Command {
    Close { display: String, pids: Vec<u32> },
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Overview,
    Programs,
    About,
}

#[derive(Clone, Copy, PartialEq)]
enum SortKey {
    Name,
    Count,
    Cpu,
    Memory,
    Disk,
    Gpu,
}

struct PendingClose {
    display: String,
    pids: Vec<u32>,
}

pub struct App {
    shared: Arc<Mutex<Shared>>,
    commands: Sender<Command>,
    page: Page,
    sort: SortKey,
    search: String,
    pending: Option<PendingClose>,
    own_pid: u32,
    demo: bool,
}

/// What the UI needs from the shared state for one frame.
struct View {
    latest: Option<Snapshot>,
    diag: Diagnosis,
    series: Series,
    toast: Option<(String, bool, Instant)>,
    error: Option<String>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        source: Result<Box<dyn Source>, String>,
        demo: bool,
    ) -> Self {
        theme::apply(&cc.egui_ctx);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = mpsc::channel();
        let own_pid = std::process::id();
        let ctx = cc.egui_ctx.clone();
        let thread_shared = shared.clone();

        // All measuring happens on this thread so the window never stutters.
        std::thread::spawn(move || {
            let mut source = match source {
                Ok(s) => s,
                Err(e) => {
                    thread_shared.lock().unwrap().error = Some(e);
                    ctx.request_repaint();
                    return;
                }
            };
            let mut next = Instant::now() + TICK;
            loop {
                // Wait for the next tick, but handle "End task" clicks immediately.
                match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Ok(Command::Close { display, pids }) => {
                        let failed = source.kill(&pids);
                        let toast = if failed == 0 {
                            (format!("{display} was closed."), true)
                        } else {
                            (
                                format!(
                                    "Couldn't close {failed} of {} {display} processes. Try running as administrator.",
                                    pids.len()
                                ),
                                false,
                            )
                        };
                        thread_shared.lock().unwrap().toast =
                            Some((toast.0, toast.1, Instant::now()));
                        ctx.request_repaint();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                next = Instant::now() + TICK;
                let snap = source.sample();
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
            page: Page::Overview,
            sort: SortKey::Cpu,
            search: String::new(),
            pending: None,
            own_pid,
            demo,
        }
    }

    fn view(&self) -> View {
        let s = self.shared.lock().unwrap();
        View {
            latest: s.history.back().cloned(),
            diag: s.diagnosis.clone(),
            series: Series::from(&s.history),
            toast: s.toast.clone(),
            error: s.error.clone(),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let v = self.view();

        egui::Panel::left("nav")
            .exact_size(216.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(SIDEBAR).inner_margin(Margin {
                left: 14,
                right: 14,
                top: 18,
                bottom: 14,
            }))
            .show(ui, |ui| self.sidebar(ui, &v));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin {
                left: 28,
                right: 28,
                top: 22,
                bottom: 0,
            }))
            .show(ui, |ui| {
                self.header(ui, &v);
                ui.add_space(14.0);
                ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    if let Some(e) = &v.error {
                        card(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(e).color(RED));
                        });
                    }
                    match self.page {
                        Page::Overview => self.overview(ui, &v),
                        Page::Programs => self.programs(ui, &v),
                        Page::About => self.about(ui),
                    }
                    ui.add_space(24.0);
                });
            });

        self.confirm_dialog(ui);
        self.toast(ui, &v);
    }
}

// ---- Chrome -------------------------------------------------------------------------

impl App {
    fn sidebar(&mut self, ui: &mut Ui, v: &View) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
            logo(ui.painter(), r);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(
                    RichText::new("Bottleneck")
                        .font(bold_font(15.0))
                        .color(TEXT),
                );
                ui.label(RichText::new("Finder").size(12.5).color(TEXT_2));
            });
        });
        ui.add_space(26.0);
        self.nav_item(ui, Page::Overview, "Overview", icon_overview);
        self.nav_item(ui, Page::Programs, "Programs", icon_programs);
        self.nav_item(ui, Page::About, "About", icon_about);

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.label(
                RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                    .size(11.5)
                    .color(TEXT_3),
            );
            if let Some(own) = v
                .latest
                .as_ref()
                .and_then(|s| s.groups.iter().find(|g| g.pids.contains(&self.own_pid)))
            {
                ui.label(
                    RichText::new(format!(
                        "This app: {:.1}% CPU · {}",
                        own.cpu_pct,
                        fmt_bytes(own.mem)
                    ))
                    .size(11.5)
                    .color(TEXT_3),
                );
            }
            ui.horizontal(|ui| {
                dot(ui, GREEN);
                ui.label(
                    RichText::new("Live · every second")
                        .size(12.0)
                        .color(TEXT_2),
                );
            });
            if self.demo {
                ui.add_space(4.0);
                pill(ui, "DEMO DATA", AMBER);
            }
        });
    }

    fn nav_item(
        &mut self,
        ui: &mut Ui,
        page: Page,
        label: &str,
        icon: fn(&Painter, Rect, Color32),
    ) {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
        let selected = self.page == page;
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, 8.0, RAISED);
            p.rect_filled(
                Rect::from_min_size(rect.left_top() + vec2(0.0, 10.0), vec2(3.0, 18.0)),
                2.0,
                ACCENT,
            );
        } else if resp.hovered() {
            p.rect_filled(rect, 8.0, HOVER.gamma_multiply(0.6));
        }
        let color = if selected { TEXT } else { TEXT_2 };
        icon(
            p,
            Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(16.0, 16.0)),
            if selected { ACCENT } else { TEXT_2 },
        );
        let font = if selected {
            bold_font(14.0)
        } else {
            theme::font(14.0)
        };
        p.text(
            pos2(rect.left() + 42.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            color,
        );
        if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            self.page = page;
        }
        ui.add_space(2.0);
    }

    fn header(&mut self, ui: &mut Ui, v: &View) {
        let (title, subtitle) = match self.page {
            Page::Overview => (
                "Overview",
                "What's slowing your PC down right now, and how to fix it",
            ),
            Page::Programs => ("Programs", "Everything running, grouped by program"),
            Page::About => ("About", "How PC Bottleneck Finder works"),
        };
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new(title).font(bold_font(24.0)).color(TEXT));
                ui.label(RichText::new(subtitle).color(TEXT_2));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (text, color) = status(&v.diag);
                Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(99.0)
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            dot(ui, color);
                            ui.label(RichText::new(text).font(bold_font(13.0)).color(TEXT));
                        });
                    });
            });
        });
    }

    fn toast(&mut self, ui: &mut Ui, v: &View) {
        let Some((msg, ok, at)) = &v.toast else {
            return;
        };
        let age = at.elapsed();
        if age > TOAST_FOR {
            self.shared.lock().unwrap().toast = None;
            return;
        }
        ui.ctx().request_repaint_after(TOAST_FOR - age);
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(Align2::CENTER_BOTTOM, vec2(108.0, -24.0))
            .show(ui.ctx(), |ui| {
                Frame::new()
                    .fill(RAISED)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(10.0)
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            dot(ui, if *ok { GREEN } else { AMBER });
                            ui.label(RichText::new(msg).color(TEXT));
                        });
                    });
            });
    }

    fn confirm_dialog(&mut self, ui: &mut Ui) {
        let Some(p) = &self.pending else { return };
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::unique("confirm-close"))
            .frame(Frame::new().fill(SURFACE).stroke(Stroke::new(1.0, BORDER)).corner_radius(14.0).inner_margin(22.0))
            .show(ui.ctx(), |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(format!("End {}?", p.display)).font(bold_font(18.0)).color(TEXT));
                ui.add_space(4.0);
                let n = p.pids.len();
                ui.label(RichText::new(if n > 1 {
                    format!("This closes all {n} of its processes. Any unsaved work in it will be lost.")
                } else {
                    "This closes the program. Any unsaved work in it will be lost.".to_string()
                })
                .color(TEXT_2));
                ui.add_space(14.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if danger_button(ui, "End task").clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decision = Some(false);
                    }
                });
            });
        // Escape or a click outside the dialog cancels.
        if decision.is_none() && modal.should_close() {
            decision = Some(false);
        }
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

fn status(d: &Diagnosis) -> (String, Color32) {
    if !d.ready {
        return ("Measuring".into(), ACCENT);
    }
    let worst = d.findings.iter().map(|f| f.severity).max();
    let n = d.findings.len();
    match worst {
        None => ("Healthy".into(), GREEN),
        Some(sev) => (
            format!("{n} {}", if n == 1 { "issue" } else { "issues" }),
            severity_color(sev),
        ),
    }
}

fn severity_color(s: Severity) -> Color32 {
    match s {
        Severity::Critical => RED,
        Severity::Warning => AMBER,
        Severity::Info => ACCENT,
    }
}

// ---- Overview ------------------------------------------------------------------------

impl App {
    fn overview(&mut self, ui: &mut Ui, v: &View) {
        section_title(ui, "Diagnosis");
        if !v.diag.ready {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    dot(ui, ACCENT);
                    ui.label(
                        RichText::new("Measuring your system")
                            .font(bold_font(16.0))
                            .color(TEXT),
                    );
                });
                ui.label(
                    RichText::new("A verdict appears after a few seconds of measurements.")
                        .color(TEXT_2),
                );
            });
        } else if v.diag.findings.is_empty() {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                    check_badge(ui.painter(), r);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("No bottlenecks detected")
                                .font(bold_font(17.0))
                                .color(TEXT),
                        );
                        ui.label(
                            RichText::new("CPU, memory, disk and graphics are all keeping up.")
                                .color(TEXT_2),
                        );
                    });
                });
            });
        } else {
            for f in &v.diag.findings {
                self.finding_card(ui, f);
                ui.add_space(10.0);
            }
        }

        if let Some(s) = &v.latest {
            ui.add_space(14.0);
            section_title(ui, "Live usage");
            resource_grid(ui, s, &v.series);
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                section_title(ui, "Top programs");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Show all programs").clicked() {
                        self.page = Page::Programs;
                    }
                });
            });
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                self.program_table(ui, s, Some(6), "");
            });
        }
    }

    fn finding_card(&mut self, ui: &mut Ui, f: &Finding) {
        let color = severity_color(f.severity);
        let tag = match f.severity {
            Severity::Critical => "CRITICAL",
            Severity::Warning => "WARNING",
            Severity::Info => "NOTE",
        };
        let resp = Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(12.0)
            .inner_margin(Margin {
                left: 22,
                right: 18,
                top: 16,
                bottom: 16,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    pill(ui, tag, color);
                    ui.label(
                        RichText::new(resource_label(f.resource))
                            .size(12.0)
                            .color(TEXT_3),
                    );
                });
                ui.add_space(6.0);
                ui.label(RichText::new(&f.headline).font(bold_font(18.0)).color(TEXT));
                ui.add_space(2.0);
                ui.label(RichText::new(&f.detail).color(TEXT_2));
                if !f.culprits.is_empty() {
                    ui.add_space(10.0);
                    for c in &f.culprits {
                        Frame::new()
                            .fill(RAISED)
                            .corner_radius(10.0)
                            .inner_margin(12.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let (r, _) =
                                        ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
                                    avatar(ui.painter(), r.center(), 17.0, &c.display);
                                    ui.add_space(4.0);
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 1.0;
                                        ui.label(
                                            RichText::new(&c.display)
                                                .font(bold_font(14.5))
                                                .color(TEXT),
                                        )
                                        .on_hover_text(&c.name);
                                        ui.label(RichText::new(&c.usage).size(13.0).color(TEXT_2));
                                    });
                                    if c.can_close {
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if danger_button(ui, "End task").clicked() {
                                                    self.pending = Some(PendingClose {
                                                        display: c.display.clone(),
                                                        pids: c.pids.clone(),
                                                    });
                                                }
                                            },
                                        );
                                    }
                                });
                                ui.add_space(6.0);
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new("What to do")
                                            .font(bold_font(13.0))
                                            .color(color),
                                    );
                                    ui.label(RichText::new(&c.advice).color(TEXT));
                                });
                            });
                        ui.add_space(6.0);
                    }
                }
                if let Some(tip) = &f.tip {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Tip").font(bold_font(13.0)).color(ACCENT));
                        ui.label(RichText::new(tip).color(TEXT_2));
                    });
                }
            });
        let r = resp.response.rect;
        ui.painter().rect_filled(
            Rect::from_min_size(r.min, vec2(4.0, r.height())),
            CornerRadius {
                nw: 12,
                sw: 12,
                ne: 0,
                se: 0,
            },
            color,
        );
    }
}

fn resource_label(r: Resource) -> &'static str {
    match r {
        Resource::Memory => "Memory",
        Resource::Cpu => "Processor",
        Resource::Disk => "Storage",
        Resource::SingleCore => "Processor",
        Resource::Gpu => "Graphics",
        Resource::VideoMemory => "Graphics memory",
        Resource::WrongGpu => "Graphics",
    }
}

/// Graph data copied out of the shared history.
#[derive(Default)]
struct Series {
    cpu: Vec<f32>,
    mem: Vec<f32>,
    disk: Vec<f32>,
    net: Vec<f32>,
    gpus: Vec<Vec<f32>>,
}

impl Series {
    fn from(h: &VecDeque<Snapshot>) -> Self {
        let gpu_count = h.back().map_or(0, |s| s.gpus.len());
        Series {
            cpu: h.iter().map(|s| s.cpu_pct).collect(),
            mem: h.iter().map(|s| s.mem_pct()).collect(),
            disk: h.iter().map(|s| s.disk_busy_pct.unwrap_or(0.0)).collect(),
            net: h
                .iter()
                .map(|s| ((s.net_rx_bps + s.net_tx_bps) / 1e6) as f32)
                .collect(),
            gpus: (0..gpu_count)
                .map(|i| {
                    h.iter()
                        .map(|s| s.gpus.get(i).map_or(0.0, |g| g.util_pct))
                        .collect()
                })
                .collect(),
        }
    }
}

struct ResourceCard<'a> {
    title: String,
    badge: Option<(String, Color32)>,
    value: String,
    sub: String,
    series: &'a [f32],
    max: f32,
    color: Color32,
    meter: Option<(String, f32)>,
}

fn resource_grid(ui: &mut Ui, s: &Snapshot, series: &Series) {
    let busiest_core = s.core_pcts.iter().cloned().fold(0.0f32, f32::max);
    let mut cards = vec![
        ResourceCard {
            title: "CPU".into(),
            badge: Some((format!("{} threads", s.core_pcts.len()), TEXT_2)),
            value: format!("{:.0}%", s.cpu_pct),
            sub: format!("Busiest core {busiest_core:.0}%"),
            series: &series.cpu,
            max: 100.0,
            color: CPU,
            meter: None,
        },
        ResourceCard {
            title: "Memory".into(),
            badge: None,
            value: format!("{:.0}%", s.mem_pct()),
            sub: format!("{} of {}", fmt_bytes(s.mem_used), fmt_bytes(s.mem_total)),
            series: &series.mem,
            max: 100.0,
            color: MEMORY,
            meter: None,
        },
        ResourceCard {
            title: "Disk".into(),
            badge: s.disk_name.clone().map(|n| (n, TEXT_2)),
            value: s.disk_busy_pct.map_or("n/a".into(), |d| format!("{d:.0}%")),
            sub: format!("{} read + write", fmt_rate(s.disk_bps)),
            series: &series.disk,
            max: 100.0,
            color: DISK,
            meter: None,
        },
        ResourceCard {
            title: "Network".into(),
            badge: None,
            value: fmt_rate(s.net_rx_bps + s.net_tx_bps),
            sub: format!(
                "Down {} · Up {}",
                fmt_rate(s.net_rx_bps),
                fmt_rate(s.net_tx_bps)
            ),
            series: &series.net,
            max: 0.1,
            color: NETWORK,
            meter: None,
        },
    ];
    for (i, g) in s.gpus.iter().enumerate() {
        let color = GPU[i % GPU.len()];
        let kind_color = match g.kind {
            GpuKind::Discrete => color,
            _ => TEXT_2,
        };
        let meter = match g.vram_pct() {
            Some(p) => Some((
                format!(
                    "Video memory {} / {}",
                    fmt_bytes(g.dedicated_used),
                    fmt_bytes(g.dedicated_total)
                ),
                p / 100.0,
            )),
            None => Some((
                format!("Shared memory {}", fmt_bytes(g.shared_used)),
                if g.shared_total > 0 {
                    g.shared_used as f32 / g.shared_total as f32
                } else {
                    0.0
                },
            )),
        };
        cards.push(ResourceCard {
            title: format!("GPU {i}"),
            badge: Some((g.kind.label().into(), kind_color)),
            value: format!("{:.0}%", g.util_pct),
            sub: g.name.clone(),
            series: series.gpus.get(i).map_or(&[], |v| v.as_slice()),
            max: 100.0,
            color,
            meter,
        });
    }

    let gap = 12.0;
    let avail = ui.available_width();
    let cols = (((avail + gap) / (232.0 + gap)).floor() as usize).clamp(1, cards.len());
    // Balance the rows: 4 cards become 2 + 2 rather than 3 + 1.
    let cols = cards.len().div_ceil(cards.len().div_ceil(cols));
    let w = (avail - gap * (cols - 1) as f32) / cols as f32;
    let has_meter = cards.iter().any(|c| c.meter.is_some());
    for row in cards.chunks(cols) {
        // Top-aligned so cards line up even while their heights are being measured.
        ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for c in row {
                ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(w);
                    resource_card(ui, c, has_meter);
                });
            }
        });
        ui.add_space(gap);
    }
}

fn resource_card(ui: &mut Ui, c: &ResourceCard, reserve_meter: bool) {
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        // Same height for every card so rows line up.
        ui.set_min_height(if reserve_meter { 192.0 } else { 146.0 });
        ui.horizontal(|ui| {
            ui.label(RichText::new(&c.title).font(bold_font(13.0)).color(TEXT_2));
            if let Some((b, color)) = &c.badge {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    pill(ui, b, *color);
                });
            }
        });
        ui.label(RichText::new(&c.value).font(bold_font(28.0)).color(c.color));
        ui.add(egui::Label::new(RichText::new(&c.sub).size(12.5).color(TEXT_2)).truncate());
        ui.add_space(6.0);
        sparkline(ui, 46.0, c.series, c.max, c.color);
        if let Some((label, frac)) = &c.meter {
            ui.add_space(8.0);
            ui.add(egui::Label::new(RichText::new(label).size(12.0).color(TEXT_2)).truncate());
            meter(ui, *frac, c.color);
        }
    });
}

// ---- Programs ------------------------------------------------------------------------

impl App {
    fn programs(&mut self, ui: &mut Ui, v: &View) {
        let Some(s) = &v.latest else { return };
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search programs")
                    .desired_width(280.0)
                    .margin(Margin::symmetric(10, 6)),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let procs: usize = s.groups.iter().map(|g| g.pids.len()).sum();
                ui.label(
                    RichText::new(format!("{} programs · {} processes", s.groups.len(), procs))
                        .color(TEXT_2),
                );
            });
        });
        ui.add_space(10.0);
        let search = self.search.clone();
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            self.program_table(ui, s, None, &search);
        });
    }

    fn program_table(&mut self, ui: &mut Ui, s: &Snapshot, limit: Option<usize>, filter: &str) {
        let has_gpu = !s.gpus.is_empty();
        let mut cols: Vec<(Option<SortKey>, &str, f32)> = vec![
            (Some(SortKey::Name), "Name", 0.0),
            (Some(SortKey::Count), "Processes", 90.0),
            (Some(SortKey::Cpu), "CPU", 84.0),
            (Some(SortKey::Memory), "Memory", 100.0),
            (Some(SortKey::Disk), "Disk", 100.0),
        ];
        if has_gpu {
            cols.push((Some(SortKey::Gpu), "GPU", 112.0));
        }
        cols.push((None, "", 104.0));
        let width = ui.available_width();
        let fixed: f32 = cols.iter().map(|c| c.2).sum();
        cols[0].2 = (width - fixed).max(140.0);

        let filter = filter.to_lowercase();
        let mut rows: Vec<&ProcGroup> = s
            .groups
            .iter()
            .filter(|g| {
                filter.is_empty()
                    || g.name.to_lowercase().contains(&filter)
                    || knowledge::display_name(&g.name)
                        .to_lowercase()
                        .contains(&filter)
            })
            .collect();
        match self.sort {
            SortKey::Name => rows.sort_by_key(|g| knowledge::display_name(&g.name).to_lowercase()),
            SortKey::Count => rows.sort_by_key(|g| std::cmp::Reverse(g.pids.len())),
            SortKey::Cpu => rows.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct)),
            SortKey::Memory => rows.sort_by_key(|g| std::cmp::Reverse(g.mem)),
            SortKey::Disk => rows.sort_by(|a, b| b.disk_bps.total_cmp(&a.disk_bps)),
            SortKey::Gpu => rows.sort_by(|a, b| b.gpu_pct.total_cmp(&a.gpu_pct)),
        }
        if let Some(n) = limit {
            rows.truncate(n);
        }

        // Header
        let (hrect, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
        let mut x = hrect.left();
        for (i, (key, label, w)) in cols.iter().enumerate() {
            let cell = Rect::from_min_size(pos2(x, hrect.top()), vec2(*w, hrect.height()));
            x += w;
            let Some(key) = key else { continue };
            let resp = ui
                .interact(cell, ui.scope_id().with(("sort", i)), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);
            if resp.clicked() {
                self.sort = *key;
            }
            let active = self.sort == *key;
            let color = if active {
                TEXT
            } else if resp.hovered() {
                TEXT_2
            } else {
                TEXT_3
            };
            let (pos, align) = if i == 0 {
                (
                    pos2(cell.left() + 12.0, cell.center().y),
                    Align2::LEFT_CENTER,
                )
            } else {
                (
                    pos2(cell.right() - 12.0, cell.center().y),
                    Align2::RIGHT_CENTER,
                )
            };
            let text_rect =
                ui.painter()
                    .text(pos, align, label.to_uppercase(), bold_font(11.5), color);
            if active {
                // small "sorted descending" triangle
                let c = if i == 0 {
                    pos2(text_rect.right() + 8.0, cell.center().y)
                } else {
                    pos2(text_rect.left() - 8.0, cell.center().y)
                };
                ui.painter().add(egui::Shape::convex_polygon(
                    vec![
                        c + vec2(-3.5, -2.0),
                        c + vec2(3.5, -2.0),
                        c + vec2(0.0, 2.5),
                    ],
                    ACCENT,
                    Stroke::NONE,
                ));
            }
        }
        ui.painter().line_segment(
            [hrect.left_bottom(), hrect.right_bottom()],
            Stroke::new(1.0, BORDER),
        );
        ui.add_space(4.0);

        if rows.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("No programs match your search.").color(TEXT_2));
            return;
        }

        for g in rows {
            let (rect, resp) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
            if resp.hovered() {
                ui.painter().rect_filled(rect, 8.0, RAISED);
            }
            let display = knowledge::display_name(&g.name);
            let mut x = rect.left();
            for (key, _, w) in cols.iter() {
                let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(*w, rect.height()));
                x += w;
                let p = ui.painter_at(cell);
                let right = pos2(cell.right() - 12.0, cell.center().y);
                let heat = |p: &Painter, amount: f32| {
                    let a = amount.clamp(0.0, 1.0);
                    if a > 0.02 {
                        p.rect_filled(
                            cell.shrink2(vec2(3.0, 5.0)),
                            6.0,
                            AMBER.gamma_multiply(0.06 + 0.30 * a),
                        );
                    }
                };
                match key {
                    Some(SortKey::Name) => {
                        avatar(
                            &p,
                            pos2(cell.left() + 26.0, cell.center().y),
                            13.0,
                            &display,
                        );
                        p.text(
                            pos2(cell.left() + 48.0, cell.center().y),
                            Align2::LEFT_CENTER,
                            &display,
                            bold_font(14.0),
                            TEXT,
                        );
                    }
                    Some(SortKey::Count) => {
                        p.text(
                            right,
                            Align2::RIGHT_CENTER,
                            g.pids.len().to_string(),
                            theme::font(14.0),
                            TEXT_2,
                        );
                    }
                    Some(SortKey::Cpu) => {
                        heat(&p, g.cpu_pct / 40.0);
                        p.text(
                            right,
                            Align2::RIGHT_CENTER,
                            format!("{:.1}%", g.cpu_pct),
                            theme::font(14.0),
                            TEXT,
                        );
                    }
                    Some(SortKey::Memory) => {
                        heat(&p, g.mem as f32 / (s.mem_total.max(1) as f32 * 0.25));
                        p.text(
                            right,
                            Align2::RIGHT_CENTER,
                            fmt_bytes(g.mem),
                            theme::font(14.0),
                            TEXT,
                        );
                    }
                    Some(SortKey::Disk) => {
                        heat(&p, (g.disk_bps / 20e6) as f32);
                        p.text(
                            right,
                            Align2::RIGHT_CENTER,
                            fmt_rate(g.disk_bps),
                            theme::font(14.0),
                            TEXT,
                        );
                    }
                    Some(SortKey::Gpu) => {
                        heat(&p, g.gpu_pct / 80.0);
                        let text = match g.gpu_index {
                            Some(i) if g.gpu_pct >= 0.05 => format!("{:.1}%  GPU {i}", g.gpu_pct),
                            _ => "0%".into(),
                        };
                        p.text(right, Align2::RIGHT_CENTER, text, theme::font(14.0), TEXT);
                    }
                    None => {
                        let own = g.pids.contains(&self.own_pid);
                        if own || knowledge::is_protected(&g.name, &g.pids, self.own_pid) {
                            p.text(
                                right,
                                Align2::RIGHT_CENTER,
                                if own { "This app" } else { "System" },
                                theme::font(12.5),
                                TEXT_3,
                            );
                        } else if resp.hovered() || limit.is_none() {
                            let btn = Rect::from_center_size(
                                cell.center() + vec2(-6.0, 0.0),
                                vec2(84.0, 26.0),
                            );
                            // Drawn by hand (not a Button widget) so it can't shift the row layout.
                            let b = ui
                                .interact(btn, ui.scope_id().with(("end", &g.name)), Sense::click())
                                .on_hover_cursor(CursorIcon::PointingHand);
                            let fill = if b.hovered() { 0.20 } else { 0.08 };
                            p.rect_filled(btn, 6.0, RED.gamma_multiply(fill));
                            p.rect_stroke(
                                btn,
                                6.0,
                                Stroke::new(1.0, RED.gamma_multiply(0.4)),
                                egui::StrokeKind::Inside,
                            );
                            p.text(
                                btn.center(),
                                Align2::CENTER_CENTER,
                                "End task",
                                theme::font(12.5),
                                RED,
                            );
                            if b.clicked() {
                                self.pending = Some(PendingClose {
                                    display: display.clone(),
                                    pids: g.pids.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---- About ---------------------------------------------------------------------------

impl App {
    fn about(&mut self, ui: &mut Ui) {
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("What it does")
                    .font(bold_font(16.0))
                    .color(TEXT),
            );
            ui.add_space(4.0);
            ui.label(RichText::new(
                "Every second, PC Bottleneck Finder measures your processor, memory, storage, network and graphics cards. \
                 When one of them stays under pressure for most of the last 15 seconds, it tells you which one, \
                 which programs are responsible, and what you can do about it. Short spikes are ignored.",
            ).color(TEXT_2));
            ui.add_space(12.0);
            ui.label(
                RichText::new("What it can detect")
                    .font(bold_font(16.0))
                    .color(TEXT),
            );
            ui.add_space(4.0);
            for (title, text) in [
                (
                    "Memory",
                    "RAM nearly full, especially when Windows starts using the much slower page file.",
                ),
                (
                    "Processor",
                    "The CPU running at full load, or a program held back because it can only use one core.",
                ),
                (
                    "Storage",
                    "A disk that is busy almost all the time, and whether memory paging is the cause.",
                ),
                (
                    "Graphics",
                    "A graphics card at full load, video memory running out, and games running on a laptop's integrated graphics instead of its faster discrete GPU.",
                ),
            ] {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(title).font(bold_font(14.0)).color(TEXT));
                    ui.label(RichText::new(text).color(TEXT_2));
                });
            }
            ui.add_space(12.0);
            ui.label(
                RichText::new("Good to know")
                    .font(bold_font(16.0))
                    .color(TEXT),
            );
            ui.add_space(4.0);
            for text in [
                "Windows system processes can never be ended from this app.",
                "Some programs can only be ended when the app runs as administrator.",
                "The advice is based on rules of thumb. It catches the common cases reliably, but it's a well-informed guess, not a guarantee.",
            ] {
                ui.label(RichText::new(format!("·  {text}")).color(TEXT_2));
            }
        });
    }
}

// ---- Icons ---------------------------------------------------------------------------

fn icon_overview(p: &Painter, r: Rect, c: Color32) {
    let s = r.width() * 0.42;
    for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let min = r.min + vec2(dx * (r.width() - s), dy * (r.height() - s));
        p.rect_filled(Rect::from_min_size(min, vec2(s, s)), 2.0, c);
    }
}

fn icon_programs(p: &Painter, r: Rect, c: Color32) {
    for i in 0..3 {
        let y = r.top() + 2.0 + i as f32 * (r.height() - 4.0) / 2.0;
        p.circle_filled(pos2(r.left() + 1.5, y), 1.6, c);
        p.line_segment(
            [pos2(r.left() + 5.0, y), pos2(r.right(), y)],
            Stroke::new(2.0, c),
        );
    }
}

fn icon_about(p: &Painter, r: Rect, c: Color32) {
    p.circle_stroke(r.center(), r.width() * 0.45, Stroke::new(1.6, c));
    p.circle_filled(r.center() + vec2(0.0, -3.4), 1.2, c);
    p.line_segment(
        [r.center() + vec2(0.0, -0.8), r.center() + vec2(0.0, 4.0)],
        Stroke::new(1.8, c),
    );
}

fn check_badge(p: &Painter, r: Rect) {
    p.circle_filled(r.center(), r.width() * 0.5, GREEN.gamma_multiply(0.16));
    let c = r.center();
    let s = Stroke::new(3.0, GREEN);
    p.line_segment([c + vec2(-7.0, 0.5), c + vec2(-2.0, 5.5)], s);
    p.line_segment([c + vec2(-2.0, 5.5), c + vec2(8.0, -5.0)], s);
}
