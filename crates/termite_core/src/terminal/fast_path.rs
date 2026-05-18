use crate::grid::is_fast_width1_char;
use crate::{ParserAdapter, Style};

use super::TerminalCore;

impl<P> TerminalCore<P>
where
    P: ParserAdapter,
{
    pub(super) fn process_fast_sgr(&mut self, input: &[u8]) -> Option<usize> {
        if let Some(len) = self.process_simple_fast_sgr(input) {
            return Some(len);
        }

        let params = input.strip_prefix(b"\x1b[")?;
        let end = params.iter().position(|byte| *byte == b'm')?;
        if !params[..end]
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b';')
        {
            return None;
        }

        if end == 0 {
            self.style = Style::default();
            return Some(3);
        }

        for param in params[..end].split(|byte| *byte == b';') {
            let code = if param.is_empty() {
                0
            } else {
                parse_sgr_code(param)?
            };
            self.apply_fast_sgr_code(code)?;
        }
        Some(end + 3)
    }

    fn process_simple_fast_sgr(&mut self, input: &[u8]) -> Option<usize> {
        match input {
            [0x1b, b'[', b'0', b'm', ..] => {
                self.style = Style::default();
                Some(4)
            }
            [0x1b, b'[', b'1', b'm', ..] => {
                self.style.set_bold(true);
                Some(4)
            }
            [0x1b, b'[', b'3', b'm', ..] => {
                self.style.set_italic(true);
                Some(4)
            }
            [0x1b, b'[', b'4', b'm', ..] => {
                self.style.set_underline(true);
                Some(4)
            }
            [0x1b, b'[', b'3', digit @ b'0'..=b'7', b'm', ..] => {
                self.style.foreground = crate::Color::Indexed(digit - b'0');
                Some(5)
            }
            [0x1b, b'[', b'4', digit @ b'0'..=b'7', b'm', ..] => {
                self.style.background = crate::Color::Indexed(digit - b'0');
                Some(5)
            }
            [0x1b, b'[', b'9', digit @ b'0'..=b'7', b'm', ..] => {
                self.style.foreground = crate::Color::Indexed(digit - b'0' + 8);
                Some(5)
            }
            [0x1b, b'[', b'3', b'9', b'm', ..] => {
                self.style.foreground = crate::Color::DefaultForeground;
                Some(5)
            }
            [0x1b, b'[', b'4', b'9', b'm', ..] => {
                self.style.background = crate::Color::DefaultBackground;
                Some(5)
            }
            [0x1b, b'[', b'2', b'2', b'm', ..] => {
                self.style.set_bold(false);
                Some(5)
            }
            [0x1b, b'[', b'2', b'3', b'm', ..] => {
                self.style.set_italic(false);
                Some(5)
            }
            [0x1b, b'[', b'2', b'4', b'm', ..] => {
                self.style.set_underline(false);
                Some(5)
            }
            [0x1b, b'[', b'1', b'0', digit @ b'0'..=b'7', b'm', ..] => {
                self.style.background = crate::Color::Indexed(digit - b'0' + 8);
                Some(6)
            }
            _ => None,
        }
    }

    fn apply_fast_sgr_code(&mut self, code: u16) -> Option<()> {
        match code {
            0 => self.style = Style::default(),
            1 => self.style.set_bold(true),
            3 => self.style.set_italic(true),
            4 => self.style.set_underline(true),
            22 => self.style.set_bold(false),
            23 => self.style.set_italic(false),
            24 => self.style.set_underline(false),
            30..=37 => self.style.foreground = crate::Color::Indexed((code - 30) as u8),
            39 => self.style.foreground = crate::Color::DefaultForeground,
            40..=47 => self.style.background = crate::Color::Indexed((code - 40) as u8),
            49 => self.style.background = crate::Color::DefaultBackground,
            90..=97 => self.style.foreground = crate::Color::Indexed((code - 90 + 8) as u8),
            100..=107 => self.style.background = crate::Color::Indexed((code - 100 + 8) as u8),
            _ => return None,
        }
        Some(())
    }

    pub(super) fn process_fast_text(&mut self, input: &[u8]) -> usize {
        let mut index = 0;
        while index < input.len() {
            if input[index].is_ascii_graphic() || input[index] == b' ' {
                let start = index;
                while index < input.len()
                    && (input[index].is_ascii_graphic() || input[index] == b' ')
                {
                    index += 1;
                }
                let run = &input[start..index];
                self.last_printed = char::from(input[index - 1]);
                if fast_width1_char_at(input, index).is_some()
                    && let Some(run) =
                        scan_fast_width1_run(input, start, None, &mut self.fast_width1_chars)
                {
                    self.grid
                        .put_width1_chars(&self.fast_width1_chars, self.style);
                    self.last_printed = run.last;
                    index = run.end;
                    continue;
                }
                if input.get(index..index + 2) == Some(b"\r\n")
                    && let Some((consumed, last_printed)) = self
                        .grid
                        .put_scrolling_ascii_crlf_runs(&input[start..], self.style)
                {
                    self.last_printed = last_printed;
                    index = start + consumed;
                    continue;
                }
                if input.get(index..index + 2) == Some(b"\r\n")
                    && self.grid.put_ascii_run_crlf(run, self.style)
                {
                    index += 2;
                    continue;
                }
                self.grid.put_ascii_run(run, self.style);
                continue;
            }
            if let Some(ch) = decode_utf8_char(input, index)
                && !ch.is_control()
            {
                if is_fast_width1_non_ascii(ch)
                    && let Some(run) =
                        scan_fast_width1_run(input, index, Some(ch), &mut self.fast_width1_chars)
                {
                    self.grid
                        .put_width1_chars(&self.fast_width1_chars, self.style);
                    self.last_printed = run.last;
                    index = run.end;
                    continue;
                }
                let _ = self.grid.put_char(ch, self.style);
                self.last_printed = ch;
                index += ch.len_utf8();
                continue;
            }

            match input[index] {
                b'\r' if input.get(index + 1) == Some(&b'\n') => {
                    self.grid.carriage_return_line_feed();
                    index += 2;
                    continue;
                }
                b'\n' | 0x0b | 0x0c => {
                    let _ = self.grid.put_char('\n', self.style);
                }
                b'\r' => {
                    let _ = self.grid.move_cursor(0, self.grid.cursor().y);
                }
                b'\t' => self.grid.put_tab(self.style),
                0x08 => {
                    let cursor = self.grid.cursor();
                    if cursor.x > 0 {
                        let _ = self.grid.move_cursor(cursor.x - 1, cursor.y);
                    }
                }
                0x20..=0x7e => {
                    let ch = char::from(input[index]);
                    let _ = self.grid.put_char(ch, self.style);
                    self.last_printed = ch;
                }
                _ => break,
            }
            index += 1;
        }
        index
    }

    pub(super) fn process_print_text(&mut self, text: &str) {
        let mut index = 0;
        while index < text.len() {
            let bytes = text.as_bytes();
            if bytes[index].is_ascii_graphic() || bytes[index] == b' ' {
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_graphic() || bytes[index] == b' ')
                {
                    index += 1;
                }
                self.last_printed = char::from(bytes[index - 1]);
                if fast_width1_char_at(bytes, index).is_some()
                    && let Some(run) =
                        scan_fast_width1_run(bytes, start, None, &mut self.fast_width1_chars)
                {
                    self.grid
                        .put_width1_chars(&self.fast_width1_chars, self.style);
                    self.last_printed = run.last;
                    index = run.end;
                    continue;
                }
                self.grid.put_ascii_run(&bytes[start..index], self.style);
                continue;
            }

            let ch = text[index..].chars().next().expect("valid char boundary");
            if is_fast_width1_non_ascii(ch)
                && let Some(run) =
                    scan_fast_width1_run(bytes, index, Some(ch), &mut self.fast_width1_chars)
            {
                self.grid
                    .put_width1_chars(&self.fast_width1_chars, self.style);
                self.last_printed = run.last;
                index = run.end;
                continue;
            }
            let _ = self.grid.put_char(ch, self.style);
            self.last_printed = ch;
            index += ch.len_utf8();
        }
    }
}

fn decode_utf8_char(input: &[u8], index: usize) -> Option<char> {
    let first = *input.get(index)?;
    let len = match first {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    let bytes = input.get(index..index + len)?;
    std::str::from_utf8(bytes).ok()?.chars().next()
}

#[derive(Debug, Clone, Copy)]
struct FastWidth1Run {
    end: usize,
    last: char,
}

fn scan_fast_width1_run(
    input: &[u8],
    start: usize,
    first: Option<char>,
    chars: &mut Vec<char>,
) -> Option<FastWidth1Run> {
    chars.clear();
    let mut index = start;
    let mut last = None;
    let mut saw_non_ascii = first.is_some();
    if let Some(ch) = first {
        chars.push(ch);
        last = Some(ch);
        index += ch.len_utf8();
    }

    while index < input.len() {
        let byte = input[index];
        if byte.is_ascii_graphic() || byte == b' ' {
            let ch = char::from(byte);
            chars.push(ch);
            last = Some(ch);
            index += 1;
            continue;
        }
        let Some(ch) = fast_width1_char_at(input, index) else {
            break;
        };
        saw_non_ascii = true;
        chars.push(ch);
        last = Some(ch);
        index += ch.len_utf8();
    }

    if saw_non_ascii {
        Some(FastWidth1Run {
            end: index,
            last: last?,
        })
    } else {
        None
    }
}

fn fast_width1_char_at(input: &[u8], index: usize) -> Option<char> {
    let ch = decode_utf8_char(input, index)?;
    is_fast_width1_non_ascii(ch).then_some(ch)
}

#[inline]
fn is_fast_width1_non_ascii(ch: char) -> bool {
    !ch.is_ascii() && !ch.is_control() && is_fast_width1_char(ch)
}

fn parse_sgr_code(bytes: &[u8]) -> Option<u16> {
    let mut value = 0u16;
    for byte in bytes {
        let digit = byte.checked_sub(b'0')?;
        if digit > 9 {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u16::from(digit))?;
    }
    Some(value)
}
