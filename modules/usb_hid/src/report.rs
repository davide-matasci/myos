//! The HID report descriptor (HID 1.11, section 6.2.2), as far as a
//! pointer needs it: where the X and Y axes, the wheel and the buttons are
//! in an input report, and their logical ranges. The descriptor is a list
//! of items, each a prefix byte (size, type, tag) and 0, 1, 2 or 4 bytes of
//! data; the global items (usage page, logical range, report size, count
//! and id) hold until changed, the local ones (usages) last for one main
//! item. Every `Input` main item lays out `count` fields of `size` bits
//! after the ones before it in its report.

/// Buttons a pointer may have (usages 1 to 5 of the button page).
pub const MAX_BUTTONS: usize = 5;

const PAGE_DESKTOP: u32 = 0x01;
const PAGE_BUTTON: u32 = 0x09;
const USAGE_X: u32 = 0x30;
const USAGE_Y: u32 = 0x31;
const USAGE_WHEEL: u32 = 0x38;

/// One field of the report: its first bit (after the report id byte, if
/// any), its width and range.
#[derive(Clone, Copy, Default)]
pub struct Field {
    pub bit: u16,
    pub size: u8,
    pub min: i32,
    pub max: i32,
    /// Relative (motion) rather than absolute (a position).
    pub relative: bool,
}

impl Field {
    /// The field's value in `report` (sign-extended when its range is).
    pub fn get(&self, report: &[u8]) -> Option<i32> {
        if self.size == 0 || self.size > 32 {
            return None;
        }
        let mut v: u32 = 0;
        for i in 0..u16::from(self.size) {
            let bit = self.bit + i;
            let byte = *report.get(usize::from(bit / 8))?;
            v |= u32::from((byte >> (bit % 8)) & 1) << i;
        }
        if self.min < 0 && self.size < 32 && v & (1 << (self.size - 1)) != 0 {
            v |= u32::MAX << self.size;
        }
        Some(v as i32)
    }
}

/// Where a pointer's report keeps what it says.
#[derive(Clone, Copy, Default)]
pub struct Layout {
    /// The report id of the pointer's report (0: the device uses none, and
    /// its reports have no id byte).
    pub id: u8,
    pub x: Field,
    pub y: Field,
    pub wheel: Option<Field>,
    pub buttons: [Option<Field>; MAX_BUTTONS],
}

impl Layout {
    /// The boot protocol's mouse report (HID 1.11, appendix B.2): three
    /// buttons in bits 0 to 2, then X and Y as signed bytes.
    pub fn boot_mouse() -> Layout {
        let axis = |bit| Field { bit, size: 8, min: -127, max: 127, relative: true };
        let button = |bit| Some(Field { bit, size: 1, min: 0, max: 1, relative: false });
        Layout {
            id: 0,
            x: axis(8),
            y: axis(16),
            wheel: None,
            buttons: [button(0), button(1), button(2), None, None],
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Globals {
    page: u32,
    min: i32,
    max: i32,
    size: u32,
    count: u32,
    id: u8,
}

/// Walk descriptor `d`, calling `field(id, page, usage, field)` for every
/// variable, non-constant input field. `None` when it is malformed.
fn walk(d: &[u8], mut field: impl FnMut(u8, u32, u32, Field)) -> Option<()> {
    let mut g = Globals::default();
    let mut stack = [Globals::default(); 4];
    let mut depth = 0;
    // Local items: the usages of the next main item, listed or a range.
    let mut usages = [0u32; 16];
    let mut n_usages = 0;
    let mut range: Option<(u32, u32)> = None;
    // The bits laid out so far, per report id.
    let mut bits = [0u32; 256];
    let mut i = 0;
    while i < d.len() {
        let prefix = d[i];
        if prefix == 0xFE {
            // A long item: its data size is the next byte.
            i += 3 + usize::from(*d.get(i + 1)?);
            continue;
        }
        let len = [0, 1, 2, 4][usize::from(prefix & 3)];
        let data = d.get(i + 1..i + 1 + len)?;
        i += 1 + len;
        let mut u: u32 = 0;
        for (k, b) in data.iter().enumerate() {
            u |= u32::from(*b) << (8 * k);
        }
        // Signed: sign-extended from the item's size.
        let s = match len {
            1 => i32::from(u as u8 as i8),
            2 => i32::from(u as u16 as i16),
            _ => u as i32,
        };
        let tag = prefix >> 4;
        match (prefix >> 2) & 3 {
            // Main: Input lays out fields; every main item ends the locals.
            0 => {
                if tag == 0x8 {
                    let id = usize::from(g.id);
                    // Constant (bit 0) fields are padding, arrays (bit 1
                    // clear) a keyboard's: neither is a pointer's.
                    if u & 1 == 0 && u & 2 != 0 {
                        for k in 0..g.count {
                            let usage = if let Some((lo, hi)) = range {
                                lo.saturating_add(k).min(hi)
                            } else if n_usages > 0 {
                                usages[(k as usize).min(n_usages - 1)]
                            } else {
                                continue;
                            };
                            // A 4-byte usage names its page in the high half.
                            let page = if usage > 0xFFFF { usage >> 16 } else { g.page };
                            let bit = bits[id].saturating_add(k.saturating_mul(g.size));
                            let f = Field {
                                bit: u16::try_from(bit).ok()?,
                                size: g.size.min(32) as u8,
                                min: g.min,
                                max: g.max,
                                relative: u & 4 != 0,
                            };
                            field(g.id, page, usage & 0xFFFF, f);
                        }
                    }
                    bits[id] = bits[id].saturating_add(g.count.saturating_mul(g.size));
                }
                n_usages = 0;
                range = None;
            }
            // Global.
            1 => match tag {
                0x0 => g.page = u,
                0x1 => g.min = s,
                // A logical maximum is unsigned when the minimum is not
                // negative (0..255 in one byte).
                0x2 => g.max = if g.min >= 0 { u as i32 } else { s },
                0x7 => g.size = u,
                0x8 => g.id = u as u8,
                0x9 => g.count = u,
                0xA if depth < stack.len() => {
                    stack[depth] = g;
                    depth += 1;
                }
                0xB if depth > 0 => {
                    depth -= 1;
                    g = stack[depth];
                }
                _ => {}
            },
            // Local.
            2 => match tag {
                0x0 if n_usages < usages.len() => {
                    usages[n_usages] = if len == 4 { u } else { u & 0xFFFF };
                    n_usages += 1;
                }
                0x1 => range = Some((u, range.map_or(u, |r| r.1))),
                0x2 => range = Some((range.map_or(0, |r| r.0), u)),
                _ => {}
            },
            _ => {}
        }
    }
    Some(())
}

/// The pointer in report descriptor `d`: the report with the first X axis
/// of the generic desktop page, which has a Y axis too. `None` when there
/// is none (a keyboard) or the descriptor is malformed.
pub fn parse(d: &[u8]) -> Option<Layout> {
    let mut id = None;
    walk(d, |rid, page, usage, _| {
        if id.is_none() && page == PAGE_DESKTOP && usage == USAGE_X {
            id = Some(rid);
        }
    })?;
    let id = id?;
    let mut l = Layout { id, ..Layout::default() };
    let (mut x, mut y) = (false, false);
    walk(d, |rid, page, usage, f| {
        if rid != id {
            return;
        }
        match (page, usage) {
            (PAGE_DESKTOP, USAGE_X) if !x => {
                l.x = f;
                x = true;
            }
            (PAGE_DESKTOP, USAGE_Y) if !y => {
                l.y = f;
                y = true;
            }
            (PAGE_DESKTOP, USAGE_WHEEL) if l.wheel.is_none() => l.wheel = Some(f),
            (PAGE_BUTTON, 1..=5) => l.buttons[usage as usize - 1] = Some(f),
            _ => {}
        }
    })?;
    y.then_some(l)
}
