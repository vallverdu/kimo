//! A kimo terminal in egui: alacritty_terminal parses the session's output (from the server),
//! egui paints the grid, keys go back over the same connection.

use super::input::{self, Modes};
use super::theme;
use crate::client::{Client, TermIn, TermOut, TerminalLink};
use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};
use eframe::egui;
use eframe::egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, Vec2};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Terminal replies (device attributes, cursor position…) go back to the program.
#[derive(Clone)]
struct Responder(Sender<TermOut>);

impl EventListener for Responder {
    fn send_event(&self, event: TermEvent) {
        if let TermEvent::PtyWrite(text) = event {
            let _ = self.0.send(TermOut::Bytes(text.into_bytes()));
        }
    }
}

pub struct TerminalView {
    pub session_id: String,
    /// The kimo host the session runs on (`local` or a peer id).
    pub host: String,
    /// The session has appeared in its host's list (until then a missing session is just new).
    pub seen: bool,
    term: Term<Responder>,
    parser: Processor,
    link: TerminalLink,
    size: (u16, u16),
    scroll_rest: f32,
}

/// Selection highlight (light teal, readable under any text colour).
const SELECTION: Color32 = Color32::from_rgb(0xcf, 0xe8, 0xe9);

/// Grid cell under a position, with the half of the cell it is in.
pub fn cell_at(
    pos: egui::Pos2,
    origin: egui::Pos2,
    cw: f32,
    rh: f32,
    cols: usize,
    rows: usize,
    offset: usize,
) -> (Point, Side) {
    let x = ((pos.x - origin.x) / cw).max(0.0);
    let col = (x.floor() as usize).min(cols.saturating_sub(1));
    let row = (((pos.y - origin.y) / rh).max(0.0).floor() as usize).min(rows.saturating_sub(1));
    let side = if x.fract() < 0.5 {
        Side::Left
    } else {
        Side::Right
    };
    (
        Point::new(Line(row as i32 - offset as i32), Column(col)),
        side,
    )
}

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

/// Colour of a cell colour, honouring palette changes made by the program (OSC 4/10/11).
pub fn resolve(color: Color, colors: &Colors) -> Color32 {
    match color {
        Color::Spec(c) => rgb(c),
        Color::Indexed(i) => colors[i as usize].map(rgb).unwrap_or_else(|| indexed(i)),
        Color::Named(n) => colors[n].map(rgb).unwrap_or_else(|| named(n)),
    }
}

fn named(n: NamedColor) -> Color32 {
    match n {
        NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => theme::INK,
        NamedColor::Background => theme::BACKGROUND,
        NamedColor::DimForeground => theme::MUTED,
        other => {
            let i = other as usize;
            if i < 16 {
                theme::ANSI[i]
            } else {
                // Dim colours: the normal colour, lighter.
                let base = theme::ANSI[(i.saturating_sub(NamedColor::DimBlack as usize)) % 8];
                base.gamma_multiply(0.7)
            }
        }
    }
}

/// xterm 256-colour palette.
pub fn indexed(i: u8) -> Color32 {
    match i {
        0..=15 => theme::ANSI[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let g = 8 + (i - 232) * 10;
            Color32::from_rgb(g, g, g)
        }
    }
}

impl TerminalView {
    pub fn open(client: &Client, host: &str, session_id: &str, ctx: &egui::Context) -> Self {
        let repaint = ctx.clone();
        let link = client.terminal(session_id, Arc::new(move || repaint.request_repaint()));
        let term = Term::new(
            Config::default(),
            &TermSize::new(100, 30),
            Responder(link.tx.clone()),
        );
        Self {
            session_id: session_id.to_string(),
            host: host.to_string(),
            seen: false,
            term,
            parser: Processor::new(),
            link,
            size: (0, 0),
            scroll_rest: 0.0,
        }
    }

    fn reset(&mut self) {
        let (cols, rows) = (self.size.0.max(10) as usize, self.size.1.max(4) as usize);
        self.term = Term::new(
            Config::default(),
            &TermSize::new(cols, rows),
            Responder(self.link.tx.clone()),
        );
        self.parser = Processor::new();
    }

    fn pump(&mut self) {
        while let Ok(msg) = self.link.rx.try_recv() {
            match msg {
                TermIn::Reset => {
                    self.reset();
                    // The server should know our size again.
                    if self.size.0 > 0 {
                        let _ = self.link.tx.send(TermOut::Resize {
                            cols: self.size.0,
                            rows: self.size.1,
                        });
                    }
                }
                TermIn::Data(bytes) => self.parser.advance(&mut self.term, &bytes),
            }
        }
    }

    /// Type text into the terminal (quick commands, the message box).
    pub fn send(&self, text: &str) {
        let _ = self.link.tx.send(TermOut::Bytes(text.as_bytes().to_vec()));
    }

    /// Paint the terminal in the remaining space and handle its input.
    pub fn show(&mut self, ui: &mut Ui, font_size: f32) {
        self.pump();
        let font = FontId::monospace(font_size);
        let (cw, rh) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        let id = egui::Id::unique(("terminal", &self.session_id));
        let rect = ui.available_rect_before_wrap();
        let response = ui.interact(rect, id, Sense::click_and_drag());
        ui.advance_cursor_after_rect(rect);
        if response.clicked() || !ui.memory(|m| m.focused().is_some()) {
            response.request_focus();
        }

        // Size the grid to the space.
        let cols = ((rect.width() - 8.0) / cw).floor().max(10.0) as u16;
        let rows = ((rect.height() - 8.0) / rh).floor().max(4.0) as u16;
        if (cols, rows) != self.size {
            self.size = (cols, rows);
            self.term
                .resize(TermSize::new(cols as usize, rows as usize));
            let _ = self.link.tx.send(TermOut::Resize { cols, rows });
        }

        // Selection: drag selects, double-click selects a word, right-click copies.
        let origin = rect.min + Vec2::splat(4.0);
        let (cols_n, rows_n, offset) = (
            self.size.0 as usize,
            self.size.1 as usize,
            self.term.grid().display_offset(),
        );
        if let Some(pos) = response.interact_pointer_pos() {
            let (point, side) = cell_at(pos, origin, cw, rh, cols_n, rows_n, offset);
            if response.double_clicked() {
                self.term.selection = Some(Selection::new(SelectionType::Semantic, point, side));
            } else if response.drag_started() {
                self.term.selection = Some(Selection::new(SelectionType::Simple, point, side));
            } else if response.dragged() {
                if let Some(sel) = self.term.selection.as_mut() {
                    sel.update(point, side);
                }
            } else if response.clicked() {
                self.term.selection = None;
            }
        }
        if response.secondary_clicked() {
            self.copy_selection(ui);
        }

        // Input.
        let focused = response.has_focus();
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
            let mode = *self.term.mode();
            let modes = Modes {
                app_cursor: mode.contains(TermMode::APP_CURSOR),
                bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            };
            let events = ui.input(|i| i.events.clone());
            let mut typed = false;
            for event in &events {
                // Ctrl+C copies when something is selected (^C otherwise); Ctrl+Shift+C always copies.
                let copy_key = matches!(event, egui::Event::Key { key: egui::Key::C, pressed: true, modifiers, .. } if modifiers.ctrl && modifiers.shift);
                if copy_key || (matches!(event, egui::Event::Copy) && self.has_selection()) {
                    self.copy_selection(ui);
                    continue;
                }
                if let Some(bytes) = input::encode(event, modes) {
                    let _ = self.link.tx.send(TermOut::Bytes(bytes));
                    typed = true;
                }
            }
            if typed {
                self.term.selection = None;
                self.term.scroll_display(Scroll::Bottom);
            }
        }
        if response.hovered() {
            self.scroll_rest += ui.input(|i| i.smooth_scroll_delta.y) / rh;
            let lines = self.scroll_rest.trunc() as i32;
            if lines != 0 {
                self.scroll_rest -= lines as f32;
                self.term.scroll_display(Scroll::Delta(lines));
            }
        }

        self.paint(ui, rect, &font, cw, rh, focused);
    }

    fn has_selection(&self) -> bool {
        self.term.selection.as_ref().is_some_and(|s| !s.is_empty())
    }

    fn copy_selection(&mut self, ui: &Ui) {
        if let Some(text) = self.term.selection_to_string().filter(|t| !t.is_empty()) {
            ui.ctx().copy_text(text);
        }
        self.term.selection = None;
    }

    fn paint(&self, ui: &Ui, rect: Rect, font: &FontId, cw: f32, rh: f32, focused: bool) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::BACKGROUND);
        let origin = rect.min + Vec2::splat(4.0);
        let content = self.term.renderable_content();
        let selection = content.selection;
        let offset = content.display_offset as i32;
        let colors = content.colors;
        let cell_rect = |col: usize, row: i32, width: usize| {
            Rect::from_min_size(
                origin + Vec2::new(col as f32 * cw, row as f32 * rh),
                Vec2::new(cw * width as f32, rh),
            )
        };

        // Runs of plain ASCII with the same colour are drawn as one string; anything else per cell
        // (fallback glyphs may have other widths, so they are pinned to their cell).
        let mut run = String::new();
        let mut run_start = (0usize, 0i32, Color32::TRANSPARENT, false);
        let flush = |run: &mut String, start: (usize, i32, Color32, bool)| {
            if !run.is_empty() {
                let pos = cell_rect(start.0, start.1, 1).min;
                painter.text(pos, Align2::LEFT_TOP, run.as_str(), font.clone(), start.2);
                if start.3 {
                    let y = pos.y + rh - 2.0;
                    painter.hline(
                        pos.x..=pos.x + cw * run.chars().count() as f32,
                        y,
                        Stroke::new(1.0, start.2),
                    );
                }
                run.clear();
            }
        };
        for indexed in content.display_iter {
            let cell = indexed.cell;
            let row = indexed.point.line.0 + offset;
            let col = indexed.point.column.0;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let (mut fg, mut bg) = (resolve(cell.fg, colors), resolve(cell.bg, colors));
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if selection.is_some_and(|r| r.contains(indexed.point)) {
                bg = SELECTION;
            }
            if cell.flags.intersects(Flags::DIM) {
                fg = fg.gamma_multiply(0.65);
            }
            let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            if bg != theme::BACKGROUND {
                painter.rect_filled(cell_rect(col, row, width), 0.0, bg);
            }
            let hidden = cell.flags.contains(Flags::HIDDEN);
            let underline = cell.flags.intersects(Flags::ALL_UNDERLINES);
            let c = if hidden { ' ' } else { cell.c };
            let continues = !run.is_empty()
                && run_start.1 == row
                && run_start.2 == fg
                && run_start.3 == underline
                && run_start.0 + run.chars().count() == col;
            if c.is_ascii() && width == 1 {
                if !continues {
                    flush(&mut run, run_start);
                    run_start = (col, row, fg, underline);
                }
                run.push(c);
            } else {
                flush(&mut run, run_start);
                if c != ' ' {
                    painter.text(
                        cell_rect(col, row, width).center(),
                        Align2::CENTER_CENTER,
                        c.to_string(),
                        font.clone(),
                        fg,
                    );
                }
            }
        }
        flush(&mut run, run_start);

        // Cursor (programs like Claude Code hide it and draw their own).
        if content.mode.contains(TermMode::SHOW_CURSOR) && offset == 0 {
            let p = content.cursor.point;
            let r = cell_rect(p.column.0, p.line.0, 1);
            if focused {
                painter.rect_filled(r, 0.0, theme::INK.gamma_multiply(0.85));
            } else {
                painter.rect_stroke(
                    r,
                    0.0,
                    Stroke::new(1.0, theme::INK),
                    egui::StrokeKind::Inside,
                );
            }
        }
        if offset > 0 {
            painter.text(
                rect.right_top() + Vec2::new(-8.0, 6.0),
                Align2::RIGHT_TOP,
                format!("↑ {offset} lines"),
                FontId::monospace(11.0),
                theme::MUTED,
            );
        }
        let _ = self.term.grid().columns();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_under_the_pointer() {
        let origin = egui::pos2(4.0, 4.0);
        let (p, side) = cell_at(
            egui::pos2(4.0 + 8.0 * 2.2, 4.0 + 16.0 * 3.5),
            origin,
            8.0,
            16.0,
            80,
            24,
            0,
        );
        assert_eq!((p.line.0, p.column.0, side), (3, 2, Side::Left));
        let (p, side) = cell_at(
            egui::pos2(4.0 + 8.0 * 2.7, 4.0),
            origin,
            8.0,
            16.0,
            80,
            24,
            5,
        );
        assert_eq!(
            (p.line.0, p.column.0, side),
            (-5, 2, Side::Right),
            "scrolled back 5 lines"
        );
        let (p, _) = cell_at(egui::pos2(9999.0, 9999.0), origin, 8.0, 16.0, 80, 24, 0);
        assert_eq!((p.line.0, p.column.0), (23, 79), "clamped to the grid");
    }

    #[test]
    fn palette() {
        assert_eq!(indexed(1), theme::BAD);
        assert_eq!(indexed(16), Color32::from_rgb(0, 0, 0));
        assert_eq!(indexed(231), Color32::from_rgb(255, 255, 255));
        assert_eq!(indexed(232), Color32::from_rgb(8, 8, 8));
        let colors = Colors::default();
        assert_eq!(
            resolve(Color::Named(NamedColor::Foreground), &colors),
            theme::INK
        );
        assert_eq!(
            resolve(Color::Spec(Rgb { r: 1, g: 2, b: 3 }), &colors),
            Color32::from_rgb(1, 2, 3)
        );
    }
}
