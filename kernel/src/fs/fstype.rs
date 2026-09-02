//! Registered filesystem types. `mount(2)` looks up a name and binds a blk id.

use myos_abi::{FsBind, ModuleVfsOps};
use spin::Mutex;

use super::vfs::MountOps;

/// In-kernel fstype bind: parse the device and return `MountOps`.
pub type KernelBind = fn(u32) -> Option<MountOps>;

const MAX: usize = 8;
const NAME_CAP: usize = 16;

struct Ent {
    name: [u8; NAME_CAP],
    name_len: u8,
    bind: FsBind,
}

static TYPES: Mutex<[Option<Ent>; MAX]> = Mutex::new([const { None }; MAX]);

struct KernelEnt {
    name: [u8; NAME_CAP],
    name_len: u8,
    bind: KernelBind,
}

static KERNEL_TYPES: Mutex<[Option<KernelEnt>; MAX]> = Mutex::new([const { None }; MAX]);

pub fn register_kernel(name: &str, bind: KernelBind) -> bool {
    if name.is_empty() || name.len() > NAME_CAP {
        return false;
    }
    let mut n = [0u8; NAME_CAP];
    n[..name.len()].copy_from_slice(name.as_bytes());
    let mut table = KERNEL_TYPES.lock();
    for slot in table.iter() {
        if let Some(e) = slot {
            if e.name_len as usize == name.len() && &e.name[..name.len()] == name.as_bytes() {
                return false;
            }
        }
    }
    for slot in table.iter_mut() {
        if slot.is_none() {
            *slot = Some(KernelEnt {
                name: n,
                name_len: name.len() as u8,
                bind,
            });
            return true;
        }
    }
    false
}

pub fn bind_kernel(name: &str, dev: u32) -> Option<MountOps> {
    let bind_fn = {
        let table = KERNEL_TYPES.lock();
        let mut found = None;
        for slot in table.iter().flatten() {
            let n = slot.name_len as usize;
            if n == name.len() && &slot.name[..n] == name.as_bytes() {
                found = Some(slot.bind);
                break;
            }
        }
        found?
    };
    bind_fn(dev)
}

pub fn register(name: &str, bind: FsBind) -> bool {
    if name.is_empty() || name.len() > NAME_CAP {
        return false;
    }
    let mut n = [0u8; NAME_CAP];
    n[..name.len()].copy_from_slice(name.as_bytes());
    let mut table = TYPES.lock();
    for slot in table.iter() {
        if let Some(e) = slot {
            if e.name_len as usize == name.len() && &e.name[..name.len()] == name.as_bytes() {
                return false;
            }
        }
    }
    for slot in table.iter_mut() {
        if slot.is_none() {
            *slot = Some(Ent {
                name: n,
                name_len: name.len() as u8,
                bind,
            });
            return true;
        }
    }
    false
}

pub fn bind(name: &str, dev: u32) -> Option<ModuleVfsOps> {
    let bind_fn = {
        let table = TYPES.lock();
        let mut found = None;
        for slot in table.iter().flatten() {
            let n = slot.name_len as usize;
            if n == name.len() && &slot.name[..n] == name.as_bytes() {
                found = Some(slot.bind);
                break;
            }
        }
        found?
    };
    // Function pointers must not be zero-initialized (invalid bit pattern).
    let mut ops = core::mem::MaybeUninit::<ModuleVfsOps>::uninit();
    let rc = unsafe { (bind_fn)(dev, ops.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: successful bind fills every ModuleVfsOps field.
    let ops = unsafe { ops.assume_init() };
    if ops.lookup as usize == 0 {
        None
    } else {
        Some(ops)
    }
}
