//! Loading user ELF images: initial load, in-place reload/expand on exec,
//! the heap window and the initial argv/envp stack.

use super::*;

/// Realize `bytes` at USER_BASE: frames, stack page, aspace.
/// Callers that abandon a prior aspace must [`reclaim_user_aspace`] it.
/// `relocate == false` leaves the relocations to a dynamic linker (see
/// [`elf::realize_as`]); the same holds for reload and expand below.
pub(super) fn load_user_elf(bytes: &[u8], relocate: bool) -> Option<(u64, usize, usize, u64)> {
    let info = elf::image_span(bytes).ok()?;
    let code_pages = info.span.div_ceil(PAGE);
    if code_pages == 0 || code_pages > MAX_INIT_PAGES {
        return None;
    }
    let n_pages = code_pages.max(USER_EXEC_RELOAD_PAGES).min(MAX_INIT_PAGES);

    let base = USER_BASE.load(Ordering::SeqCst);
    let stack_off = (n_pages * PAGE) as u64;

    if info.span > ELF_SCRATCH_BYTES {
        return None;
    }
    let _scratch_guard = ELF_SCRATCH_LOCK.lock();
    let buf = elf_scratch_mut(info.span)?;
    unsafe {
        core::ptr::write_bytes(buf.as_mut_ptr(), 0, info.span);
    }
    // A crafted ELF whose lowest vaddr is above the load base would make
    // this bias underflow (span_from caps it below IMAGE_VADDR_MAX; a real
    // image's min_vaddr is 0 for PIE or the base for ET_EXEC).
    let load_bias = base.checked_sub(info.min_vaddr)?;
    let entry = match elf::realize_as(bytes, buf.as_mut_ptr(), load_bias, relocate) {
        Ok(e) => e,
        Err(_) => return None,
    };

    // Heap, not kstack: MAX_INIT_PAGES×8 ≈ 8KiB would sit on the same
    // 64KiB stack as the live syscall trap frame (riscv64 exec→load fallback).
    let mut frames = alloc::vec![0u64; n_pages];
    for i in 0..n_pages {
        frames[i] = mm::alloc_frame_site(2);
        if i < code_pages {
            let off = i * PAGE;
            let len = core::cmp::min(PAGE, info.span - off);
            unsafe {
                core::ptr::copy_nonoverlapping(buf.as_ptr().add(off), mm::hhdm(frames[i]), len);
                if len < PAGE {
                    core::ptr::write_bytes(mm::hhdm(frames[i]).add(len), 0, PAGE - len);
                }
            }
            sync_icache(mm::hhdm(frames[i]) as usize, PAGE);
        } else {
            unsafe {
                core::ptr::write_bytes(mm::hhdm(frames[i]), 0, PAGE);
            }
            sync_icache(mm::hhdm(frames[i]) as usize, PAGE);
        }
    }
    drop(_scratch_guard);
    let mut stack_frames = [0u64; USER_STACK_PAGES];
    for frame in &mut stack_frames {
        *frame = mm::alloc_frame_site(5);
    }
    let aspace = create_aspace(&frames[..n_pages], &stack_frames, base, stack_off);
    apply_elf_load_prots(aspace, bytes, base, code_pages);
    map_initial_heap_pages(aspace, base, stack_off);
    let mapped_span = n_pages * PAGE;
    Some((aspace, entry as usize, mapped_span, stack_off))
}

fn map_initial_heap_pages(aspace: u64, base: u64, stack_off: u64) {
    // All arches map the brk window on demand via `sys_brk`. Eagerly pre-mapping
    // HEAP_PAGES on x86 (historically a "small" 256-page window) became a 16 MiB
    // per-load/expand allocation storm after HEAP_PAGES rose to 4096 for GNU make,
    // and dominated the bios/uefi full-boot OOM (fault0 site ≈96% of live frames)
    // across the 137-case os-test smoke. aarch64/riscv already used on-demand.
    let _ = (aspace, base, stack_off);
}

/// Remap the loaded image so PT_LOAD `p_flags` control R/W/X.
///
/// `load_user_elf` / expand initially map the whole span executable (and often
/// writable). That let a bad jump into `.data.rel.ro` become an illegal
/// instruction (`scause=0x2` on zeros) instead of an instruction page fault.
fn apply_elf_load_prots(aspace: u64, bytes: &[u8], base: u64, image_pages: usize) {
    let Ok(info) = elf::image_span(bytes) else {
        return;
    };
    let min_v = info.min_vaddr;
    for i in 0..image_pages {
        let va = base + (i * PAGE) as u64;
        let Some(phys) = virt_to_phys(aspace, va) else {
            continue;
        };
        let page_lo = min_v + (i * PAGE) as u64;
        let page_hi = page_lo + PAGE as u64;
        let mut prot = 0usize;
        let _ = elf::for_each_load_segment(bytes, |seg| {
            let seg_lo = seg.vaddr;
            let seg_hi = seg.vaddr.saturating_add(seg.memsz);
            if page_lo < seg_hi && page_hi > seg_lo {
                prot |= elf::pf_to_prot(seg.flags);
            }
        });
        if prot == 0 {
            prot = PROT_READ;
        }
        map_user_page_prot(aspace, va, phys, prot);
    }
    flush_user_tlb();
}

pub(super) fn reuse_or_alloc_frame(aspace: u64, va: u64) -> u64 {
    if let Some(phys) = virt_to_phys(aspace, va) {
        phys
    } else {
        let frame = mm::alloc_frame_site(1);
        // alloc_frame returns a zeroed frame.
        frame
    }
}

static ELF_SCRATCH_PHYS: AtomicU64 = AtomicU64::new(0);
static ELF_SCRATCH_PAGES: AtomicUsize = AtomicUsize::new(0);
/// Serialize realize→copy-out on the shared scratch. Pipeline children
/// (`cat | cat`) RR-spread across APs under `-smp 4` and exec concurrently;
/// without this lock they shred each other's relocated image (UEFI full-boot
/// user #PF cr2=0x401 / bogus low pointer in sbase `cat` after os-test).
static ELF_SCRATCH_LOCK: Mutex<()> = Mutex::new(());

/// Borrow the shared ELF scratch. Caller must hold [`ELF_SCRATCH_LOCK`] for
/// the whole realize + copy-into-aspace window — not merely this call.
pub(super) fn elf_scratch_mut(len: usize) -> Option<&'static mut [u8]> {
    if len == 0 || len > ELF_SCRATCH_BYTES {
        return None;
    }
    let need_pages = len.div_ceil(PAGE);
    let mut phys = ELF_SCRATCH_PHYS.load(Ordering::SeqCst);
    let have = ELF_SCRATCH_PAGES.load(Ordering::SeqCst);
    if phys == 0 || need_pages > have {
        // Contiguous bump only — freelist pages are rarely adjacent and would
        // make grow-on-demand fail spuriously (see `alloc_contiguous_frames`).
        let first = mm::alloc_contiguous_frames(need_pages)?;
        // Prior smaller scratch is leaked (bump allocator cannot free).
        ELF_SCRATCH_PHYS.store(first, Ordering::SeqCst);
        ELF_SCRATCH_PAGES.store(need_pages, Ordering::SeqCst);
        phys = first;
    }
    Some(unsafe { core::slice::from_raw_parts_mut(mm::hhdm(phys), len) })
}

/// Overwrite the current user image in an existing aspace (no new frames).
/// Keeps `stack_off` so the mapped stack page stays valid. Fails if the new
/// image needs more bytes than the current mapping span.
pub(super) fn reload_user_elf(
    aspace: u64,
    bytes: &[u8],
    base: u64,
    stack_off: u64,
    _mapped_span: usize,
    relocate: bool,
) -> Option<(usize, usize, u64)> {
    let info = elf::image_span(bytes).ok()?;
    let image_pages = info.span.div_ceil(PAGE);
    if image_pages == 0 || image_pages > MAX_RELOAD_PAGES {
        return None;
    }
    let n_pages = image_pages
        .max(USER_EXEC_RELOAD_PAGES)
        .min(MAX_RELOAD_PAGES);
    // Code must sit below the mapped stack. Otherwise PT_LOAD pages overlap stack
    // slots (reload saw stack PTEs as "mapped" and clobbered them — heap #10).
    if n_pages * PAGE > stack_off as usize {
        return None;
    }
    for i in 0..n_pages {
        if virt_to_phys(aspace, base + (i * PAGE) as u64).is_none() {
            return None;
        }
    }
    if info.span > MAX_RELOAD_PAGES * PAGE {
        return None;
    }
    let _scratch_guard = ELF_SCRATCH_LOCK.lock();
    let buf = elf_scratch_mut(info.span)?;
    unsafe {
        core::ptr::write_bytes(buf.as_mut_ptr(), 0, info.span);
    }
    // A crafted ELF whose lowest vaddr is above the load base would make
    // this bias underflow (span_from caps it below IMAGE_VADDR_MAX; a real
    // image's min_vaddr is 0 for PIE or the base for ET_EXEC).
    let load_bias = base.checked_sub(info.min_vaddr)?;
    let entry = match elf::realize_as(bytes, buf.as_mut_ptr(), load_bias, relocate) {
        Ok(e) => e,
        Err(_) => return None,
    };
    for i in 0..n_pages {
        let va = base + (i * PAGE) as u64;
        let Some(phys) = virt_to_phys(aspace, va) else {
            return None;
        };
        let off = i * PAGE;
        if i < image_pages {
            let len = core::cmp::min(PAGE, info.span.saturating_sub(off));
            unsafe {
                if len > 0 {
                    core::ptr::copy_nonoverlapping(buf.as_ptr().add(off), mm::hhdm(phys), len);
                }
                if len < PAGE {
                    core::ptr::write_bytes(mm::hhdm(phys).add(len), 0, PAGE - len);
                }
            }
        } else {
            unsafe {
                core::ptr::write_bytes(mm::hhdm(phys), 0, PAGE);
            }
        }
        sync_icache(mm::hhdm(phys) as usize, PAGE);
    }
    drop(_scratch_guard);
    apply_elf_load_prots(aspace, bytes, base, image_pages);
    // Drop inherited brk pages so the next image starts with an empty on-demand
    // heap (see `free_abandoned_stack_heap`). Reload keeps `stack_off` fixed so
    // expand's reclaim would otherwise no-op on the heap window.
    //
    // On riscv64 the syscall keeps the *user* root loaded in satp for the whole
    // syscall (kernel maps live in the same Sv39 root), so sret back to user
    // does not flush the TLB the way the x86 cr3 swap does. Without this
    // sfence the task returns to user mode with stale VA→PA entries for the
    // freed brk window; later sys_brk writes then hit recycled frames
    // belonging to other tasks — the random git-object/TLS/ra=0 / sepc=0
    // corruption seen in the riscv64 CI boot flake. munmap, brk-shrink, expand
    // and the exec wrapper all flush after their unmaps; this path was the
    // only one that skipped it.
    free_heap_window(aspace, base, stack_off);
    flush_user_tlb();
    Some((entry as usize, n_pages * PAGE, stack_off))
}

/// Unmap+free every mapped page in the brk window for `stack_off`.
fn free_heap_window(aspace: u64, base: u64, stack_off: u64) {
    let mut va = heap_base_va(base, stack_off);
    let lim = heap_limit_va(base, stack_off);
    while va < lim {
        free_mapped_page(aspace, va);
        va += PAGE as u64;
    }
}

/// Grow the current aspace and load a large ELF (up to [`MAX_EXPAND_PAGES`]).
/// Used when [`reload_user_elf`] is too small but we already have an aspace
/// (post-fork exec of release uutils / ripgrep).
/// Free stack/heap pages from a prior expand/load when the stack window moves,
/// and **always** drop the old heap window on in-place exec.
///
/// Pages that fall inside the new code span `[base, base+new_stack_off)` are
/// kept for `reuse_or_alloc_frame` to turn into code. Without stack reclaim,
/// each uutils→rg-sized expand abandons `USER_STACK_PAGES + HEAP_PAGES` frames
/// and riscv UEFI walks the freelist dry → classic `sepc=0` after the next ecall.
///
/// Heap is freed even when `stack_off` is unchanged: in-place reload/expand used
/// to leave prior brk pages mapped while `replace_user` reset `brk_cur` to
/// `heap_base`, so exec-heavy smoke accumulated anonymous heap and freelist
/// pressure until later HTTPS faults looked "random".
fn free_abandoned_stack_heap(aspace: u64, base: u64, old_stack_off: u64, new_stack_off: u64) {
    if old_stack_off == 0 {
        return;
    }
    let new_code_end = base + new_stack_off;
    if old_stack_off != new_stack_off {
        for i in 0..USER_STACK_PAGES {
            let va = base + old_stack_off + (i * PAGE) as u64;
            if va >= new_code_end {
                free_mapped_page(aspace, va);
            }
        }
    }
    let mut va = heap_base_va(base, old_stack_off);
    let lim = heap_limit_va(base, old_stack_off);
    while va < lim {
        if va >= new_code_end {
            free_mapped_page(aspace, va);
        }
        va += PAGE as u64;
    }
}

pub(super) fn expand_user_elf(
    aspace: u64,
    bytes: &[u8],
    base: u64,
    old_stack_off: u64,
    relocate: bool,
) -> Option<(usize, usize, u64)> {
    let info = elf::image_span(bytes).ok()?;
    let image_pages = info.span.div_ceil(PAGE);
    if image_pages == 0 || image_pages > MAX_EXPAND_PAGES {
        return None;
    }
    let n_pages = image_pages.max(USER_EXEC_RELOAD_PAGES).min(MAX_EXPAND_PAGES);
    let new_stack_off = (n_pages * PAGE) as u64;
    if info.span > ELF_SCRATCH_BYTES {
        return None;
    }

    // Realize into scratch *before* touching the live aspace. Growing scratch
    // (or a realize error) must not leave the caller with a half-rewritten
    // mapping and a SYSERR return into wiped user text (ISO login rip=0).
    // Hold ELF_SCRATCH_LOCK across realize→copy-out so a peer AP exec
    // (pipeline RR) cannot overwrite scratch mid-flight.
    let _scratch_guard = ELF_SCRATCH_LOCK.lock();
    let buf = elf_scratch_mut(info.span)?;
    unsafe {
        core::ptr::write_bytes(buf.as_mut_ptr(), 0, info.span);
    }
    // A crafted ELF whose lowest vaddr is above the load base would make
    // this bias underflow (span_from caps it below IMAGE_VADDR_MAX; a real
    // image's min_vaddr is 0 for PIE or the base for ET_EXEC).
    let load_bias = base.checked_sub(info.min_vaddr)?;
    let entry = match elf::realize_as(bytes, buf.as_mut_ptr(), load_bias, relocate) {
        Ok(e) => e,
        Err(_) => return None,
    };

    free_abandoned_stack_heap(aspace, base, old_stack_off, new_stack_off);
    // Drop stale VA→PA before reuse_or_alloc / remap. Without this, a recycled
    // frame can still be reachable via TLB while the PTE walk already sees the
    // clear — classic freelist/PTE skew behind sepc=0 after expand (ripgrep).
    flush_user_tlb();

    // Remap code/stack PTEs with correct flags. Reuse existing frames when the
    // VA is already mapped so post-fork exec of large ELFs does not leak hundreds
    // of physical pages and walk the bump allocator into the kernel image.
    for i in 0..n_pages {
        let va = base + (i * PAGE) as u64;
        let frame = reuse_or_alloc_frame(aspace, va);
        map_user_code_page(aspace, va, frame);
        sync_icache(mm::hhdm(frame) as usize, PAGE);
    }
    for i in 0..USER_STACK_PAGES {
        let va = base + new_stack_off + (i * PAGE) as u64;
        let frame = reuse_or_alloc_frame(aspace, va);
        map_user_stack_page(aspace, va, frame);
    }
    map_initial_heap_pages(aspace, base, new_stack_off);
    flush_user_tlb();

    for i in 0..n_pages {
        let va = base + (i * PAGE) as u64;
        let Some(phys) = virt_to_phys(aspace, va) else {
            return None;
        };
        let off = i * PAGE;
        if i < image_pages {
            let len = core::cmp::min(PAGE, info.span.saturating_sub(off));
            unsafe {
                if len > 0 {
                    core::ptr::copy_nonoverlapping(buf.as_ptr().add(off), mm::hhdm(phys), len);
                }
                if len < PAGE {
                    core::ptr::write_bytes(mm::hhdm(phys).add(len), 0, PAGE - len);
                }
            }
        } else {
            unsafe {
                core::ptr::write_bytes(mm::hhdm(phys), 0, PAGE);
            }
        }
        sync_icache(mm::hhdm(phys) as usize, PAGE);
    }
    apply_elf_load_prots(aspace, bytes, base, image_pages);
    Some((entry as usize, n_pages * PAGE, new_stack_off))
}

/// Auxiliary-vector entries placed before the `AT_NULL` terminator.
pub struct AuxV {
    e: [(usize, usize); 16],
    n: usize,
}

impl AuxV {
    pub const fn new() -> Self {
        Self { e: [(0, 0); 16], n: 0 }
    }
    pub fn push(&mut self, key: usize, val: usize) {
        if self.n < self.e.len() {
            self.e[self.n] = (key, val);
            self.n += 1;
        }
    }
    pub fn entries(&self) -> &[(usize, usize)] {
        &self.e[..self.n]
    }
}

/// SysV-style user stack at process entry (`rsp % 16 == 0`):
/// `[argc][argv…][NULL][envp…][NULL][auxv…][AT_NULL][gap][strings…]`.
pub(super) fn build_argv_stack(
    aspace: u64,
    user_base: u64,
    stack_off: u64,
    args: &[&[u8]],
    env: &[&[u8]],
    aux: &[(usize, usize)],
) -> Option<(usize, usize)> {
    if args.len() > MAX_ARGC || env.len() > MAX_ENVC {
        return None;
    }
    let total: usize = args.iter().chain(env).map(|s| s.len() + 1).sum();
    if total > MAX_EXEC_STRINGS {
        return None;
    }
    let stack_top = (user_base + stack_off + (USER_STACK_PAGES * PAGE) as u64) as usize;
    let stack_bot = (user_base + stack_off) as usize;
    // Leave bytes below stack_top: argv strings must not end at stack_top (unmapped).
    let mut sp = stack_top.checked_sub(64)?;
    // Each string with its NUL, highest first; returns where each landed.
    let mut push_strings = |list: &[&[u8]]| -> Option<Vec<usize>> {
        let mut ptrs = Vec::with_capacity(list.len());
        for item in list {
            sp = sp.checked_sub(item.len() + 1)?;
            if sp < stack_bot {
                return None;
            }
            if !write_user_bytes(aspace, sp, item) || !write_user_bytes(aspace, sp + item.len(), &[0]) {
                return None;
            }
            ptrs.push(sp);
        }
        Some(ptrs)
    };
    let arg_ptrs = push_strings(args)?;
    let env_ptrs = push_strings(env)?;
    // Gap so the argv pointer table cannot overlap the copied strings (x86 std
    // `_start` reads argv[] immediately; a tight layout can alias string bytes).
    sp = sp.checked_sub(16)?;
    if sp < stack_bot {
        return None;
    }
    // argc + argv + NULL + envp + NULL + auxv pairs + AT_NULL (type,val).
    let words = 1 + args.len() + 1 + env.len() + 1 + 2 * aux.len() + 2;
    sp = sp.checked_sub(words * core::mem::size_of::<usize>())?;
    // Linux/SysV AMD64: %rsp ≡ 0 (mod 16) at _start. crt0/`call` then yields
    // callee %rsp ≡ 8. fe50282 forced ≡8 with an extra `sp -= 8`, which inverted
    // every frame (oksh pipe #GP) and required a matching dummy `push` in the
    // Rust PAL; keep ≡0 here and leave PAL without that pad.
    let pad = sp & 15;
    if pad != 0 {
        sp = sp.checked_sub(pad)?;
    }
    debug_assert_eq!(sp & 15, 0);
    if sp < stack_bot {
        return None;
    }
    let argc_sp = sp;
    if !write_user_usize(aspace, sp, args.len()) {
        return None;
    }
    sp += core::mem::size_of::<usize>();
    for i in 0..args.len() {
        if !write_user_usize(aspace, sp, arg_ptrs[i]) {
            return None;
        }
        sp += core::mem::size_of::<usize>();
    }
    if !write_user_usize(aspace, sp, 0) {
        return None;
    }
    sp += core::mem::size_of::<usize>();
    for i in 0..env.len() {
        if !write_user_usize(aspace, sp, env_ptrs[i]) {
            return None;
        }
        sp += core::mem::size_of::<usize>();
    }
    if !write_user_usize(aspace, sp, 0) {
        return None;
    }
    sp += core::mem::size_of::<usize>();
    for &(key, val) in aux {
        if !write_user_usize(aspace, sp, key) || !write_user_usize(aspace, sp + 8, val) {
            return None;
        }
        sp += 2 * core::mem::size_of::<usize>();
    }
    // Minimal auxv terminator (AT_NULL). Fresh stacks are zeroed, but write it
    // explicitly so crt0/std walkers never read string bytes as aux entries.
    if !write_user_usize(aspace, sp, 0) {
        return None;
    }
    sp += core::mem::size_of::<usize>();
    if !write_user_usize(aspace, sp, 0) {
        return None;
    }
    Some((argc_sp, argc_sp + core::mem::size_of::<usize>()))
}

/// Map ELF `bytes` at `va` in `aspace` without relocating it (a dynamic
/// linker relocates itself): fresh frames, protections from its PT_LOAD
/// flags. Returns the biased entry and the mapped runs `(va, pages, prot)`
/// for the caller to record as mmap regions.
pub(crate) fn map_elf_unrelocated(
    aspace: u64,
    bytes: &[u8],
    va: u64,
) -> Option<(usize, Vec<(u64, u32, u32)>)> {
    let info = elf::image_span(bytes).ok()?;
    if info.min_vaddr % PAGE as u64 != 0 || info.span > ELF_SCRATCH_BYTES {
        return None;
    }
    let pages = info.span.div_ceil(PAGE);
    let guard = ELF_SCRATCH_LOCK.lock();
    let buf = elf_scratch_mut(info.span)?;
    let load_bias = va.checked_sub(info.min_vaddr)?;
    let entry = elf::realize_as(bytes, buf.as_mut_ptr(), load_bias, false).ok()?;
    let mut runs: Vec<(u64, u32, u32)> = Vec::new();
    for i in 0..pages {
        let page_lo = info.min_vaddr + (i * PAGE) as u64;
        let page_hi = page_lo + PAGE as u64;
        let mut prot = 0usize;
        let _ = elf::for_each_load_segment(bytes, |seg| {
            if page_lo < seg.vaddr.saturating_add(seg.memsz) && page_hi > seg.vaddr {
                prot |= elf::pf_to_prot(seg.flags);
            }
        });
        if prot == 0 {
            prot = PROT_READ;
        }
        let frame = mm::alloc_frame_site(2);
        let off = i * PAGE;
        let len = PAGE.min(info.span - off);
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr().add(off), mm::hhdm(frame), len);
        }
        sync_icache(mm::hhdm(frame) as usize, PAGE);
        let page_va = va + off as u64;
        map_user_page_prot(aspace, page_va, frame, prot);
        match runs.last_mut() {
            Some(r) if r.2 == prot as u32 && r.0 + r.1 as u64 * PAGE as u64 == page_va => r.1 += 1,
            _ => runs.push((page_va, 1, prot as u32)),
        }
    }
    drop(guard);
    flush_user_tlb();
    Some((entry as usize, runs))
}
