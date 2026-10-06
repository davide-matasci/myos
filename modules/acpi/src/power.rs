//! The ACPI power methods (`docs/power.md`), registered with the kernel's
//! power sequence: power-off by entering S5 (the `_S5` sleep types written
//! to the FADT's PM1 control registers) and reboot through the FADT's reset
//! register. Both are system I/O ports on a PC; an ACPI register in memory
//! (a hardware-reduced platform: aarch64 under EDK2) is left to the arch's
//! own methods (PSCI).

use core::sync::atomic::{AtomicU32, Ordering};

use myos_abi::{KernelApi, MYOS_POWER_OFF, MYOS_POWER_REBOOT};

/// What the methods need from the FADT, as ports (0: none).
static PM1A_CNT: AtomicU32 = AtomicU32::new(0);
static PM1B_CNT: AtomicU32 = AtomicU32::new(0);
/// `SLP_TYPa` and `SLP_TYPb` of `_S5`.
static SLP_TYP: AtomicU32 = AtomicU32::new(0);
/// The SMI command port and the value that hands ACPI to the OS.
static SMI_CMD: AtomicU32 = AtomicU32::new(0);
static ACPI_ENABLE: AtomicU32 = AtomicU32::new(0);
/// The reset register's port and value.
static RESET_PORT: AtomicU32 = AtomicU32::new(0);
static RESET_VALUE: AtomicU32 = AtomicU32::new(0);

const SLP_EN: u16 = 1 << 13;
const SCI_EN: u16 = 1;
/// The FADT flags: the reset register is there; the platform has no PM1
/// registers.
const RESET_REG_SUP: u32 = 1 << 10;
const HW_REDUCED_ACPI: u32 = 1 << 20;
/// Generic address structure space: system I/O.
const SPACE_IO: u8 = 1;

/// Read the FADT's registers and register the methods it allows. `s5` is
/// `_S5`'s `(SLP_TYPa, SLP_TYPb)` when the DSDT has it.
pub fn register(api: &KernelApi, fadt: &[u8], s5: Option<(u8, u8)>) {
    let u32_at = |off: usize| fadt.get(off..off + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let flags = u32_at(112).unwrap_or(0);
    if !cfg!(target_arch = "x86_64") || flags & HW_REDUCED_ACPI != 0 {
        return;
    }
    if let (Some((typa, typb)), Some(pm1a)) = (s5, io_reg(fadt, 172, 64)) {
        PM1A_CNT.store(pm1a, Ordering::Relaxed);
        PM1B_CNT.store(io_reg(fadt, 184, 68).unwrap_or(0), Ordering::Relaxed);
        SLP_TYP.store(u32::from(typa) | u32::from(typb) << 8, Ordering::Relaxed);
        SMI_CMD.store(u32_at(48).unwrap_or(0), Ordering::Relaxed);
        ACPI_ENABLE.store(u32::from(fadt.get(52).copied().unwrap_or(0)), Ordering::Relaxed);
        // SAFETY: `power_off` is a function of this module, which the
        // registration keeps loaded.
        let _ = unsafe { api.power_register(MYOS_POWER_OFF, "acpi s5", power_off) };
    }
    // The reset register (ACPI 2.0): a generic address at 116, its value at 128.
    if flags & RESET_REG_SUP != 0 && fadt.len() >= 129 && fadt[116] == SPACE_IO {
        let port = u64::from_le_bytes(fadt[120..128].try_into().unwrap_or([0; 8]));
        if port != 0 && port <= 0xFFFF {
            RESET_PORT.store(port as u32, Ordering::Relaxed);
            RESET_VALUE.store(u32::from(fadt[128]), Ordering::Relaxed);
            // SAFETY: as for `power_off`.
            let _ = unsafe { api.power_register(MYOS_POWER_REBOOT, "acpi reset register", reset) };
        }
    }
}

/// A PM1 control register's port: the 64-bit generic address at `x_off`
/// when it is a port, else the 32-bit block address at `off`.
fn io_reg(fadt: &[u8], x_off: usize, off: usize) -> Option<u32> {
    if let Some(gas) = fadt.get(x_off..x_off + 12) {
        let addr = u64::from_le_bytes(gas[4..12].try_into().ok()?);
        if addr != 0 {
            return (gas[0] == SPACE_IO && addr <= 0xFFFF).then_some(addr as u32);
        }
    }
    let port = u32::from_le_bytes(fadt.get(off..off + 4)?.try_into().ok()?);
    (port != 0 && port <= 0xFFFF).then_some(port)
}

/// Enter S5: hand ACPI to the OS if the firmware still has it (`SCI_EN`
/// clear), then write `SLP_TYPx | SLP_EN` to each PM1 control register.
unsafe extern "C" fn power_off() {
    let pm1a = PM1A_CNT.load(Ordering::Relaxed) as u16;
    let pm1b = PM1B_CNT.load(Ordering::Relaxed) as u16;
    let typ = SLP_TYP.load(Ordering::Relaxed);
    let smi_cmd = SMI_CMD.load(Ordering::Relaxed) as u16;
    let enable = ACPI_ENABLE.load(Ordering::Relaxed) as u8;
    if io::inw(pm1a) & SCI_EN == 0 && smi_cmd != 0 && enable != 0 {
        io::outb(smi_cmd, enable);
        for _ in 0..1_000_000 {
            if io::inw(pm1a) & SCI_EN != 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }
    let sleep = |port: u16, slp_typ: u32| {
        let v = io::inw(port) & !(7 << 10);
        io::outw(port, v | ((slp_typ as u16 & 7) << 10) | SLP_EN);
    };
    sleep(pm1a, typ & 0xFF);
    if pm1b != 0 {
        sleep(pm1b, typ >> 8);
    }
}

unsafe extern "C" fn reset() {
    io::outb(RESET_PORT.load(Ordering::Relaxed) as u16, RESET_VALUE.load(Ordering::Relaxed) as u8);
}

#[cfg(target_arch = "x86_64")]
mod io {
    pub fn inw(port: u16) -> u16 {
        let value: u16;
        unsafe {
            core::arch::asm!("in ax, dx", in("dx") port, out("ax") value, options(nomem, nostack, preserves_flags));
        }
        value
    }

    pub fn outw(port: u16, value: u16) {
        unsafe {
            core::arch::asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
        }
    }

    pub fn outb(port: u16, value: u8) {
        unsafe {
            core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
        }
    }
}

/// No I/O ports: `register` registers nothing (only a PC's ACPI has them).
#[cfg(not(target_arch = "x86_64"))]
mod io {
    pub fn inw(_port: u16) -> u16 {
        0
    }

    pub fn outw(_port: u16, _value: u16) {}

    pub fn outb(_port: u16, _value: u8) {}
}
