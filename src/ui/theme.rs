//! Colors, fonts and widget styling.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, Visuals, vec2,
};
use std::sync::Arc;

pub const SIDEBAR: Color32 = Color32::from_rgb(0x0B, 0x0E, 0x13);
pub const BG: Color32 = Color32::from_rgb(0x10, 0x14, 0x1A);
pub const SURFACE: Color32 = Color32::from_rgb(0x17, 0x1C, 0x24);
pub const RAISED: Color32 = Color32::from_rgb(0x1E, 0x24, 0x2E);
pub const HOVER: Color32 = Color32::from_rgb(0x24, 0x2B, 0x36);
pub const BORDER: Color32 = Color32::from_rgb(0x27, 0x2E, 0x39);

pub const TEXT: Color32 = Color32::from_rgb(0xE8, 0xEC, 0xF2);
pub const TEXT_2: Color32 = Color32::from_rgb(0x9C, 0xA6, 0xB4);
pub const TEXT_3: Color32 = Color32::from_rgb(0x6A, 0x75, 0x84);

pub const ACCENT: Color32 = Color32::from_rgb(0x4F, 0x8C, 0xFF);
pub const RED: Color32 = Color32::from_rgb(0xF2, 0x5F, 0x5C);
pub const AMBER: Color32 = Color32::from_rgb(0xF5, 0xA5, 0x24);
pub const GREEN: Color32 = Color32::from_rgb(0x2F, 0xBF, 0x71);

pub const CPU: Color32 = Color32::from_rgb(0x4F, 0x8C, 0xFF);
pub const MEMORY: Color32 = Color32::from_rgb(0xA1, 0x7C, 0xF2);
pub const DISK: Color32 = Color32::from_rgb(0x2F, 0xBF, 0x9B);
pub const NETWORK: Color32 = Color32::from_rgb(0xF5, 0xA5, 0x24);
pub const GPU: [Color32; 3] = [
    Color32::from_rgb(0x22, 0xB8, 0xCF),
    Color32::from_rgb(0xF0, 0x6C, 0xB4),
    Color32::from_rgb(0xC3, 0xD6, 0x4A),
];

/// Font family for headings and emphasis.
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn bold_font(size: f32) -> FontId {
    FontId::new(size, bold())
}

pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_visuals(visuals());
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, font(22.0)),
            (TextStyle::Body, font(14.0)),
            (TextStyle::Button, font(14.0)),
            (TextStyle::Small, font(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = vec2(8.0, 6.0);
        s.button_padding = vec2(12.0, 5.0);
        s.interact_size.y = 30.0;
        s.window_margin = egui::Margin::same(20);
        style.visuals = visuals();
    });
}

fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = SIDEBAR;
    v.faint_bg_color = RAISED;
    v.weak_text_color = Some(TEXT_2);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(14);
    v.window_shadow = Shadow {
        offset: [0, 12],
        blur: 32,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;

    let r = CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = SURFACE;
    w.noninteractive.weak_bg_fill = SURFACE;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = r;
    for (state, fill, stroke) in [
        (&mut w.inactive, RAISED, BORDER),
        (&mut w.hovered, HOVER, Color32::from_rgb(0x3A, 0x44, 0x52)),
        (&mut w.active, Color32::from_rgb(0x2B, 0x33, 0x3F), ACCENT),
        (&mut w.open, RAISED, BORDER),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, TEXT);
        state.corner_radius = r;
        state.expansion = 0.0;
    }
    v
}

/// Uses the native Windows font (Segoe UI) when available, falling back to Inter or
/// egui's built-in font. Fonts are loaded from the system, not bundled.
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let load = |paths: &[&str]| paths.iter().find_map(|p| std::fs::read(p).ok());
    let regular = load(&[
        r"C:\Windows\Fonts\segoeui.ttf",
        "/usr/share/fonts/opentype/inter/Inter-Regular.otf",
        "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
    ]);
    let semibold = load(&[
        r"C:\Windows\Fonts\seguisb.ttf",
        r"C:\Windows\Fonts\segoeuib.ttf",
        "/usr/share/fonts/opentype/inter/Inter-SemiBold.otf",
        "/usr/share/fonts/truetype/inter/Inter-SemiBold.ttf",
    ]);
    let symbols = load(&[r"C:\Windows\Fonts\seguisym.ttf"]);

    let mut proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if let Some(bytes) = symbols {
        fonts
            .font_data
            .insert("symbols".into(), Arc::new(FontData::from_owned(bytes)));
        proportional.insert(0, "symbols".into());
    }
    if let Some(bytes) = regular {
        fonts
            .font_data
            .insert("ui".into(), Arc::new(FontData::from_owned(bytes)));
        proportional.insert(0, "ui".into());
    }
    let mut bold = proportional.clone();
    if let Some(bytes) = semibold {
        fonts
            .font_data
            .insert("ui-bold".into(), Arc::new(FontData::from_owned(bytes)));
        bold.insert(0, "ui-bold".into());
    }
    fonts
        .families
        .insert(FontFamily::Proportional, proportional);
    fonts.families.insert(self::bold(), bold);
    fonts
}
