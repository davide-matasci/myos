//! The service registry: a module publishes a function table under a name
//! (`KernelApi::service_register`), another finds it
//! (`KernelApi::service_lookup`). How a bus module and its class drivers
//! reach each other without linking: the `xhci` host publishes
//! `myos_abi::USB_SERVICE`, `usb_hub` and `usb_storage` drive devices
//! through it (`docs/usb.md`). The kernel knows nothing of a table's
//! layout; the two sides share it through `modules/abi`.

use spin::Mutex;

const MAX_SERVICES: usize = 8;
const NAME_MAX: usize = 31;

#[derive(Clone, Copy)]
struct Service {
    name: [u8; NAME_MAX],
    name_len: u8,
    table: usize,
}

static SERVICES: Mutex<[Option<Service>; MAX_SERVICES]> = Mutex::new([None; MAX_SERVICES]);

impl Service {
    fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }
}

/// Publish `table` under `name`: false when the name is taken or invalid,
/// or the registry is full.
pub fn register(name: &str, table: usize) -> bool {
    if name.is_empty() || name.len() > NAME_MAX || table == 0 {
        return false;
    }
    let mut services = SERVICES.lock();
    if services.iter().flatten().any(|s| s.name_str() == name) {
        return false;
    }
    let Some(slot) = services.iter().position(|s| s.is_none()) else {
        return false;
    };
    let mut s = Service { name: [0; NAME_MAX], name_len: name.len() as u8, table };
    s.name[..name.len()].copy_from_slice(name.as_bytes());
    services[slot] = Some(s);
    true
}

/// The table published under `name`.
pub fn lookup(name: &str) -> Option<usize> {
    SERVICES.lock().iter().flatten().find(|s| s.name_str() == name).map(|s| s.table)
}
