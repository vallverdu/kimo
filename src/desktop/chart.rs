//! A daily column chart: one stacked column per day (Claude below, Codex above), hairline
//! grid, the peak labelled, a tooltip per column.

use super::app::tokens;
use super::theme;
use eframe::egui;
use eframe::egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Ui};

/// Series colours, validated as a pair on each scheme's surface (colour-blind safe).
pub fn claude() -> Color32 {
    theme::palette().claude
}

pub fn codex() -> Color32 {
    theme::palette().codex
}

pub struct Column {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub claude: u64,
    pub codex: u64,
    pub prompts: u64,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `2026-10-19` → `19 Oct`.
pub fn short_date(date: &str) -> String {
    let mut parts = date.split('-').skip(1);
    match (
        parts.next().and_then(|m| m.parse::<usize>().ok()),
        parts.next(),
    ) {
        (Some(m @ 1..=12), Some(d)) => format!("{} {}", d.trim_start_matches('0'), MONTHS[m - 1]),
        _ => date.to_string(),
    }
}

/// 1, 2 or 5 times a power of ten, at least `x`.
fn nice(x: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    let p = 10f64.powf(x.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * p)
        .find(|v| *v >= x)
        .unwrap_or(10.0 * p)
}

/// A legend entry: a swatch shaped like the mark (a small rect) and its name.
pub fn legend(ui: &mut Ui, color: Color32, name: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(2), color);
    ui.label(egui::RichText::new(name).size(11.0).color(theme::text()));
    ui.add_space(8.0);
}

pub fn daily(ui: &mut Ui, cols: &[Column], height: f32) {
    let width = ui.available_width().max(200.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter_at(rect.expand(2.0));
    let axis = 44.0; // y labels on the left
    let bottom = 20.0; // x labels below
    let plot = Rect::from_min_max(
        pos2(rect.left() + axis, rect.top() + 14.0),
        pos2(rect.right(), rect.bottom() - bottom),
    );
    let max = cols.iter().map(|c| c.claude + c.codex).max().unwrap_or(0);
    let step = nice(max as f64 / 4.0);
    let top = (max as f64 / step).ceil().max(1.0) * step;
    let y = |v: f64| plot.bottom() - (v / top) as f32 * plot.height();
    let small = FontId::proportional(10.0);

    // Grid: hairlines at clean values, the baseline a step darker.
    let mut v = 0.0;
    while v <= top + 0.5 {
        let yy = y(v).round() + 0.5;
        let stroke = if v == 0.0 {
            Stroke::new(1.0, theme::muted().gamma_multiply(0.6))
        } else {
            Stroke::new(1.0, theme::line())
        };
        painter.hline(plot.x_range(), yy, stroke);
        painter.text(
            pos2(plot.left() - 8.0, yy),
            Align2::RIGHT_CENTER,
            tokens(v as u64),
            small.clone(),
            theme::muted(),
        );
        v += step;
    }

    let n = cols.len().max(1);
    let slot = plot.width() / n as f32;
    let bar = (slot * 0.62).clamp(2.0, 24.0);
    let hovered = response
        .hover_pos()
        .filter(|p| plot.x_range().contains(p.x))
        .map(|p| (((p.x - plot.left()) / slot) as usize).min(n - 1));
    if let Some(i) = hovered {
        let x = plot.left() + slot * i as f32;
        painter.rect_filled(
            Rect::from_min_max(pos2(x, plot.top()), pos2(x + slot, plot.bottom())),
            CornerRadius::ZERO,
            theme::surface(),
        );
    }
    let peak = cols
        .iter()
        .enumerate()
        .max_by_key(|(_, c)| c.claude + c.codex)
        .filter(|(_, c)| c.claude + c.codex > 0)
        .map(|(i, _)| i);
    let every = n.div_ceil(7).max(1);
    for (i, c) in cols.iter().enumerate() {
        let cx = plot.left() + slot * (i as f32 + 0.5);
        let (x0, x1) = (cx - bar / 2.0, cx + bar / 2.0);
        let base = plot.bottom();
        let h1 = base - y(c.claude as f64);
        let h2 = base - y(c.codex as f64);
        // The top segment carries the rounded end; segments are split by a 2px surface gap.
        if c.claude > 0 {
            let round = if c.codex > 0 {
                CornerRadius::ZERO
            } else {
                CornerRadius {
                    nw: 4,
                    ne: 4,
                    sw: 0,
                    se: 0,
                }
            };
            painter.rect_filled(
                Rect::from_min_max(pos2(x0, base - h1.max(1.0)), pos2(x1, base)),
                round,
                claude(),
            );
        }
        if c.codex > 0 {
            let start = if c.claude > 0 { base - h1 - 2.0 } else { base };
            let end = (start - h2).min(start - 1.0);
            painter.rect_filled(
                Rect::from_min_max(pos2(x0, end), pos2(x1, start)),
                CornerRadius {
                    nw: 4,
                    ne: 4,
                    sw: 0,
                    se: 0,
                },
                codex(),
            );
        }
        if peak == Some(i) {
            let top_y = base
                - h1
                - h2
                - if c.claude > 0 && c.codex > 0 {
                    2.0
                } else {
                    0.0
                };
            painter.text(
                pos2(cx, top_y - 3.0),
                Align2::CENTER_BOTTOM,
                tokens(c.claude + c.codex),
                small.clone(),
                theme::ink(),
            );
        }
        // Dates: the first, the last, and evenly between them (never crowding the last).
        let last = i == n - 1;
        if last || (i % every == 0 && n - 1 - i >= every.div_ceil(2)) {
            painter.text(
                pos2(cx, plot.bottom() + 5.0),
                Align2::CENTER_TOP,
                short_date(&c.date),
                small.clone(),
                theme::muted(),
            );
        }
    }

    if let Some(c) = hovered.and_then(|i| cols.get(i)) {
        response.on_hover_ui_at_pointer(|ui| {
            ui.label(
                egui::RichText::new(short_date(&c.date))
                    .size(11.0)
                    .color(theme::muted()),
            );
            for (color, name, value) in
                [(codex(), "codex", c.codex), (claude(), "claude", c.claude)]
            {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                    ui.painter()
                        .hline(r.x_range(), r.center().y, Stroke::new(2.0, color));
                    ui.label(
                        egui::RichText::new(tokens(value))
                            .strong()
                            .color(theme::ink()),
                    );
                    ui.label(egui::RichText::new(name).size(11.0).color(theme::muted()));
                });
            }
            ui.label(
                egui::RichText::new(format!("{} prompts", c.prompts))
                    .size(11.0)
                    .color(theme::text()),
            );
        });
    }
}

/// A thin horizontal bar: Claude and Codex shares of `scale` (the largest row).
pub fn share(ui: &mut Ui, claude: u64, codex: u64, scale: u64, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 10.0), Sense::hover());
    let painter = ui.painter();
    let scale = scale.max(1) as f32;
    let w1 = width * claude as f32 / scale;
    let w2 = width * codex as f32 / scale;
    let y = rect.y_range();
    if claude > 0 {
        let round = if codex > 0 {
            CornerRadius::ZERO
        } else {
            CornerRadius {
                nw: 0,
                ne: 3,
                sw: 0,
                se: 3,
            }
        };
        painter.rect_filled(
            Rect::from_x_y_ranges(rect.left()..=rect.left() + w1.max(1.0), y),
            round,
            self::claude(),
        );
    }
    if codex > 0 {
        let start = rect.left() + if claude > 0 { w1 + 2.0 } else { 0.0 };
        painter.rect_filled(
            Rect::from_x_y_ranges(start..=start + w2.max(1.0), y),
            CornerRadius {
                nw: 0,
                ne: 3,
                sw: 0,
                se: 3,
            },
            self::codex(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_and_dates() {
        assert_eq!(nice(0.0), 1.0);
        assert_eq!(nice(230.0), 500.0);
        assert_eq!(nice(1_000.0), 1_000.0);
        assert_eq!(nice(1_100_000.0), 2_000_000.0);
        assert_eq!(short_date("2026-10-09"), "9 Oct");
        assert_eq!(short_date("bad"), "bad");
    }
}
