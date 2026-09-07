//! Pixel framebuffer writer using the built-in 8x8 font.
//!
//! Interprets a minimal ANSI/VT100 CSI subset so userspace TUIs (vim) can
//! position the cursor, clear regions, and set colors. Serial stays raw — only
//! the framebuffer path parses escapes.

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

/// ANSI 8-color palette (normal / bright), tuned to the soft dark theme.
const ANSI_FG: [(u8, u8, u8); 8] = [
    (0x28, 0x2c, 0x34), // black
    (0xf0, 0x6c, 0x75), // red
    (0x7e, 0xc9, 0x8a), // green
    (0xe5, 0xc0, 0x7b), // yellow
    (0x79, 0xb8, 0xff), // blue
    (0xc6, 0x78, 0xdd), // magenta
    (0x56, 0xb6, 0xc2), // cyan
    (0xc8, 0xd0, 0xd8), // white
];
const ANSI_FG_BRIGHT: [(u8, u8, u8); 8] = [
    (0x6b, 0x73, 0x80), // bright black (dim)
    (0xff, 0x8a, 0x92), // bright red
    (0xa3, 0xe0, 0xad), // bright green
    (0xf0, 0xd4, 0x9a), // bright yellow
    (0x9d, 0xcc, 0xff), // bright blue
    (0xdb, 0x9a, 0xee), // bright magenta
    (0x7a, 0xd0, 0xdb), // bright cyan
    (0xff, 0xff, 0xff), // bright white
];
const ANSI_BG: [(u8, u8, u8); 8] = [
    (0x12, 0x14, 0x1a), // black ≈ default BG
    (0x5a, 0x20, 0x24), // red
    (0x1e, 0x3a, 0x24), // green
    (0x3a, 0x32, 0x1a), // yellow
    (0x1a, 0x2a, 0x44), // blue
    (0x3a, 0x1e, 0x40), // magenta
    (0x1a, 0x34, 0x38), // cyan
    (0x3a, 0x40, 0x48), // white/gray
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnsiState {
    Normal,
    /// Saw ESC (0x1b).
    Escape,
    /// Saw ESC `[` — collecting CSI params until a final byte.
    Csi,
}

const CSI_MAX_PARAMS: usize = 16;

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
    bg: (u8, u8, u8),
    bold: bool,
    /// True when the next byte starts a logical line (after newline / clear).
    line_start: bool,
    /// Buffered bytes while matching a `[ TAG ] ` status prefix across writes.
    prefix_buf: [u8; 9],
    prefix_len: u8,
    ansi_state: AnsiState,
    /// Accumulated CSI numeric parameters (default when a slot was empty: 0).
    csi_params: [u16; CSI_MAX_PARAMS],
    csi_param_count: u8,
    /// True once at least one digit was seen for the current parameter slot.
    csi_has_digit: bool,
    /// CSI private-mode marker (`ESC [ ? …`).
    csi_private: bool,
    saved_col: usize,
    saved_row: usize,
    /// Inclusive scroll-region top row (0-based).
    scroll_top: usize,
    /// Inclusive scroll-region bottom row (0-based); `usize::MAX` = last row.
    scroll_bottom: usize,
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
            bg: BG,
            bold: false,
            line_start: true,
            prefix_buf: [0; 9],
            prefix_len: 0,
            ansi_state: AnsiState::Normal,
            csi_params: [0; CSI_MAX_PARAMS],
            csi_param_count: 0,
            csi_has_digit: false,
            csi_private: false,
            saved_col: 0,
            saved_row: 0,
            scroll_top: 0,
            scroll_bottom: usize::MAX,
        }
    }
}

impl FrameBufferWriter<'_> {
    fn cols(&self) -> usize {
        (self.width / FONT_W).max(1)
    }

    fn rows(&self) -> usize {
        (self.height / FONT_H).max(1)
    }

    /// Character-cell geometry for `TIOCGWINSZ`.
    pub fn winsize(&self) -> (u16, u16) {
        (self.rows() as u16, self.cols() as u16)
    }

    fn scroll_bottom_row(&self) -> usize {
        self.scroll_bottom.min(self.rows().saturating_sub(1))
    }

    fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = usize::MAX;
    }

    pub fn clear(&mut self) {
        self.clear_region(0, 0, self.cols(), self.rows());
        self.col = 0;
        self.row = 0;
        self.line_start = true;
        self.prefix_len = 0;
        self.reset_ansi_parser();
    }

    fn reset_ansi_parser(&mut self) {
        self.ansi_state = AnsiState::Normal;
        self.csi_params = [0; CSI_MAX_PARAMS];
        self.csi_param_count = 0;
        self.csi_has_digit = false;
        self.csi_private = false;
    }

    pub fn set_fg(&mut self, fg: (u8, u8, u8)) {
        self.fg = fg;
    }

    pub fn put_str_colored(&mut self, s: &str, fg: (u8, u8, u8)) {
        let saved = self.fg;
        self.fg = fg;
        // Colored helpers bypass status-prefix detection and ANSI mid-stream.
        self.prefix_len = 0;
        self.line_start = false;
        self.reset_ansi_parser();
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
        self.reset_ansi_parser();
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
        // ANSI/VT100 first: never paint ESC/CSI as glyphs, and never feed the
        // status-prefix detector while a sequence is in flight.
        match self.ansi_state {
            AnsiState::Normal => {
                if byte == 0x1b {
                    self.flush_prefix_plain();
                    self.ansi_state = AnsiState::Escape;
                    return;
                }
            }
            AnsiState::Escape => {
                self.handle_escape_byte(byte);
                return;
            }
            AnsiState::Csi => {
                self.handle_csi_byte(byte);
                return;
            }
        }

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

    fn handle_escape_byte(&mut self, byte: u8) {
        match byte {
            b'[' => {
                self.csi_params = [0; CSI_MAX_PARAMS];
                self.csi_param_count = 0;
                self.csi_has_digit = false;
                self.csi_private = false;
                self.ansi_state = AnsiState::Csi;
            }
            // DECSC / DECRC (optional): save / restore cursor.
            b'7' => {
                self.saved_col = self.col;
                self.saved_row = self.row;
                self.ansi_state = AnsiState::Normal;
            }
            b'8' => {
                self.set_cursor(self.saved_row, self.saved_col);
                self.ansi_state = AnsiState::Normal;
            }
            // RIS — reset to initial state (full screen, default colors).
            b'c' => {
                self.fg = TEXT;
                self.bg = BG;
                self.bold = false;
                self.reset_scroll_region();
                self.clear();
                self.ansi_state = AnsiState::Normal;
            }
            // Ignore other short ESC forms (e.g. ESC D IND).
            _ => {
                self.ansi_state = AnsiState::Normal;
            }
        }
    }

    fn handle_csi_byte(&mut self, byte: u8) {
        match byte {
            b'?' if self.csi_param_count == 0 && !self.csi_has_digit && !self.csi_private => {
                self.csi_private = true;
            }
            b'0'..=b'9' => {
                let digit = (byte - b'0') as u16;
                if self.csi_param_count == 0 {
                    self.csi_param_count = 1;
                }
                let idx = (self.csi_param_count as usize) - 1;
                if idx >= CSI_MAX_PARAMS {
                    self.reset_ansi_parser();
                    return;
                }
                if !self.csi_has_digit {
                    self.csi_params[idx] = digit;
                    self.csi_has_digit = true;
                } else {
                    self.csi_params[idx] = self.csi_params[idx]
                        .saturating_mul(10)
                        .saturating_add(digit);
                }
            }
            b';' => {
                // Finalize current slot (empty ⇒ 0) and open the next.
                if self.csi_param_count == 0 {
                    self.csi_param_count = 1;
                }
                if self.csi_param_count as usize >= CSI_MAX_PARAMS {
                    self.reset_ansi_parser();
                    return;
                }
                self.csi_param_count += 1;
                self.csi_has_digit = false;
            }
            // Intermediate bytes (0x20–0x2F) — ignore for the sequences we care about.
            0x20..=0x2f => {}
            // Final byte: dispatch.
            0x40..=0x7e => {
                self.dispatch_csi(byte);
                self.reset_ansi_parser();
            }
            // Cancel on unexpected control / other.
            _ => {
                self.reset_ansi_parser();
            }
        }
    }

    fn csi_param(&self, idx: usize, default: u16) -> u16 {
        if idx < self.csi_param_count as usize {
            let v = self.csi_params[idx];
            if v == 0 {
                default
            } else {
                v
            }
        } else {
            default
        }
    }

    /// Raw parameter (0 means "was empty/zero", caller decides default).
    fn csi_param_raw(&self, idx: usize) -> u16 {
        if idx < self.csi_param_count as usize {
            self.csi_params[idx]
        } else {
            0
        }
    }

    fn dispatch_csi(&mut self, final_byte: u8) {
        // Ignore private-mode sequences (CSI ? … h/l etc.) — consume only.
        if self.csi_private {
            return;
        }
        // Cursor save/restore via CSI (ANSI.SYS / xterm): s / u
        match final_byte {
            b's' => {
                self.saved_col = self.col;
                self.saved_row = self.row;
                return;
            }
            b'u' => {
                self.set_cursor(self.saved_row, self.saved_col);
                return;
            }
            _ => {}
        }

        match final_byte {
            // CUU — cursor up
            b'A' => {
                let n = self.csi_param(0, 1) as usize;
                self.row = self.row.saturating_sub(n);
                self.line_start = false;
                self.prefix_len = 0;
            }
            // CUD — cursor down
            b'B' => {
                let n = self.csi_param(0, 1) as usize;
                let max_row = self.rows().saturating_sub(1);
                self.row = (self.row + n).min(max_row);
                self.line_start = false;
                self.prefix_len = 0;
            }
            // CUF — cursor forward
            b'C' => {
                let n = self.csi_param(0, 1) as usize;
                let max_col = self.cols().saturating_sub(1);
                self.col = (self.col + n).min(max_col);
                self.line_start = false;
                self.prefix_len = 0;
            }
            // CUB — cursor back
            b'D' => {
                let n = self.csi_param(0, 1) as usize;
                self.col = self.col.saturating_sub(n);
                self.line_start = false;
                self.prefix_len = 0;
            }
            // CUP / HVP — cursor position (1-based)
            b'H' | b'f' => {
                let row = self.csi_param(0, 1) as usize;
                let col = self.csi_param(1, 1) as usize;
                self.set_cursor(row.saturating_sub(1), col.saturating_sub(1));
            }
            // ED — erase display
            b'J' => {
                let mode = self.csi_param_raw(0);
                self.erase_display(mode);
            }
            // EL — erase line
            b'K' => {
                let mode = self.csi_param_raw(0);
                self.erase_line(mode);
            }
            // SGR — select graphic rendition
            b'm' => {
                self.apply_sgr();
            }
            // DECSTBM — set scrolling region (1-based); bare CSI r resets.
            b'r' => {
                let rows = self.rows();
                if self.csi_param_count == 0 {
                    self.reset_scroll_region();
                } else {
                    let top = (self.csi_param(0, 1) as usize).saturating_sub(1);
                    let bot = (self.csi_param(1, rows as u16) as usize).saturating_sub(1);
                    if top < bot && bot < rows {
                        self.scroll_top = top;
                        self.scroll_bottom = bot;
                    } else {
                        self.reset_scroll_region();
                    }
                }
                // VT100: DECSTBM homes the cursor.
                self.set_cursor(0, 0);
            }
            // SU — scroll up n lines inside the region
            b'S' => {
                let n = self.csi_param(0, 1) as usize;
                let top = self.scroll_top.min(self.scroll_bottom_row());
                let bot = self.scroll_bottom_row();
                for _ in 0..n {
                    self.scroll_up_region(top, bot);
                }
            }
            // SD — scroll down n lines inside the region
            b'T' => {
                let n = self.csi_param(0, 1) as usize;
                let top = self.scroll_top.min(self.scroll_bottom_row());
                let bot = self.scroll_bottom_row();
                for _ in 0..n {
                    self.scroll_down_region(top, bot);
                }
            }
            // Ignore unsupported finals (still consumed).
            _ => {}
        }
    }

    fn set_cursor(&mut self, row: usize, col: usize) {
        let max_row = self.rows().saturating_sub(1);
        let max_col = self.cols().saturating_sub(1);
        self.row = row.min(max_row);
        self.col = col.min(max_col);
        self.line_start = self.col == 0;
        self.prefix_len = 0;
    }

    fn clear_region(&mut self, col0: usize, row0: usize, col1: usize, row1: usize) {
        let cols = self.cols();
        let rows = self.rows();
        let c0 = col0.min(cols);
        let c1 = col1.min(cols);
        let r0 = row0.min(rows);
        let r1 = row1.min(rows);
        for r in r0..r1 {
            for c in c0..c1 {
                // Fill with current background (SGR-aware).
                let x0 = c * FONT_W;
                let y0 = r * FONT_H;
                for dy in 0..FONT_H {
                    for dx in 0..FONT_W {
                        self.put_pixel(x0 + dx, y0 + dy, self.bg);
                    }
                }
            }
        }
    }

    fn erase_display(&mut self, mode: u16) {
        let cols = self.cols();
        let rows = self.rows();
        match mode {
            0 => {
                // Cursor to end of screen.
                self.clear_region(self.col, self.row, cols, self.row + 1);
                if self.row + 1 < rows {
                    self.clear_region(0, self.row + 1, cols, rows);
                }
            }
            1 => {
                // Start of screen to cursor.
                if self.row > 0 {
                    self.clear_region(0, 0, cols, self.row);
                }
                self.clear_region(0, self.row, self.col + 1, self.row + 1);
            }
            2 | 3 => {
                // Entire screen (3 = scrollback; we have none).
                self.clear_region(0, 0, cols, rows);
            }
            _ => {}
        }
        self.prefix_len = 0;
    }

    fn erase_line(&mut self, mode: u16) {
        let cols = self.cols();
        match mode {
            0 => {
                // Cursor to end of line.
                self.clear_region(self.col, self.row, cols, self.row + 1);
            }
            1 => {
                // Start of line to cursor.
                self.clear_region(0, self.row, self.col + 1, self.row + 1);
            }
            2 => {
                self.clear_region(0, self.row, cols, self.row + 1);
            }
            _ => {}
        }
        self.prefix_len = 0;
    }

    fn apply_sgr(&mut self) {
        // No parameters ⇒ reset (CSI m / CSI 0 m).
        let count = if self.csi_param_count == 0 {
            1
        } else {
            self.csi_param_count as usize
        };
        let mut i = 0;
        while i < count {
            let p = if self.csi_param_count == 0 {
                0
            } else {
                self.csi_params[i]
            };
            match p {
                0 => {
                    self.fg = TEXT;
                    self.bg = BG;
                    self.bold = false;
                }
                1 => {
                    self.bold = true;
                }
                22 => {
                    self.bold = false;
                }
                30..=37 => {
                    let idx = (p - 30) as usize;
                    self.fg = if self.bold {
                        ANSI_FG_BRIGHT[idx]
                    } else {
                        ANSI_FG[idx]
                    };
                }
                39 => {
                    self.fg = TEXT;
                }
                40..=47 => {
                    self.bg = ANSI_BG[(p - 40) as usize];
                }
                49 => {
                    self.bg = BG;
                }
                90..=97 => {
                    self.fg = ANSI_FG_BRIGHT[(p - 90) as usize];
                }
                100..=107 => {
                    // Bright backgrounds — approximate with normal bg palette.
                    self.bg = ANSI_BG[(p - 100) as usize];
                }
                // 38 / 48 extended colors: skip (optionally consume 2 more args).
                38 | 48 => {
                    // CSI 38;5;n or 38;2;r;g;b — skip rest of this attribute.
                    if i + 1 < count {
                        let mode = self.csi_params[i + 1];
                        if mode == 5 && i + 2 < count {
                            i += 2; // consumed 38;5;n
                        } else if mode == 2 && i + 4 < count {
                            i += 4; // consumed 38;2;r;g;b
                        } else {
                            i += 1;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
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
                // Unexpected byte mid-tag: drop the buffered prefix as plain
                // text and clear parser state so the next line can match again.
                self.flush_prefix_plain();
                debug_assert_eq!(self.prefix_len, 0);
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
                let cols = self.cols();
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
        let top = self.scroll_top.min(self.scroll_bottom_row());
        let bot = self.scroll_bottom_row();
        if self.row >= bot {
            // Newline at bottom of scroll region scrolls within the region.
            self.scroll_up_region(top, bot);
            self.row = bot;
        } else if self.row < top {
            self.row = top;
        } else {
            self.row += 1;
        }
    }

    /// Scroll text rows `[top, bottom]` up by one row (region-aware).
    fn scroll_up_region(&mut self, top: usize, bottom: usize) {
        if bottom <= top {
            return;
        }
        let src_y = (top + 1) * FONT_H;
        let dst_y = top * FONT_H;
        let height_px = (bottom - top) * FONT_H;
        let copy_len = height_px * self.pitch;
        let src_off = src_y * self.pitch;
        let dst_off = dst_y * self.pitch;
        if copy_len > 0
            && src_off + copy_len <= self.buffer.len()
            && dst_off + copy_len <= self.buffer.len()
        {
            unsafe {
                core::ptr::copy(
                    self.buffer.as_ptr().add(src_off),
                    self.buffer.as_mut_ptr().add(dst_off),
                    copy_len,
                );
            }
        }
        self.clear_region(0, bottom, self.cols(), bottom + 1);
    }

    /// Scroll text rows `[top, bottom]` down by one row (region-aware).
    fn scroll_down_region(&mut self, top: usize, bottom: usize) {
        if bottom <= top {
            return;
        }
        let src_y = top * FONT_H;
        let dst_y = (top + 1) * FONT_H;
        let height_px = (bottom - top) * FONT_H;
        let copy_len = height_px * self.pitch;
        let src_off = src_y * self.pitch;
        let dst_off = dst_y * self.pitch;
        if copy_len > 0
            && src_off + copy_len <= self.buffer.len()
            && dst_off + copy_len <= self.buffer.len()
        {
            unsafe {
                core::ptr::copy(
                    self.buffer.as_ptr().add(src_off),
                    self.buffer.as_mut_ptr().add(dst_off),
                    copy_len,
                );
            }
        }
        self.clear_region(0, top, self.cols(), top + 1);
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
                self.put_pixel(x0 + dx, y0 + dy, if on { fg } else { self.bg });
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
