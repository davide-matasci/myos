//! Per-CPU GDT + TSS so each AP can safely ring3↔ring0 (own RSP0 / IST).

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};
use x86_64::instructions::segmentation::{Segment, CS, SS};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

use crate::smp;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

#[derive(Clone, Copy)]
struct Selectors {
    code: SegmentSelector,
    data: SegmentSelector,
    user_data: SegmentSelector,
    user_code: SegmentSelector,
    tss: SegmentSelector,
}

static mut GDT: [MaybeUninit<GlobalDescriptorTable>; smp::MAX_CPUS] =
    [const { MaybeUninit::uninit() }; smp::MAX_CPUS];
static mut TSS: [MaybeUninit<TaskStateSegment>; smp::MAX_CPUS] =
    [const { MaybeUninit::uninit() }; smp::MAX_CPUS];
static mut SEL: [MaybeUninit<Selectors>; smp::MAX_CPUS] =
    [const { MaybeUninit::uninit() }; smp::MAX_CPUS];
static mut DF_STACKS: [[u8; 4096 * 5]; smp::MAX_CPUS] = [[0; 4096 * 5]; smp::MAX_CPUS];
static READY: [AtomicBool; smp::MAX_CPUS] = [const { AtomicBool::new(false) }; smp::MAX_CPUS];
static SHARED_SEL: spin::Once<Selectors> = spin::Once::new();

fn init_for_cpu(cpu: usize) {
    assert!(cpu < smp::MAX_CPUS);
    if READY[cpu].swap(true, Ordering::SeqCst) {
        load_cpu(cpu);
        return;
    }

    let tss_ptr = unsafe {
        let tss = TSS[cpu].write(TaskStateSegment::new());
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            let start = VirtAddr::from_ptr(core::ptr::addr_of!(DF_STACKS[cpu]));
            start + 4096 * 5
        };
        tss as *const TaskStateSegment
    };

    let (gdt, selectors) = unsafe {
        let gdt = GDT[cpu].write(GlobalDescriptorTable::new());
        let code = gdt.append(Descriptor::kernel_code_segment());
        let data = gdt.append(Descriptor::kernel_data_segment());
        let user_data = gdt.append(Descriptor::user_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
        // Descriptor captures the static TSS address (not a temporary).
        let tss_sel = gdt.append(Descriptor::tss_segment(&*tss_ptr));
        let selectors = Selectors {
            code,
            data,
            user_data,
            user_code,
            tss: tss_sel,
        };
        SEL[cpu].write(selectors);
        (gdt as *const GlobalDescriptorTable, selectors)
    };
    let _ = gdt;
    SHARED_SEL.call_once(|| selectors);
    load_cpu(cpu);
}

fn load_cpu(cpu: usize) {
    let gdt = unsafe { GDT[cpu].assume_init_ref() };
    let sel = unsafe { SEL[cpu].assume_init_ref() };
    unsafe {
        gdt.load_unsafe();
        CS::set_reg(sel.code);
        SS::set_reg(sel.data);
        load_tss(sel.tss);
    }
}

pub fn init() {
    init_for_cpu(0);
}

/// Secondary CPU: own GDT+TSS so `ltr` does not #GP on a Busy shared descriptor.
pub fn load_for_ap(cpu: usize) {
    init_for_cpu(cpu);
}

pub fn set_rsp0(rsp: u64) {
    let cpu = smp::cpu_id().min(smp::MAX_CPUS - 1);
    assert!(READY[cpu].load(Ordering::SeqCst), "TSS");
    let tss = unsafe { TSS[cpu].assume_init_mut() };
    unsafe {
        core::ptr::write_unaligned(
            core::ptr::addr_of_mut!(tss.privilege_stack_table[0]),
            VirtAddr::new(rsp),
        );
    }
}

fn selectors() -> &'static Selectors {
    SHARED_SEL.get().expect("GDT")
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
