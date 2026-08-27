//! Direct writes to the VGA text buffer at 0xb8000.
//!
//! 80 columns by 25 rows. Each cell is an ASCII byte plus a color byte.
//! Writes go through `write_volatile` so the compiler cannot drop them.

use core::fmt;
use core::ptr::{read_volatile, write_volatile};

const VGA_ADDRESS: usize = 0xb8000;
const WIDTH: usize = 80;
const HEIGHT: usize = 25;

#[allow(dead_code)]
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

#[derive(Clone, Copy)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    const fn pack(fg: Color, bg: Color) -> Self {
        Self((bg as u8) << 4 | (fg as u8))
    }
}

/// Writes ASCII text into VGA memory, wrapping and scrolling as needed.
pub struct Writer {
    col: usize,
    row: usize,
    color: ColorCode,
}

impl Writer {
    /// Identity-mapped VGA text buffer, cleared, cursor at the top-left.
    pub fn new() -> Self {
        let mut writer = Self {
            col: 0,
            row: 0,
            color: ColorCode::pack(Color::LightGreen, Color::Black),
        };
        writer.clear();
        writer
    }

    pub fn clear(&mut self) {
        for row in 0..HEIGHT {
            for col in 0..WIDTH {
                self.poke(row, col, b' ');
            }
        }
        self.col = 0;
        self.row = 0;
    }

    pub fn put_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            byte => {
                if self.col >= WIDTH {
                    self.newline();
                }
                let glyph = if (0x20..0x7f).contains(&byte) {
                    byte
                } else {
                    b'?'
                };
                self.poke(self.row, self.col, glyph);
                self.col += 1;
            }
        }
    }

    pub fn put_str(&mut self, s: &str) {
        for byte in s.bytes() {
            self.put_byte(byte);
        }
    }

    fn poke(&self, row: usize, col: usize, byte: u8) {
        let index = row * WIDTH + col;
        let cell = u16::from(byte) | (u16::from(self.color.0) << 8);
        unsafe {
            write_volatile((VGA_ADDRESS as *mut u16).add(index), cell);
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        if self.row + 1 >= HEIGHT {
            self.scroll();
        } else {
            self.row += 1;
        }
    }

    fn scroll(&mut self) {
        let ptr = VGA_ADDRESS as *mut u16;
        for row in 1..HEIGHT {
            for col in 0..WIDTH {
                let src = row * WIDTH + col;
                let dst = (row - 1) * WIDTH + col;
                unsafe {
                    let value = read_volatile(ptr.add(src));
                    write_volatile(ptr.add(dst), value);
                }
            }
        }
        for col in 0..WIDTH {
            self.poke(HEIGHT - 1, col, b' ');
        }
        self.row = HEIGHT - 1;
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.put_str(s);
        Ok(())
    }
}
