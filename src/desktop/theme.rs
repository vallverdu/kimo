//! The shared design language (foundation/design/tokens.json) for egui: colours (light and
//! dark), Roboto Mono, flat square shapes, thin lines.

use eframe::egui;
use eframe::egui::{Color32, FontData, FontDefinitions, FontFamily, Stroke};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// One colour scheme: the brand's light colours, or their dark counterparts.
pub struct Palette {
    pub background: Color32,
    pub surface: Color32,
    pub ink: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub line: Color32,
    pub accent: Color32,
    pub teal: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub bad: Color32,
    /// Chart series, validated as a pair on this scheme's surface (colour-blind safe).
    pub claude: Color32,
    pub codex: Color32,
    /// Terminal selection.
    pub selection: Color32,
    /// ANSI 0–15, tuned so agent output stays readable on this background.
    pub ansi: [Color32; 16],
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub const LIGHT: Palette = Palette {
    background: rgb(0xff, 0xff, 0xff),
    surface: rgb(0xf9, 0xf9, 0xf9),
    ink: rgb(0x23, 0x23, 0x23),
    text: rgb(0x66, 0x66, 0x66),
    muted: rgb(0x89, 0x89, 0x89),
    line: rgb(0xe5, 0xe5, 0xe5),
    accent: rgb(0xff, 0x45, 0x56),
    teal: rgb(0x12, 0x93, 0x9a),
    good: rgb(0x2f, 0x8f, 0x4e),
    warn: rgb(0xb7, 0x79, 0x1f),
    bad: rgb(0xd6, 0x33, 0x45),
    claude: rgb(0xeb, 0x68, 0x34),
    codex: rgb(0x00, 0x9a, 0xa8),
    selection: rgb(0xcf, 0xe8, 0xe9),
    ansi: [
        rgb(0x23, 0x23, 0x23),
        rgb(0xd6, 0x33, 0x45),
        rgb(0x2f, 0x8f, 0x4e),
        rgb(0xb7, 0x79, 0x1f),
        rgb(0x2b, 0x5f, 0xb4),
        rgb(0x8a, 0x3f, 0xa0),
        rgb(0x12, 0x93, 0x9a),
        rgb(0x89, 0x89, 0x89),
        rgb(0x66, 0x66, 0x66),
        rgb(0xe0, 0x49, 0x5a),
        rgb(0x3a, 0x9d, 0x5a),
        rgb(0xa8, 0x74, 0x1c),
        rgb(0x3d, 0x6f, 0xc4),
        rgb(0x9b, 0x52, 0xb1),
        rgb(0x1a, 0xa3, 0xaa),
        rgb(0xaa, 0xaa, 0xaa),
    ],
};

pub const DARK: Palette = Palette {
    background: rgb(0x1c, 0x1c, 0x1c),
    surface: rgb(0x24, 0x24, 0x24),
    ink: rgb(0xec, 0xec, 0xec),
    text: rgb(0xb8, 0xb8, 0xb8),
    muted: rgb(0x8c, 0x8c, 0x8c),
    line: rgb(0x36, 0x36, 0x36),
    accent: rgb(0xff, 0x45, 0x56),
    teal: rgb(0x3f, 0xb8, 0xbf),
    good: rgb(0x55, 0xb8, 0x72),
    warn: rgb(0xe0, 0xa8, 0x4a),
    bad: rgb(0xf2, 0x5f, 0x6e),
    claude: rgb(0xe0, 0x61, 0x2e),
    codex: rgb(0x00, 0x9a, 0xa8),
    selection: rgb(0x1f, 0x4e, 0x52),
    ansi: [
        rgb(0x3a, 0x3a, 0x3a),
        rgb(0xf2, 0x5f, 0x6e),
        rgb(0x55, 0xb8, 0x72),
        rgb(0xe0, 0xa8, 0x4a),
        rgb(0x6c, 0x9c, 0xf0),
        rgb(0xc0, 0x84, 0xd6),
        rgb(0x3f, 0xb8, 0xbf),
        rgb(0xc8, 0xc8, 0xc8),
        rgb(0x6e, 0x6e, 0x6e),
        rgb(0xff, 0x7a, 0x87),
        rgb(0x72, 0xd4, 0x8e),
        rgb(0xf0, 0xc0, 0x68),
        rgb(0x8c, 0xb4, 0xff),
        rgb(0xd6, 0xa0, 0xea),
        rgb(0x5f, 0xd4, 0xda),
        rgb(0xf0, 0xf0, 0xf0),
    ],
};

static IS_DARK: AtomicBool = AtomicBool::new(false);

/// The scheme in use.
pub fn palette() -> &'static Palette {
    if IS_DARK.load(Ordering::Relaxed) {
        &DARK
    } else {
        &LIGHT
    }
}

pub fn is_dark() -> bool {
    IS_DARK.load(Ordering::Relaxed)
}

/// Switch between the light and dark scheme (repaints with the new colours).
pub fn set_dark(ctx: &egui::Context, dark: bool) {
    if IS_DARK.swap(dark, Ordering::Relaxed) != dark {
        ctx.set_theme(if dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        ctx.request_repaint();
    }
}

pub fn background() -> Color32 {
    palette().background
}
pub fn surface() -> Color32 {
    palette().surface
}
pub fn ink() -> Color32 {
    palette().ink
}
pub fn text() -> Color32 {
    palette().text
}
pub fn muted() -> Color32 {
    palette().muted
}
pub fn line() -> Color32 {
    palette().line
}
pub fn accent() -> Color32 {
    palette().accent
}
pub fn teal() -> Color32 {
    palette().teal
}
pub fn good() -> Color32 {
    palette().good
}
pub fn warn() -> Color32 {
    palette().warn
}
pub fn bad() -> Color32 {
    palette().bad
}

pub fn status_color(status: &str) -> Color32 {
    match status {
        "running" => good(),
        "paused" => warn(),
        "exited" => bad(),
        _ => muted(),
    }
}

/// egui's widget colours for a scheme.
fn visuals(p: &Palette, dark: bool) -> egui::Visuals {
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.background;
    v.window_fill = p.background;
    v.extreme_bg_color = p.background;
    v.faint_bg_color = p.surface;
    v.code_bg_color = p.surface;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.teal;
    v.selection.bg_fill = p.ink;
    v.selection.stroke = Stroke::new(1.0, p.background);
    v.window_stroke = Stroke::new(1.0, p.line);
    v.window_corner_radius = 0.into();
    v.menu_corner_radius = 0.into();
    let (shadow, popup) = if dark { (120, 75) } else { (40, 25) };
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(shadow),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(popup),
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
    let border = if dark {
        Color32::from_gray(0x4a)
    } else {
        Color32::from_gray(0xcc)
    };
    v.widgets.noninteractive.bg_fill = p.background;
    v.widgets.noninteractive.weak_bg_fill = p.background;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.bg_fill = p.background;
    v.widgets.inactive.weak_bg_fill = p.background;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.ink);
    v.widgets.hovered.bg_fill = p.surface;
    v.widgets.hovered.weak_bg_fill = p.surface;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.ink);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.ink);
    v.widgets.active.bg_fill = p.ink;
    v.widgets.active.weak_bg_fill = p.ink;
    v.widgets.active.fg_stroke = Stroke::new(1.0, p.background);
    v.widgets.open.bg_fill = p.surface;
    v.widgets.open.weak_bg_fill = p.surface;
    v.widgets.open.fg_stroke = Stroke::new(1.0, p.ink);
    v
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

    ctx.set_visuals_of(egui::Theme::Light, visuals(&LIGHT, false));
    ctx.set_visuals_of(egui::Theme::Dark, visuals(&DARK, true));
    ctx.set_theme(if is_dark() {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
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
    let label = ui.label(egui::RichText::new(spaced).size(11.0).color(ink()));
    let y = label.rect.bottom() + 4.0;
    ui.painter()
        .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, ink()));
    ui.add_space(12.0);
}

/// Black primary button.
pub fn primary(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(background()))
            .fill(ink())
            .stroke(Stroke::new(1.0, ink())),
    )
}

/// Small status dot.
pub fn dot(ui: &mut egui::Ui, color: Color32) {
    // As tall as a row, so the dot sits on the text's centre line.
    let height = ui.spacing().interact_size.y;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, height), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}
