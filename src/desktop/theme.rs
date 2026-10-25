//! The shared design language (foundation/design/tokens.json) for egui: colours, Roboto Mono,
//! flat square shapes, thin lines.

use eframe::egui;
use eframe::egui::{Color32, FontData, FontDefinitions, FontFamily, Stroke};
use std::sync::Arc;

pub const BACKGROUND: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
pub const SURFACE: Color32 = Color32::from_rgb(0xf9, 0xf9, 0xf9);
pub const INK: Color32 = Color32::from_rgb(0x23, 0x23, 0x23);
pub const TEXT: Color32 = Color32::from_rgb(0x66, 0x66, 0x66);
pub const MUTED: Color32 = Color32::from_rgb(0x89, 0x89, 0x89);
pub const LINE: Color32 = Color32::from_rgb(0xe5, 0xe5, 0xe5);
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x45, 0x56);
pub const TEAL: Color32 = Color32::from_rgb(0x12, 0x93, 0x9a);
pub const GOOD: Color32 = Color32::from_rgb(0x2f, 0x8f, 0x4e);
pub const WARN: Color32 = Color32::from_rgb(0xb7, 0x79, 0x1f);
pub const BAD: Color32 = Color32::from_rgb(0xd6, 0x33, 0x45);

/// ANSI 0–15, darkened so agent output stays readable on white (same as the web terminal).
pub const ANSI: [Color32; 16] = [
    INK,
    BAD,
    GOOD,
    WARN,
    Color32::from_rgb(0x2b, 0x5f, 0xb4),
    Color32::from_rgb(0x8a, 0x3f, 0xa0),
    TEAL,
    MUTED,
    TEXT,
    Color32::from_rgb(0xe0, 0x49, 0x5a),
    Color32::from_rgb(0x3a, 0x9d, 0x5a),
    Color32::from_rgb(0xa8, 0x74, 0x1c),
    Color32::from_rgb(0x3d, 0x6f, 0xc4),
    Color32::from_rgb(0x9b, 0x52, 0xb1),
    Color32::from_rgb(0x1a, 0xa3, 0xaa),
    Color32::from_rgb(0xaa, 0xaa, 0xaa),
];

pub fn status_color(status: &str) -> Color32 {
    match status {
        "running" => GOOD,
        "paused" => WARN,
        "exited" => BAD,
        _ => MUTED,
    }
}

pub fn apply(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "roboto-mono".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/fonts/RobotoMono.ttf"
        ))),
    );
    // Fallbacks for the symbols agents draw (● ✓ ✻ ❯ ⏵, box drawing) that Roboto Mono lacks.
    fonts.font_data.insert(
        "dejavu-mono".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/fonts/DejaVuSansMono.ttf"
        ))),
    );
    fonts.font_data.insert(
        "noto-symbols".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/fonts/NotoSansSymbols2-Regular.ttf"
        ))),
    );
    for family in [FontFamily::Monospace, FontFamily::Proportional] {
        let list = fonts.families.entry(family).or_default();
        for (i, name) in ["roboto-mono", "dejavu-mono", "noto-symbols"]
            .into_iter()
            .enumerate()
        {
            list.insert(i, name.into());
        }
    }
    ctx.set_fonts(fonts);

    ctx.set_visuals_of(egui::Theme::Light, {
        let mut v = egui::Visuals::light();
        v.panel_fill = BACKGROUND;
        v.window_fill = BACKGROUND;
        v.extreme_bg_color = BACKGROUND;
        v.faint_bg_color = SURFACE;
        v.override_text_color = Some(TEXT);
        v.hyperlink_color = TEAL;
        v.selection.bg_fill = INK;
        v.selection.stroke = Stroke::new(1.0, BACKGROUND);
        v.window_stroke = Stroke::new(1.0, LINE);
        v.window_corner_radius = 0.into();
        v.menu_corner_radius = 0.into();
        v.window_shadow = egui::epaint::Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(40),
        };
        v.popup_shadow = egui::epaint::Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(25),
        };
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = 0.into();
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.inactive.bg_fill = BACKGROUND;
        v.widgets.inactive.weak_bg_fill = BACKGROUND;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_gray(0xcc));
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, INK);
        v.widgets.hovered.bg_fill = SURFACE;
        v.widgets.hovered.weak_bg_fill = SURFACE;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, INK);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, INK);
        v.widgets.active.bg_fill = INK;
        v.widgets.active.weak_bg_fill = INK;
        v.widgets.active.fg_stroke = Stroke::new(1.0, BACKGROUND);
        v
    });
    ctx.set_theme(egui::Theme::Light);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(22.0));
        s.text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
        s.text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(12.5));
        s.text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(11.0));
    });
}

/// Small uppercase, letter-spaced section title with a 1 px ink rule (the shared style).
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    let spaced: String = title
        .to_uppercase()
        .chars()
        .flat_map(|c| [c, '\u{2009}'])
        .collect();
    let label = ui.label(egui::RichText::new(spaced).size(11.0).color(INK));
    let y = label.rect.bottom() + 4.0;
    ui.painter()
        .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, INK));
    ui.add_space(12.0);
}

/// Black primary button.
pub fn primary(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(BACKGROUND))
            .fill(INK)
            .stroke(Stroke::new(1.0, INK)),
    )
}

/// Small status dot.
pub fn dot(ui: &mut egui::Ui, color: Color32) {
    // As tall as a row, so the dot sits on the text's centre line.
    let height = ui.spacing().interact_size.y;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, height), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}
