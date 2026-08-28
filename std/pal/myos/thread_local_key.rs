//! Single-threaded TLS stub (enough for `std` init on a one-task process).

use crate::ptr::addr_of_mut;

pub struct Key {
    active: bool,
}

impl Key {
    pub const fn new(_dtor: Option<unsafe extern "C" fn(*mut u8)>) -> Key {
        Key { active: false }
    }

    pub unsafe fn get(&self) -> *mut u8 {
        core::ptr::null_mut()
    }

    pub unsafe fn set(&mut self, _value: *mut u8) {
        self.active = true;
    }
}

pub unsafe fn create(_dtor: Option<unsafe extern "C" fn(*mut u8)>) -> Key {
    Key::new(_dtor)
}

pub unsafe fn set(_key: *mut Key, _value: *mut u8) {}

pub unsafe fn get(_key: *mut Key) -> *mut u8 {
    core::ptr::null_mut()
}

pub unsafe fn destroy(_key: *mut Key) {}

static mut MAIN_TLS: u8 = 0;

pub fn main_tls() -> *mut u8 {
    unsafe { addr_of_mut!(MAIN_TLS) }
}
