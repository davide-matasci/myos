//! Limine boot protocol requests. Shared by x86_64 and aarch64.

use limine::memmap;
use limine::request::{
    DtbRequest, ExecutableAddressRequest, FramebufferRequest, HhdmRequest, MemmapRequest,
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
