//! Pixel framebuffer writer using the built-in 8x8 font.

use core::fmt;

use limine::framebuffer::Framebuffer;

use crate::font;

const FONT_W: usize = 8;
const FONT_H: usize = 8;

/// Soft dark-theme palette (framebuffer only; serial stays plain text).
pub const BG: (u8, u8, u8) = (0x12, 0x14, 0x1a);
pub const TEXT: (u8, u8, u8) = (0xc8, 0xd0, 0xd8);
pub const DIM: (u8, u8, u8) = (0x6b, 0x73, 0x80);
pub const OK: (u8, u8, u8) = (0x7e, 0xc9, 0x8a);
pub const FAIL: (u8, u8, u8) = (0xf0, 0x6c, 0x75);
pub const INFO: (u8, u8, u8) = (0x79, 0xb8, 0xff);
pub const WARN: (u8, u8, u8) = (0xe5, 0xc0, 0x7b);
pub const ACCENT: (u8, u8, u8) = (0x98, 0xc3, 0x79);

pub struct FrameBufferWriter<'a> {
    buffer: &'a mut [u8],
    width: usize,
    height: usize,
    pitch: usize,
    bytes_per_pixel: usize,
    r_shift: u8,
    g_shift: u8,
    b_shift: u8,
    r_size: u8,
    g_size: u8,
    b_size: u8,
    col: usize,
    row: usize,
    fg: (u8, u8, u8),
    /// True when the next byte starts a logical line (after newline / clear).
    line_start: bool,
    /// Buffered bytes while matching a `[ TAG ] ` status prefix across writes.
    prefix_buf: [u8; 9],
    prefix_len: u8,
}

impl FrameBufferWriter<'static> {
    pub fn from_limine(fb: &'static Framebuffer) -> Self {
        let bpp = (fb.bpp as usize).max(8);
        let bytes_per_pixel = (bpp + 7) / 8;
        Self {
            buffer: unsafe { fb.as_slice_mut() },
            width: fb.width as usize,
            height: fb.height as usize,
            pitch: fb.pitch as usize,
            bytes_per_pixel,
            r_shift: fb.red_mask_shift,
            g_shift: fb.green_mask_shift,
            b_shift: fb.blue_mask_shift,
            r_size: fb.red_mask_size.max(1),
            g_size: fb.green_mask_size.max(1),
            b_size: fb.blue_mask_size.max(1),
            col: 0,
            row: 0,
            fg: TEXT,
            line_start: true,
            prefix_buf: [0; 9],
            prefix_len: 0,
        }
    }
}

impl FrameBufferWriter<'_> {
    pub fn clear(&mut self) {
        for y in 0..self.height {
            for x in 0..self.width {
                self.put_pixel(x, y, BG);
            }
        }
        self.col = 0;
        self.row = 0;
        self.line_start = true;
        self.prefix_len = 0;
    }

    pub fn set_fg(&mut self, fg: (u8, u8, u8)) {
        self.fg = fg;
    }

    pub fn put_str_colored(&mut self, s: &str, fg: (u8, u8, u8)) {
        let saved = self.fg;
        self.fg = fg;
        // Colored helpers bypass status-prefix detection.
        self.prefix_len = 0;
        self.line_start = false;
        for byte in s.bytes() {
            self.put_byte_colored(byte, fg);
        }
        self.fg = saved;
        if s.as_bytes().last() == Some(&0x0a) {
            self.line_start = true;
        }
    }

    /// `[ TAG ] label` with semantic tag color and default text for the label.
    pub fn put_status_line(&mut self, tag: &str, tag_color: (u8, u8, u8), label: &str) {
        // Bypass prefix detection: render directly then start a fresh line.
        self.prefix_len = 0;
        self.line_start = false;
        self.put_byte_colored(b'[', DIM);
        self.put_byte_colored(b' ', DIM);
        for byte in tag.bytes() {
            self.put_byte_colored(byte, tag_color);
        }
        self.put_byte_colored(b' ', DIM);
        self.put_byte_colored(b']', DIM);
        if !label.is_empty() {
            self.put_byte_colored(b' ', TEXT);
            for byte in label.bytes() {
                self.put_byte_colored(byte, TEXT);
            }
        }
        self.newline();
        self.line_start = true;
    }

    pub fn put_str(&mut self, s: &str) {
        for byte in s.bytes() {
            self.put_byte(byte);
        }
    }

    pub fn put_byte(&mut self, byte: u8) {
        // Color status tags that arrive via the plain byte path (modules +
        // userspace `write_str`), including when the prefix is split across
        // writes. Serial stays plain.
        if byte == b'\n' {
            self.flush_prefix_plain();
            self.put_byte_colored(b'\n', self.fg);
            self.line_start = true;
            return;
        }
        if self.line_start || self.prefix_len > 0 {
            self.push_status_prefix_byte(byte);
            return;
        }
        self.put_byte_colored(byte, self.fg);
    }

    fn flush_prefix_plain(&mut self) {
        if self.prefix_len == 0 {
            return;
        }
        let n = self.prefix_len as usize;
        let buf = self.prefix_buf;
        self.prefix_len = 0;
        self.line_start = false;
        for &b in &buf[..n] {
            self.put_byte_colored(b, self.fg);
        }
    }

    fn push_status_prefix_byte(&mut self, byte: u8) {
        if self.prefix_len as usize >= self.prefix_buf.len() {
            self.flush_prefix_plain();
            self.put_byte_colored(byte, self.fg);
            return;
        }
        self.prefix_buf[self.prefix_len as usize] = byte;
        self.prefix_len += 1;
        let n = self.prefix_len as usize;
        let buf = &self.prefix_buf[..n];
        match match_status_prefix(buf) {
            PrefixMatch::Complete(tag, color) => {
                self.prefix_len = 0;
                self.line_start = false;
                self.emit_status_tag(tag, color);
                self.fg = TEXT;
            }
            PrefixMatch::Partial => {}
            PrefixMatch::None => {
                self.flush_prefix_plain();
            }
        }
    }

    fn emit_status_tag(&mut self, tag: &str, tag_color: (u8, u8, u8)) {
        self.put_byte_colored(b'[', DIM);
        self.put_byte_colored(b' ', DIM);
        for byte in tag.bytes() {
            self.put_byte_colored(byte, tag_color);
        }
        self.put_byte_colored(b' ', DIM);
        self.put_byte_colored(b']', DIM);
        self.put_byte_colored(b' ', TEXT);
    }

    fn put_byte_colored(&mut self, byte: u8, fg: (u8, u8, u8)) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            b'\x08' => {
                if self.col > 0 {
                    self.col -= 1;
                    self.draw_glyph(self.col, self.row, b' ', self.fg);
                }
            }
            byte => {
                let cols = (self.width / FONT_W).max(1);
                if self.col >= cols {
                    self.newline();
                }
                self.draw_glyph(self.col, self.row, byte, fg);
                self.col += 1;
            }
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        let rows = (self.height / FONT_H).max(1);
        if self.row + 1 >= rows {
            self.scroll_up();
        } else {
            self.row += 1;
        }
    }

    /// Scroll the text viewport up one row when the cursor passes the last line.
    fn scroll_up(&mut self) {
        let text_rows = (self.height / FONT_H).max(1);
        if text_rows <= 1 {
            self.row = 0;
            return;
        }
        let scroll_px = FONT_H;
        let visible_px = text_rows * FONT_H;
        let copy_len = (visible_px - scroll_px) * self.pitch;
        if copy_len > 0 && copy_len <= self.buffer.len() {
            unsafe {
                core::ptr::copy(
                    self.buffer.as_ptr().add(scroll_px * self.pitch),
                    self.buffer.as_mut_ptr(),
                    copy_len,
                );
            }
        }
        let y0 = visible_px - scroll_px;
        for y in y0..visible_px {
            for x in 0..self.width {
                self.put_pixel(x, y, BG);
            }
        }
        self.row = text_rows - 1;
    }

    fn draw_glyph(&mut self, col: usize, row: usize, byte: u8, fg: (u8, u8, u8)) {
        let glyph = font::glyph(if (0x20..0x7F).contains(&byte) {
            byte
        } else {
            b'?'
        });
        let x0 = col * FONT_W;
        let y0 = row * FONT_H;
        for (dy, bits) in glyph.iter().copied().enumerate() {
            for dx in 0..FONT_W {
                let on = bits & (1 << dx) != 0;
                self.put_pixel(x0 + dx, y0 + dy, if on { fg } else { BG });
            }
        }
    }

    fn put_pixel(&mut self, x: usize, y: usize, (r, g, b): (u8, u8, u8)) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = y * self.pitch + x * self.bytes_per_pixel;
        let Some(pixel) = self.buffer.get_mut(offset..offset + self.bytes_per_pixel) else {
            return;
        };
        let mut val = 0u32;
        val |= (scale(r, self.r_size) as u32) << self.r_shift;
        val |= (scale(g, self.g_size) as u32) << self.g_shift;
        val |= (scale(b, self.b_size) as u32) << self.b_shift;
        let bytes = val.to_le_bytes();
        let n = pixel.len().min(4);
        pixel[..n].copy_from_slice(&bytes[..n]);
    }
}


enum PrefixMatch {
    /// Full `[ TAG ] ` consumed; paint tag then label in TEXT.
    Complete(&'static str, (u8, u8, u8)),
    /// Still a prefix of at least one known status tag.
    Partial,
    /// Cannot be a status tag — flush as plain text.
    None,
}

fn match_status_prefix(buf: &[u8]) -> PrefixMatch {
    // Keep spacing identical to console::status_*: `[ TAG ] ` (space after `]`).
    const TAGS: &[(&str, &str, (u8, u8, u8))] = &[
        ("[ OK ] ", "OK", OK),
        ("[ FAIL ] ", "FAIL", FAIL),
        ("[ INFO ] ", "INFO", INFO),
        ("[ WARN ] ", "WARN", WARN),
        ("[ .. ] ", "..", INFO),
    ];
    let mut partial = false;
    for &(full, tag, color) in TAGS {
        let bytes = full.as_bytes();
        if buf == bytes {
            return PrefixMatch::Complete(tag, color);
        }
        if bytes.starts_with(buf) {
            partial = true;
        }
    }
    if partial {
        PrefixMatch::Partial
    } else {
        PrefixMatch::None
    }
}

fn scale(channel: u8, bits: u8) -> u8 {
    if bits >= 8 {
        channel
    } else {
        channel >> (8 - bits)
    }
}

impl fmt::Write for FrameBufferWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.put_str(s);
        Ok(())
    }
}
