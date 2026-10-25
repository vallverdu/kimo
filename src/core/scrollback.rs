//! Bounded terminal output buffer replayed to clients that (re)connect.

use std::collections::VecDeque;

pub struct Scrollback {
    buf: VecDeque<u8>,
    cap: usize,
}

impl Scrollback {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap.min(64 * 1024)),
            cap,
        }
    }

    /// Append output, dropping the oldest bytes beyond the capacity.
    pub fn push(&mut self, data: &[u8]) {
        let data = if data.len() > self.cap {
            &data[data.len() - self.cap..]
        } else {
            data
        };
        let overflow = (self.buf.len() + data.len()).saturating_sub(self.cap);
        self.buf.drain(..overflow);
        self.buf.extend(data);
    }

    /// The last `n` bytes.
    pub fn tail(&self, n: usize) -> Vec<u8> {
        self.buf
            .iter()
            .skip(self.buf.len().saturating_sub(n))
            .copied()
            .collect()
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_bytes() {
        let mut s = Scrollback::new(5);
        s.push(b"abc");
        s.push(b"defg");
        assert_eq!(s.snapshot(), b"cdefg");
        s.push(b"0123456789");
        assert_eq!(s.snapshot(), b"56789");
        assert_eq!(s.len(), 5);
        assert_eq!(s.tail(2), b"89");
        assert_eq!(s.tail(99), b"56789");
    }
}
