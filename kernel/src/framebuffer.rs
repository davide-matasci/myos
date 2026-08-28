//! Pixel framebuffer writer using the built-in 8x8 font.

use core::fmt;

use limine::framebuffer::Framebuffer;

use crate::font;

const FONT_W: usize = 8;
const FONT_H: usize = 8;
const FG: (u8, u8, u8) = (0x55, 0xFF, 0x55);
const BG: (u8, u8, u8) = (0x00, 0x00, 0x00);

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
    }

    pub fn put_str(&mut self, s: &str) {
        for byte in s.bytes() {
            self.put_byte(byte);
        }
    }

    pub fn put_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            byte => {
                let cols = (self.width / FONT_W).max(1);
                if self.col >= cols {
                    self.newline();
                }
                self.draw_glyph(self.col, self.row, byte);
                self.col += 1;
            }
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        let rows = (self.height / FONT_H).max(1);
        if self.row + 1 >= rows {
            self.row = 0;
        } else {
            self.row += 1;
        }
    }

    fn draw_glyph(&mut self, col: usize, row: usize, byte: u8) {
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
                self.put_pixel(x0 + dx, y0 + dy, if on { FG } else { BG });
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
