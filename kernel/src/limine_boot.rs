//! Limine boot protocol requests. Shared by x86_64, aarch64, and riscv64.

use limine::memmap;
use limine::request::{
    DtbRequest, ExecutableAddressRequest, ExecutableCmdlineRequest, ExecutableFileRequest, FramebufferRequest,
    HhdmRequest, MemmapRequest, ModulesRequest, MpRequest, RsdpRequest,
};
use limine::{BaseRevision, RequestsEndMarker, RequestsStartMarker};

#[used]
#[unsafe(link_section = ".limine_requests_start")]
static START: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static HHDM: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static MEMMAP: MemmapRequest = MemmapRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static EXECUTABLE_ADDRESS: ExecutableAddressRequest = ExecutableAddressRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static FRAMEBUFFER: FramebufferRequest = FramebufferRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static DTB: DtbRequest = DtbRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static MODULES: ModulesRequest = ModulesRequest::new();

/// Multi-processor: Limine parks APs until `MpInfo::bootstrap`.
#[used]
#[unsafe(link_section = ".limine_requests")]
pub static MP: MpRequest = MpRequest::new(0);

/// ACPI RSDP (virtual address under base revision ≥ 4 / current MAX).
#[used]
#[unsafe(link_section = ".limine_requests")]
pub static RSDP: RsdpRequest = RsdpRequest::new();

/// The kernel's command line (`cmdline:` in limine.conf): `platform=…`.
#[used]
#[unsafe(link_section = ".limine_requests")]
pub static CMDLINE: ExecutableCmdlineRequest = ExecutableCmdlineRequest::new();

/// The kernel's own file as Limine loaded it (`/proc/boot/kernel`).
#[used]
#[unsafe(link_section = ".limine_requests")]
pub static EXECUTABLE_FILE: ExecutableFileRequest = ExecutableFileRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests_end")]
static END: RequestsEndMarker = RequestsEndMarker::new();

pub fn base_revision_supported() -> bool {
    BASE_REVISION.is_supported()
}

pub fn hhdm_offset() -> u64 {
    HHDM.response().expect("Limine HHDM").offset
}

/// Translate a kernel virtual address to physical using Limine's uniform slide.
pub fn kernel_virt_to_phys(va: usize) -> u64 {
    let r = EXECUTABLE_ADDRESS
        .response()
        .expect("Limine executable address");
    (va as u64) - r.virtual_base + r.physical_base
}

/// Length of the largest usable memory region.
pub fn largest_usable() -> u64 {
    let entries = MEMMAP.response().expect("Limine memmap").entries();
    entries
        .iter()
        .filter(|e| e.type_ == memmap::MEMMAP_USABLE)
        .map(|e| e.length)
        .max()
        .unwrap_or(0)
}

/// Allocate `size` bytes from a usable memmap region and return the HHDM VA.
///
/// HHDM mappings are rwx, so the heap can hold runtime modules.
pub fn alloc_usable(size: usize) -> usize {
    let hhdm = hhdm_offset();
    let entries = MEMMAP.response().expect("Limine memmap").entries();
    const SKIP: u64 = 64 * 1024;
    let need = size as u64 + SKIP;
    for e in entries {
        if e.type_ != memmap::MEMMAP_USABLE {
            continue;
        }
        if e.length >= need {
            let phys = (e.base + SKIP + 0xfff) & !0xfff;
            return (phys + hhdm) as usize;
        }
    }
    panic!("no usable Limine memory for heap");
}

/// The kernel command line, empty without one.
pub fn cmdline() -> &'static str {
    CMDLINE.response().map_or("", |r| r.cmdline())
}

/// The files this boot came from, as Limine loaded them (`/proc/boot/`,
/// what `get-myos --install --local` copies onto a disk): `kernel`, the
/// kernel's file, and `initramfs`, the module of that name. Limine keeps
/// both mapped for the life of the kernel.
pub fn boot_file(name: &str) -> Option<&'static [u8]> {
    let file = match name {
        "kernel" => EXECUTABLE_FILE.response()?.executable_file(),
        "initramfs" => MODULES
            .response()?
            .modules()
            .iter()
            .find(|f| f.path().rsplit('/').next() == Some("initramfs"))?,
        _ => return None,
    };
    let data = file.data();
    Some(unsafe { core::slice::from_raw_parts(data.as_ptr(), data.len()) })
}

/// The GPT partition the kernel's file was loaded from (the boot disk's
/// ESP), its unique GUID as `/proc/partitions` writes it: `/proc/boot/partuuid`,
/// what `mount -a` mounts at `/boot`. `None` from a medium without a GPT
/// (the ISO).
pub fn boot_partuuid() -> Option<alloc::string::String> {
    let file = EXECUTABLE_FILE.response()?.executable_file();
    // Read at the offset of the protocol's `struct limine_file`: the crate's
    // `File` lacks the `unused` word after `media_type`, which puts its
    // GUID fields 4 bytes early. The GUID's bytes are as the GPT has them.
    const GPT_PART_UUID: usize = 80;
    let base = file as *const limine::file::File as *const u8;
    let mut g = [0u8; 16];
    // SAFETY: Limine's file structure is at least 112 bytes long (it ends
    // with three GUIDs), and stays mapped for the kernel's life.
    unsafe { core::ptr::copy_nonoverlapping(base.add(GPT_PART_UUID), g.as_mut_ptr(), 16) };
    if g == [0; 16] {
        return None;
    }
    Some(crate::blk::gpt::guid_text(&g))
}

/// Limine RSDP virtual address, or `None` when firmware has no ACPI.
pub fn rsdp_va() -> Option<usize> {
    let resp = RSDP.response()?;
    let p = resp.address as usize;
    if p == 0 {
        None
    } else {
        Some(p)
    }
}
