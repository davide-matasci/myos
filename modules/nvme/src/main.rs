//! NVMe: `/dev/nvmeXn1` through [`myos_abi::KernelApi::blk_register`].
//!
//! Speaks only through the module ABI. One admin SQ/CQ and one I/O SQ/CQ
//! per controller, completion by polling the CQ phase bit; no MSI/MSI-X.
//! Controllers are found by PCI class (`0x01` / `0x08`), their BAR0 mapped
//! by the kernel. Cap [`MAX_CTRL`] controllers, NSID 1 each.

#![no_std]
#![no_main]

use core::sync::atomic::{Ordering, compiler_fence};

use myos_abi::{ABI_VERSION, KernelApi, ModuleBlkOps, status_ok};

const SECTOR: usize = 512;
/// The DMA buffer: one page, what a command's PRP1 covers.
const PAGE: usize = 4096;
pub const MAX_CTRL: usize = 4;
const ADMIN_QD: u16 = 16;
const IO_QD: u16 = 16;
const SQE_SIZE: usize = 64;
const CQE_SIZE: usize = 16;

const REG_CAP: u32 = 0x00;
const REG_CC: u32 = 0x14;
const REG_CSTS: u32 = 0x1C;
const REG_AQA: u32 = 0x24;
const REG_ASQ: u32 = 0x28;
const REG_ACQ: u32 = 0x30;
const REG_INTMS: u32 = 0x0C;

const CC_EN: u32 = 1;
const CC_IOSQES: u32 = 6 << 16;
const CC_IOCQES: u32 = 4 << 20;
const CSTS_RDY: u32 = 1;

const OPC_CREATE_SQ: u8 = 0x01;
const OPC_CREATE_CQ: u8 = 0x05;
const OPC_IDENTIFY: u8 = 0x06;
const OPC_IO_WRITE: u8 = 0x01;
const OPC_IO_READ: u8 = 0x02;

const CNS_NS: u32 = 0;
const CNS_CTRL: u32 = 1;

struct Ctrl {
    bar: usize,
    dstrd: u32,
    admin_sq: *mut u8,
    admin_cq: *mut u8,
    io_sq: *mut u8,
    io_cq: *mut u8,
    io_sq_phys: u64,
    io_cq_phys: u64,
    dma_va: *mut u8,
    dma_phys: u64,
    admin_sq_tail: u16,
    admin_cq_head: u16,
    admin_cq_phase: u16,
    io_sq_tail: u16,
    io_cq_head: u16,
    io_cq_phase: u16,
    next_cid: u16,
    capacity: u64,
}

static mut CTRLS: [Option<Ctrl>; MAX_CTRL] = [const { None }; MAX_CTRL];
static mut API: Option<&'static KernelApi> = None;

static OPS: ModuleBlkOps = ModuleBlkOps {
    read: blk_read,
    write: blk_write,
    capacity_sectors: blk_capacity,
};

fn api() -> &'static KernelApi {
    unsafe { (*core::ptr::addr_of!(API)).expect("nvme: API") }
}

fn dma_page() -> Option<(u64, *mut u8)> {
    let mut phys = 0u64;
    let va = unsafe { (api().dma_alloc)(1, &mut phys) };
    if va.is_null() { None } else { Some((phys, va)) }
}

fn ctrl(i: usize) -> Option<&'static mut Ctrl> {
    unsafe { (*core::ptr::addr_of_mut!(CTRLS)).get_mut(i)?.as_mut() }
}

fn r32(bar: usize, off: u32) -> u32 {
    unsafe { core::ptr::read_volatile((bar + off as usize) as *const u32) }
}
fn w32(bar: usize, off: u32, v: u32) {
    unsafe { core::ptr::write_volatile((bar + off as usize) as *mut u32, v) }
}
fn r64(bar: usize, off: u32) -> u64 {
    let lo = r32(bar, off) as u64;
    let hi = r32(bar, off + 4) as u64;
    lo | (hi << 32)
}
fn w64(bar: usize, off: u32, v: u64) {
    w32(bar, off, v as u32);
    w32(bar, off + 4, (v >> 32) as u32);
}


fn dma_wmb() {
    compiler_fence(Ordering::Release);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence rw,rw", options(nostack, preserves_flags));
    }
}

fn dma_rmb() {
    compiler_fence(Ordering::SeqCst);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence rw,rw", options(nostack, preserves_flags));
    }
}

fn dcache_civac(va: *mut u8, len: usize) {
    let _ = (va, len);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        if len == 0 {
            return;
        }
        let mut addr = va as usize & !63;
        let end = va as usize + len;
        while addr < end {
            core::arch::asm!("dc civac, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb sy", options(nostack));
    }
}

fn doorbell_sq(bar: usize, qid: u16, dstrd: u32, tail: u16) {
    let off = 0x1000 + (2 * qid as u32) * (4 << dstrd);
    w32(bar, off, u32::from(tail));
}

fn doorbell_cq(bar: usize, qid: u16, dstrd: u32, head: u16) {
    let off = 0x1000 + (2 * qid as u32 + 1) * (4 << dstrd);
    w32(bar, off, u32::from(head));
}

fn wait_csts(bar: usize, want_rdy: bool) -> bool {
    for _ in 0..50_000_000u32 {
        dma_rmb();
        let rdy = r32(bar, REG_CSTS) & CSTS_RDY != 0;
        if rdy == want_rdy {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

unsafe fn write_sqe(sq: *mut u8, slot: u16, words: &[u32; 16]) {
    let p = unsafe { sq.add(slot as usize * SQE_SIZE) };
    for i in 0..16 {
        unsafe {
            core::ptr::write_volatile(p.add(i * 4) as *mut u32, words[i]);
        }
    }
}

fn poll_cq(
    bar: usize,
    dstrd: u32,
    cq: *mut u8,
    head: &mut u16,
    phase: &mut u16,
    qd: u16,
    qid: u16,
) -> Result<u32, ()> {
    for _ in 0..50_000_000u32 {
        dma_rmb();
        dcache_civac(cq, qd as usize * CQE_SIZE);
        let off = *head as usize * CQE_SIZE;
        let dw3 = unsafe { core::ptr::read_volatile(cq.add(off + 12) as *const u32) };
        if ((dw3 >> 16) & 1) == u32::from(*phase) {
            let dw0 = unsafe { core::ptr::read_volatile(cq.add(off) as *const u32) };
            let status = (dw3 >> 17) & 0x7FF;
            *head = (*head + 1) % qd;
            if *head == 0 {
                *phase ^= 1;
            }
            dma_wmb();
            doorbell_cq(bar, qid, dstrd, *head);
            if status != 0 {
                return Err(());
            }
            return Ok(dw0);
        }
        core::hint::spin_loop();
    }
    Err(())
}

fn submit(
    bar: usize,
    dstrd: u32,
    sq: *mut u8,
    cq: *mut u8,
    sq_tail: &mut u16,
    cq_head: &mut u16,
    cq_phase: &mut u16,
    qd: u16,
    qid: u16,
    words: &[u32; 16],
) -> Result<u32, ()> {
    let slot = *sq_tail;
    unsafe {
        write_sqe(sq, slot, words);
    }
    dcache_civac(unsafe { sq.add(slot as usize * SQE_SIZE) }, SQE_SIZE);
    dma_wmb();
    *sq_tail = (*sq_tail + 1) % qd;
    doorbell_sq(bar, qid, dstrd, *sq_tail);
    poll_cq(bar, dstrd, cq, cq_head, cq_phase, qd, qid)
}

fn cid(c: &mut Ctrl) -> u16 {
    let id = c.next_cid;
    c.next_cid = c.next_cid.wrapping_add(1);
    if c.next_cid == 0 {
        c.next_cid = 1;
    }
    id
}

fn admin(c: &mut Ctrl, mut words: [u32; 16]) -> Result<u32, ()> {
    words[0] |= u32::from(cid(c)) << 16;
    submit(
        c.bar,
        c.dstrd,
        c.admin_sq,
        c.admin_cq,
        &mut c.admin_sq_tail,
        &mut c.admin_cq_head,
        &mut c.admin_cq_phase,
        ADMIN_QD,
        0,
        &words,
    )
}

fn io(c: &mut Ctrl, mut words: [u32; 16]) -> Result<u32, ()> {
    words[0] |= u32::from(cid(c)) << 16;
    submit(
        c.bar,
        c.dstrd,
        c.io_sq,
        c.io_cq,
        &mut c.io_sq_tail,
        &mut c.io_cq_head,
        &mut c.io_cq_phase,
        IO_QD,
        1,
        &words,
    )
}

fn disable(bar: usize) -> bool {
    let cc = r32(bar, REG_CC) & !CC_EN;
    w32(bar, REG_CC, cc);
    wait_csts(bar, false)
}

fn setup(bar: usize) -> Option<Ctrl> {
    w32(bar, REG_INTMS, 0xFFFF_FFFF);
    if !disable(bar) {
        return None;
    }

    let cap = r64(bar, REG_CAP);
    let dstrd = ((cap >> 32) & 0xF) as u32;
    let mqes = (cap as u16).wrapping_add(1);
    if mqes < ADMIN_QD || mqes < IO_QD {
        return None;
    }
    let mpsmin = ((cap >> 48) & 0xF) as u32;
    if mpsmin != 0 {
        return None;
    }

    let (admin_sq_phys, admin_sq) = dma_page()?;
    let (admin_cq_phys, admin_cq) = dma_page()?;
    let (io_sq_phys, io_sq) = dma_page()?;
    let (io_cq_phys, io_cq) = dma_page()?;
    let (dma_phys, dma_va) = dma_page()?;

    w32(
        bar,
        REG_AQA,
        u32::from(ADMIN_QD - 1) | (u32::from(ADMIN_QD - 1) << 16),
    );
    w64(bar, REG_ASQ, admin_sq_phys);
    w64(bar, REG_ACQ, admin_cq_phys);

    dma_wmb();
    w32(bar, REG_CC, CC_EN | CC_IOSQES | CC_IOCQES);
    w32(bar, REG_INTMS, 0xFFFF_FFFF);
    if !wait_csts(bar, true) {
        return None;
    }

    let mut c = Ctrl {
        bar,
        dstrd,
        admin_sq,
        admin_cq,
        io_sq,
        io_cq,
        io_sq_phys,
        io_cq_phys,
        dma_va,
        dma_phys,
        admin_sq_tail: 0,
        admin_cq_head: 0,
        admin_cq_phase: 1,
        io_sq_tail: 0,
        io_cq_head: 0,
        io_cq_phase: 1,
        next_cid: 1,
        capacity: 0,
    };

    let mut id = [0u32; 16];
    id[0] = u32::from(OPC_IDENTIFY);
    id[6] = c.dma_phys as u32;
    id[7] = (c.dma_phys >> 32) as u32;
    id[10] = CNS_CTRL;
    dcache_civac(c.dma_va, 4096);
    admin(&mut c, id).ok()?;

    let mut idn = [0u32; 16];
    idn[0] = u32::from(OPC_IDENTIFY);
    idn[1] = 1;
    idn[6] = c.dma_phys as u32;
    idn[7] = (c.dma_phys >> 32) as u32;
    idn[10] = CNS_NS;
    dcache_civac(c.dma_va, 4096);
    admin(&mut c, idn).ok()?;
    dcache_civac(c.dma_va, 4096);
    let nsze = unsafe { core::ptr::read_unaligned(c.dma_va as *const u64) };
    if nsze == 0 {
        return None;
    }
    let flbas = unsafe { core::ptr::read_volatile(c.dma_va.add(26)) } & 0xF;
    let lbaf_off = 128 + flbas as usize * 4;
    let lbaf = unsafe { core::ptr::read_unaligned(c.dma_va.add(lbaf_off) as *const u32) };
    let lbads = (lbaf >> 16) & 0xFF;
    if lbads != 9 {
        return None;
    }
    // NSZE is namespace size in LBAs (LBA 0 through n-1).
    c.capacity = nsze;

    let mut ccq = [0u32; 16];
    ccq[0] = u32::from(OPC_CREATE_CQ);
    ccq[6] = c.io_cq_phys as u32;
    ccq[7] = (c.io_cq_phys >> 32) as u32;
    ccq[10] = 1 | (u32::from(IO_QD - 1) << 16);
    ccq[11] = 1;
    admin(&mut c, ccq).ok()?;

    let mut csq = [0u32; 16];
    csq[0] = u32::from(OPC_CREATE_SQ);
    csq[6] = c.io_sq_phys as u32;
    csq[7] = (c.io_sq_phys >> 32) as u32;
    csq[10] = 1 | (u32::from(IO_QD - 1) << 16);
    csq[11] = 1 | (1 << 16);
    admin(&mut c, csq).ok()?;

    Some(c)
}


/// `bar_va` is a controller's BAR0 (not yet attached). True once it is
/// initialised and in the table.
/// Bring up the controller at `bar_va` into a free slot; its index, or
/// `None` when it is already attached, fails to come up or the table is full.
fn attach(bar_va: usize) -> Option<usize> {
    let table = unsafe { &mut *core::ptr::addr_of_mut!(CTRLS) };
    if table.iter().flatten().any(|c| c.bar == bar_va) {
        return None;
    }
    let c = setup(bar_va)?;
    let slot = table.iter().position(|s| s.is_none())?;
    table[slot] = Some(c);
    Some(slot)
}

/// Every NVMe controller on the bus: the new ones come up and register as
/// `/dev/nvme<slot>n1`, the ones already attached are left alone. The
/// number of controllers attached by this call.
fn probe(api: &KernelApi) -> usize {
    let mut new = 0;
    for i in 0..MAX_CTRL as u32 {
        let (mut bus, mut slot, mut func) = (0u8, 0u8, 0u8);
        if unsafe { (api.pci_find_class)(CLASS_MASS, SUBCLASS_NVME, i, &mut bus, &mut slot, &mut func) } != 0 {
            break;
        }
        unsafe { (api.pci_enable)(bus, slot, func) };
        let (mut va, mut size) = (0usize, 0u64);
        if unsafe { (api.pci_bar_map)(bus, slot, func, 0, &mut va, &mut size) } != 0 {
            continue;
        }
        let Some(n) = attach(va) else {
            continue;
        };
        // nvme{n}n1 (n < 10: MAX_CTRL is 4).
        let name = [b'n', b'v', b'm', b'e', b'0' + n as u8, b'n', b'1'];
        if unsafe { (api.blk_register)(name.as_ptr(), name.len(), &OPS, n) } < 0 {
            break;
        }
        new += 1;
    }
    new
}

/// One read or write command: `buf` (whole sectors, at most the DMA
/// page) through the controller's DMA page.
fn xfer(c: &mut Ctrl, lba: u64, buf: &mut [u8], is_write: bool) -> Result<(), ()> {
    let n = buf.len();
    if is_write {
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), c.dma_va, n);
        }
    }
    dcache_civac(c.dma_va, n);
    let opc = if is_write { OPC_IO_WRITE } else { OPC_IO_READ };
    let mut cmd = [0u32; 16];
    cmd[0] = u32::from(opc);
    cmd[1] = 1;
    cmd[6] = c.dma_phys as u32;
    cmd[7] = (c.dma_phys >> 32) as u32;
    cmd[10] = lba as u32;
    cmd[11] = (lba >> 32) as u32;
    cmd[12] = (n / SECTOR - 1) as u32; // 0-based block count
    io(c, cmd)?;
    if !is_write {
        dcache_civac(c.dma_va, n);
        unsafe {
            core::ptr::copy_nonoverlapping(c.dma_va, buf.as_mut_ptr(), n);
        }
    }
    Ok(())
}

/// Move `buf` page by page (a page is what one command's PRP1 covers).
fn rw(c: &mut Ctrl, lba: u64, buf: &mut [u8], is_write: bool) -> i32 {
    let mut lba = lba;
    for chunk in buf.chunks_mut(PAGE) {
        if xfer(c, lba, chunk, is_write).is_err() {
            return -1;
        }
        lba += (chunk.len() / SECTOR) as u64;
    }
    0
}

unsafe extern "C" fn blk_read(ctx: usize, lba: u64, buf: *mut u8, len: usize) -> i32 {
    if buf.is_null() || len % SECTOR != 0 {
        return -1;
    }
    let Some(c) = ctrl(ctx) else {
        return -1;
    };
    rw(c, lba, unsafe { core::slice::from_raw_parts_mut(buf, len) }, false)
}

unsafe extern "C" fn blk_write(ctx: usize, lba: u64, buf: *const u8, len: usize) -> i32 {
    if buf.is_null() || len % SECTOR != 0 {
        return -1;
    }
    let Some(c) = ctrl(ctx) else {
        return -1;
    };
    // `rw` only reads from `buf` when writing.
    rw(c, lba, unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len) }, true)
}

unsafe extern "C" fn blk_capacity(ctx: usize) -> u64 {
    ctrl(ctx).map_or(0, |c| c.capacity)
}

const CLASS_MASS: u8 = 0x01;
const SUBCLASS_NVME: u8 = 0x08;

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api_ptr: *const KernelApi) -> i32 {
    if api_ptr.is_null() {
        return -1;
    }
    let api: &'static KernelApi = unsafe { &*api_ptr };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe {
        *core::ptr::addr_of_mut!(API) = Some(api);
    }
    if probe(api) == 0 {
        unsafe { (api.write_str)(b"nvme none\n".as_ptr(), 10) };
    } else {
        status_ok(api, "nvme");
    }
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

/// A `rescan` of `/proc/pci`: controllers that appeared since init.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_rescan() {
    probe(api());
}

// So `cargo build --bin nvme` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
