//! Frame allocator + offset mapper on top of bootloader 0.11's page tables.
//!
//! The bootloader already turned paging on and identity/offset-mapped the
//! kernel, framebuffer, and physical memory. We walk its memory map for free
//! frames and reuse `physical_memory_offset` so we can map extra pages (heap).

use bootloader_api::info::{MemoryRegion, MemoryRegionKind, MemoryRegions};
use bootloader_api::BootInfo;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

static mut MAPPER: Option<OffsetPageTable<'static>> = None;
static mut FRAME_ALLOC: Option<BootInfoFrameAllocator> = None;

pub fn init(boot_info: &'static mut BootInfo) {
    let phys_off = VirtAddr::new(
        boot_info
            .physical_memory_offset
            .into_option()
            .expect("bootloader did not provide physical_memory_offset"),
    );
    let mapper = unsafe { offset_table(phys_off) };
    let alloc = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_regions) };
    unsafe {
        (&raw mut MAPPER).write(Some(mapper));
        (&raw mut FRAME_ALLOC).write(Some(alloc));
    }
}

/// Map `size` bytes at `start` as present + writable 4 KiB pages.
pub fn map_writable(start: usize, size: usize) {
    let mapper = unsafe {
        (*(&raw mut MAPPER)).as_mut().expect("paging::init not called")
    };
    let alloc = unsafe {
        (*(&raw mut FRAME_ALLOC)).as_mut().expect("paging::init not called")
    };

    let start_page = Page::containing_address(VirtAddr::new(start as u64));
    let end_page = Page::containing_address(VirtAddr::new((start + size - 1) as u64));
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

    for page in Page::range_inclusive(start_page, end_page) {
        let frame = alloc
            .allocate_frame()
            .expect("out of frames while mapping heap");
        unsafe {
            mapper
                .map_to(page, frame, flags, alloc)
                .expect("map_to failed")
                .flush();
        }
    }
}

unsafe fn offset_table(phys_offset: VirtAddr) -> OffsetPageTable<'static> {
    let (frame, _) = x86_64::registers::control::Cr3::read();
    let phys = frame.start_address();
    let virt = phys_offset + phys.as_u64();
    let ptr = virt.as_mut_ptr::<PageTable>();
    unsafe { OffsetPageTable::new(&mut *ptr, phys_offset) }
}

struct BootInfoFrameAllocator {
    regions: &'static [MemoryRegion],
    next: usize,
}

impl BootInfoFrameAllocator {
    unsafe fn init(regions: &'static MemoryRegions) -> Self {
        Self {
            regions: &regions[..],
            next: 0,
        }
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
        self.regions
            .iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable)
            .flat_map(|r| (r.start..r.end).step_by(4096))
            .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames().nth(self.next)?;
        self.next += 1;
        Some(frame)
    }
}
