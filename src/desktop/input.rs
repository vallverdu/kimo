//! Keyboard, clipboard and mouse wheel events → bytes for the terminal (xterm conventions).

use eframe::egui::{Event, Key, Modifiers};

/// Terminal modes that change what keys send.
#[derive(Debug, Clone, Copy, Default)]
pub struct Modes {
    /// DECCKM: arrows send `ESC O x` instead of `ESC [ x`.
    pub app_cursor: bool,
    /// Pasted text is wrapped in `ESC [200~ … ESC [201~`.
    pub bracketed_paste: bool,
}

fn key_bytes(key: Key, m: Modifiers, modes: Modes) -> Option<Vec<u8>> {
    let arrow = |c: char| {
        let s = if m.shift || m.alt || m.ctrl {
            let code = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
            format!("\x1b[1;{code}{c}")
        } else if modes.app_cursor {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        };
        Some(s.into_bytes())
    };
    let plain = |s: &str| Some(s.as_bytes().to_vec());
    match key {
        Key::Enter => plain(if m.alt { "\x1b\r" } else { "\r" }),
        Key::Tab => plain(if m.shift { "\x1b[Z" } else { "\t" }),
        Key::Backspace => plain(if m.alt {
            "\x1b\x7f"
        } else if m.ctrl {
            "\x08"
        } else {
            "\x7f"
        }),
        Key::Escape => plain("\x1b"),
        Key::ArrowUp => arrow('A'),
        Key::ArrowDown => arrow('B'),
        Key::ArrowRight => arrow('C'),
        Key::ArrowLeft => arrow('D'),
        Key::Home => plain(if modes.app_cursor { "\x1bOH" } else { "\x1b[H" }),
        Key::End => plain(if modes.app_cursor { "\x1bOF" } else { "\x1b[F" }),
        Key::PageUp => plain("\x1b[5~"),
        Key::PageDown => plain("\x1b[6~"),
        Key::Delete => plain("\x1b[3~"),
        Key::Insert => plain("\x1b[2~"),
        Key::F1 => plain("\x1bOP"),
        Key::F2 => plain("\x1bOQ"),
        Key::F3 => plain("\x1bOR"),
        Key::F4 => plain("\x1bOS"),
        _ if m.ctrl && !m.shift => {
            // Ctrl+letter → control character; a few punctuation keys too.
            let name = key.name();
            let c = name.chars().next().filter(|_| name.len() == 1)?;
            match c.to_ascii_lowercase() {
                l @ 'a'..='z' => {
                    let control = (l as u8) & 0x1f;
                    Some(if m.alt {
                        vec![0x1b, control]
                    } else {
                        vec![control]
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Bytes for one input event, or `None` when the terminal does not use it.
pub fn encode(event: &Event, modes: Modes) -> Option<Vec<u8>> {
    match event {
        Event::Text(text) => Some(text.as_bytes().to_vec()),
        // egui turns Ctrl+C / Ctrl+X into clipboard events; in a terminal they are ^C / ^X.
        Event::Copy => Some(vec![0x03]),
        Event::Cut => Some(vec![0x18]),
        Event::Paste(text) => {
            let text = text.replace("\r\n", "\r").replace('\n', "\r");
            Some(
                if modes.bracketed_paste {
                    format!("\x1b[200~{text}\x1b[201~")
                } else {
                    text
                }
                .into_bytes(),
            )
        }
        Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => {
            // Ctrl+Shift+V / C are the app's paste / copy, not terminal input.
            if modifiers.ctrl && modifiers.shift {
                return None;
            }
            key_bytes(*key, *modifiers, modes)
        }
        _ => None,
    }
}

/// How a program wants the mouse wheel (from its terminal modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wheel {
    /// Scroll kimo's own scrollback.
    Scrollback,
    /// The program reads mouse events (SGR encoding when `sgr`).
    Report { sgr: bool },
    /// Full-screen program without mouse reporting: the wheel sends arrow keys (xterm's
    /// "alternate scroll"), so pagers and TUIs scroll.
    Arrows { app_cursor: bool },
}

/// Bytes for `lines` wheel steps (positive = up) over cell `col`, `row` (0-based).
pub fn wheel(lines: i32, col: usize, row: usize, how: Wheel) -> Vec<u8> {
    let up = lines > 0;
    let steps = lines.unsigned_abs().min(20) as usize;
    let one: Vec<u8> = match how {
        Wheel::Scrollback => return vec![],
        Wheel::Report { sgr: true } => format!(
            "\x1b[<{};{};{}M",
            if up { 64 } else { 65 },
            col + 1,
            row + 1
        )
        .into_bytes(),
        Wheel::Report { sgr: false } => {
            let b = if up { 64u8 } else { 65 };
            let pos = |v: usize| (32 + 1 + v.min(222)) as u8;
            vec![0x1b, b'[', b'M', 32 + b, pos(col), pos(row)]
        }
        Wheel::Arrows { app_cursor } => {
            let c = if up { 'A' } else { 'B' };
            if app_cursor {
                format!("\x1bO{c}")
            } else {
                format!("\x1b[{c}")
            }
            .into_bytes()
        }
    };
    one.repeat(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_steps() {
        assert_eq!(wheel(2, 0, 0, Wheel::Scrollback), b"");
        assert_eq!(
            wheel(1, 4, 2, Wheel::Report { sgr: true }),
            b"\x1b[<64;5;3M"
        );
        assert_eq!(
            wheel(-1, 0, 0, Wheel::Report { sgr: true }),
            b"\x1b[<65;1;1M"
        );
        assert_eq!(
            wheel(-1, 0, 0, Wheel::Report { sgr: false }),
            vec![0x1b, b'[', b'M', 97, 33, 33]
        );
        assert_eq!(
            wheel(2, 0, 0, Wheel::Arrows { app_cursor: false }),
            b"\x1b[A\x1b[A"
        );
        assert_eq!(
            wheel(-1, 0, 0, Wheel::Arrows { app_cursor: true }),
            b"\x1bOB"
        );
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn keys() {
        let n = Modes::default();
        assert_eq!(encode(&key(Key::Enter, Modifiers::NONE), n).unwrap(), b"\r");
        assert_eq!(
            encode(&key(Key::Backspace, Modifiers::NONE), n).unwrap(),
            b"\x7f"
        );
        assert_eq!(
            encode(&key(Key::Tab, Modifiers::SHIFT), n).unwrap(),
            b"\x1b[Z"
        );
        assert_eq!(
            encode(&key(Key::ArrowUp, Modifiers::NONE), n).unwrap(),
            b"\x1b[A"
        );
        let app = Modes {
            app_cursor: true,
            ..n
        };
        assert_eq!(
            encode(&key(Key::ArrowUp, Modifiers::NONE), app).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            encode(&key(Key::ArrowLeft, Modifiers::CTRL), n).unwrap(),
            b"\x1b[1;5D"
        );
        assert_eq!(encode(&key(Key::C, Modifiers::CTRL), n).unwrap(), b"\x03");
        assert_eq!(encode(&key(Key::D, Modifiers::CTRL), n).unwrap(), b"\x04");
        assert!(encode(&key(Key::V, Modifiers::CTRL | Modifiers::SHIFT), n).is_none());
        assert!(
            encode(&key(Key::A, Modifiers::NONE), n).is_none(),
            "letters come as Text events"
        );
        assert_eq!(
            encode(&Event::Text("hé".into()), n).unwrap(),
            "hé".as_bytes()
        );
        assert_eq!(
            encode(&Event::Copy, n).unwrap(),
            b"\x03",
            "Ctrl+C interrupts"
        );
        assert_eq!(encode(&Event::Cut, n).unwrap(), b"\x18");
    }

    #[test]
    fn paste() {
        let p = Event::Paste("a\nb".into());
        assert_eq!(encode(&p, Modes::default()).unwrap(), b"a\rb");
        assert_eq!(
            encode(
                &p,
                Modes {
                    bracketed_paste: true,
                    ..Modes::default()
                }
            )
            .unwrap(),
            b"\x1b[200~a\rb\x1b[201~"
        );
    }
}
