//! Kernel code/data, user data/code, TSS with a double-fault IST stack.

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Once;
use x86_64::instructions::segmentation::{Segment, CS, SS};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

static TSS: Once<TaskStateSegment> = Once::new();
static GDT: Once<(GlobalDescriptorTable, Selectors)> = Once::new();
static TSS_PTR: AtomicUsize = AtomicUsize::new(0);
static mut DF_STACK: [u8; 4096 * 5] = [0; 4096 * 5];

struct Selectors {
    code: SegmentSelector,
    #[allow(dead_code)]
    data: SegmentSelector,
    user_data: SegmentSelector,
    user_code: SegmentSelector,
    #[allow(dead_code)]
    tss: SegmentSelector,
}

pub fn init() {
    let tss = TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            let start = VirtAddr::from_ptr(core::ptr::addr_of!(DF_STACK));
            start + 4096 * 5
        };
        tss
    });
    TSS_PTR.store(tss as *const TaskStateSegment as usize, Ordering::SeqCst);

    let (gdt, sel) = GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let code = gdt.append(Descriptor::kernel_code_segment());
        let data = gdt.append(Descriptor::kernel_data_segment());
        let user_data = gdt.append(Descriptor::user_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
        let tss_sel = gdt.append(Descriptor::tss_segment(tss));
        (
            gdt,
            Selectors {
                code,
                data,
                user_data,
                user_code,
                tss: tss_sel,
            },
        )
    });

    gdt.load();
    unsafe {
        CS::set_reg(sel.code);
        SS::set_reg(sel.data);
        load_tss(sel.tss);
    }
}

pub fn set_rsp0(rsp: u64) {
    let p = TSS_PTR.load(Ordering::SeqCst) as *mut TaskStateSegment;
    assert!(!p.is_null(), "TSS");
    unsafe {
        core::ptr::write_unaligned(
            core::ptr::addr_of_mut!((*p).privilege_stack_table[0]),
            VirtAddr::new(rsp),
        );
    }
}

fn selectors() -> &'static Selectors {
    &GDT.call_once(|| panic!("GDT not initialized")).1
}

pub fn kernel_cs() -> u16 {
    selectors().code.0
}

pub fn user_cs() -> u16 {
    selectors().user_code.0
}

pub fn user_ss() -> u16 {
    selectors().user_data.0
}
