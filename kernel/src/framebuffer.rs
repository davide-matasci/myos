//! Pixel framebuffer writer using the built-in 8x8 font.

use bootloader_api::info::{FrameBuffer, PixelFormat};
use core::fmt;

use crate::font;

const FONT_W: usize = 8;
const FONT_H: usize = 8;
const FG: (u8, u8, u8) = (0x55, 0xFF, 0x55);
const BG: (u8, u8, u8) = (0x00, 0x00, 0x00);

pub struct FrameBufferWriter<'a> {
    buffer: &'a mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    bytes_per_pixel: usize,
    pixel_format: PixelFormat,
    col: usize,
    row: usize,
}

impl<'a> FrameBufferWriter<'a> {
    pub fn new(fb: &'a mut FrameBuffer) -> Self {
        let info = fb.info();
        Self {
            buffer: fb.buffer_mut(),
            width: info.width,
            height: info.height,
            stride: info.stride,
            bytes_per_pixel: info.bytes_per_pixel,
            pixel_format: info.pixel_format,
            col: 0,
            row: 0,
        }
    }

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

    fn put_byte(&mut self, byte: u8) {
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
        let offset = (y * self.stride + x) * self.bytes_per_pixel;
        let Some(pixel) = self.buffer.get_mut(offset..offset + self.bytes_per_pixel) else {
            return;
        };
        match self.pixel_format {
            PixelFormat::Rgb => {
                pixel[0] = r;
                pixel[1] = g;
                pixel[2] = b;
            }
            PixelFormat::Bgr => {
                pixel[0] = b;
                pixel[1] = g;
                pixel[2] = r;
            }
            PixelFormat::U8 => {
                pixel[0] = g;
            }
            PixelFormat::Unknown {
                red_position,
                green_position,
                blue_position,
            } => {
                if let Some(p) = pixel.get_mut(red_position as usize) {
                    *p = r;
                }
                if let Some(p) = pixel.get_mut(green_position as usize) {
                    *p = g;
                }
                if let Some(p) = pixel.get_mut(blue_position as usize) {
                    *p = b;
                }
            }
            _ => {}
        }
    }
}

impl fmt::Write for FrameBufferWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.put_str(s);
        Ok(())
    }
}
