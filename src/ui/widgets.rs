//! Small custom-drawn building blocks.

use super::theme::{self, BORDER, SURFACE, TEXT};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Frame, Mesh, Pos2, Rect, Response, RichText, Sense, Shape,
    Stroke, Ui, pos2, vec2,
};

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(12.0)
        .inner_margin(16.0)
        .show(ui, add)
}

pub fn section_title(ui: &mut Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(title)
            .font(theme::bold_font(15.0))
            .color(TEXT),
    );
    ui.add_space(2.0);
}

/// Small rounded label, e.g. "WARNING" or "Discrete".
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(99.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .font(theme::bold_font(11.0))
                    .color(color),
            );
        })
        .response
}

/// Coloured status dot.
pub fn dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
    ui.painter()
        .circle_filled(rect.center(), 6.5, color.gamma_multiply(0.18));
}

/// Line chart with a soft gradient fill underneath.
pub fn sparkline(ui: &mut Ui, height: f32, values: &[f32], max: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let p = ui.painter_at(rect);
    // faint guide lines
    for f in [0.0, 0.5, 1.0] {
        let y = rect.bottom() - f * rect.height();
        p.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, BORDER.gamma_multiply(0.5)),
        );
    }
    if values.len() < 2 {
        return;
    }
    let max = max
        .max(values.iter().cloned().fold(0.0, f32::max))
        .max(1e-6);
    // Fixed 60-sample width so the graph scrolls in from the right as data arrives.
    let slots = 59.0;
    let step = rect.width() / slots;
    let start = rect.right() - step * (values.len() - 1) as f32;
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            pos2(
                start + i as f32 * step,
                rect.bottom() - (v / max).clamp(0.0, 1.0) * (rect.height() - 2.0) - 1.0,
            )
        })
        .collect();

    let mut mesh = Mesh::default();
    let top = color.gamma_multiply(0.35);
    let bottom = color.gamma_multiply(0.0);
    for pt in &pts {
        mesh.colored_vertex(*pt, top);
        mesh.colored_vertex(pos2(pt.x, rect.bottom()), bottom);
    }
    for i in 0..pts.len() as u32 - 1 {
        let a = i * 2;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 2, a + 3);
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::line(pts, Stroke::new(1.6, color)));
}

/// Horizontal usage bar.
pub fn meter(ui: &mut Ui, frac: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, BORDER);
    let mut fill = rect;
    fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
    if fill.width() > 0.5 {
        p.rect_filled(fill, 3.0, color);
    }
}

/// Round badge with the first letter of a program name.
pub fn avatar(painter: &egui::Painter, center: Pos2, radius: f32, name: &str) {
    let palette = [
        theme::CPU,
        theme::MEMORY,
        theme::DISK,
        theme::NETWORK,
        theme::GPU[0],
        theme::GPU[1],
    ];
    let hash = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let color = palette[(hash as usize) % palette.len()];
    painter.circle_filled(center, radius, color.gamma_multiply(0.22));
    let letter = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .unwrap_or('?')
        .to_ascii_uppercase();
    painter.text(
        center,
        Align2::CENTER_CENTER,
        letter,
        theme::bold_font(radius * 1.05),
        color,
    );
}

/// Red outlined button for destructive actions.
pub fn danger_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .color(theme::RED)
                .font(theme::bold_font(13.0)),
        )
        .fill(theme::RED.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, theme::RED.gamma_multiply(0.45))),
    )
}

/// The app logo: three rising bars on a blue tile.
pub fn logo(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(
        rect,
        CornerRadius::same((rect.width() * 0.25) as u8),
        theme::ACCENT,
    );
    let w = rect.width();
    let bar = w * 0.14;
    let gap = w * 0.08;
    let base = rect.bottom() - w * 0.22;
    let left = rect.center().x - (bar * 3.0 + gap * 2.0) / 2.0;
    for (i, h) in [0.30, 0.48, 0.66].iter().enumerate() {
        let x = left + i as f32 * (bar + gap);
        let r = Rect::from_min_max(pos2(x, base - h * w), pos2(x + bar, base));
        let color = if i == 2 {
            Color32::from_rgb(0xFF, 0xD1, 0x66)
        } else {
            Color32::WHITE
        };
        painter.rect_filled(r, CornerRadius::same((bar * 0.35) as u8), color);
    }
}
