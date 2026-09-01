/// Read from fd 0 (keyboard + serial stdin). `buf` must lie in the user map.
pub fn fd_read_stdin(buf: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    if !user::buffer_ok(buf, len) {
        return usize::MAX;
    }
    let mut tmp = [0u8; 128];
    let want = len.min(tmp.len());
    // Do not call input::read (may yield) while TASKS is locked — deadlock.
    let n = crate::input::read(&mut tmp[..want]);
    let aspace = current_aspace();
    if !user::copy_to_user(aspace, buf, &tmp[..n]) {
        return usize::MAX;
    }
    n
}

pub fn fd_read(fd: usize, buf: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    loop {
        let (entry, map) = {
            let flags = irq_save();
            irq_off();
            let id = CURRENT.load(Ordering::SeqCst);
            let t = TASKS.lock()[id];
            irq_restore(flags);
            (
                t.fds.get(fd).copied().unwrap_or(FdEntry::Empty),
                (
                    t.user_base,
                    t.image_span,
                    t.stack_off,
                    t.brk_cur as usize,
                ),
            )
        };
        let (user_base, image_span, stack_off, brk) = map;
        let user_base = user_base as usize;
        let stack_off = stack_off as usize;
        if !user_buf_ok(buf, len.min(128), user_base, image_span, stack_off, brk) {
            return usize::MAX;
        }
        match entry {
            FdEntry::Stdin => return fd_read_stdin(buf, len),
            FdEntry::File { node, pos: _ } => {
                return with_current_mut(|t| {
                    let FdEntry::File { node, pos } = t.fds[fd] else {
                        return usize::MAX;
                    };
                    let mut tmp = [0u8; 128];
                    let want = len.min(tmp.len());
                    let n = crate::fs::read(&node, pos, &mut tmp[..want]);
                    if n != 0 {
                        if !user_buf_ok(
                            buf,
                            n,
                            t.user_base as usize,
                            t.image_span,
                            t.stack_off as usize,
                            t.brk_cur as usize,
                        ) {
                            return usize::MAX;
                        }
                        if !user::copy_to_user(t.aspace, buf, &tmp[..n]) {
                            return usize::MAX;
                        }
                    }
                    if let FdEntry::File { pos: p, .. } = &mut t.fds[fd] {
                        *p += n;
                    }
                    n
                });
            }
            FdEntry::PipeRead(id) => {
                let mut tmp = [0u8; 128];
                let want = len.min(tmp.len());
                let n = pipe::read(id, &mut tmp[..want]);
                if n == usize::MAX {
                    return usize::MAX;
                }
                if n == 0 && pipe::read_would_block(id) {
                    yield_now();
                    continue;
                }
                let aspace = current_aspace();
                if !user::copy_to_user(aspace, buf, &tmp[..n]) {
                    return usize::MAX;
                }
                return n;
            }
            FdEntry::Empty | FdEntry::Console | FdEntry::PipeWrite(_) => return usize::MAX,
        }
    }
}

pub fn fd_write(fd: usize, buf: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let mut total = 0usize;
    while total < len {
        let chunk = (len - total).min(128);
        let (entry, map) = {
            let flags = irq_save();
            irq_off();
            let id = CURRENT.load(Ordering::SeqCst);
            let t = TASKS.lock()[id];
            irq_restore(flags);
            (
                t.fds.get(fd).copied().unwrap_or(FdEntry::Empty),
                (
                    t.user_base,
                    t.image_span,
                    t.stack_off,
                    t.brk_cur as usize,
                ),
            )
        };
        let (user_base, image_span, stack_off, brk) = map;
        if !user_buf_ok(
            buf + total,
            chunk,
            user_base as usize,
            image_span,
            stack_off as usize,
            brk,
        ) {
            return if total == 0 { usize::MAX } else { total };
        }
        let mut tmp = [0u8; 128];
        unsafe {
            core::ptr::copy_nonoverlapping((buf + total) as *const u8, tmp.as_mut_ptr(), chunk);
        }
        loop {
            match entry {
                FdEntry::Console => {
                    print_bytes(&tmp[..chunk]);
                    total += chunk;
                    break;
                }
                FdEntry::PipeWrite(id) => {
                    let n = pipe::write(id, &tmp[..chunk]);
                    if n == usize::MAX {
                        return if total == 0 { usize::MAX } else { total };
                    }
                    if n == 0 && pipe::write_would_block(id) {
                        yield_now();
                        continue;
                    }
                    total += n;
                    break;
                }
                _ => return if total == 0 { usize::MAX } else { total },
            }
        }
    }
    total
}

pub fn fd_close(fd: usize) -> bool {
    if fd >= MAX_FDS {
        return false;
    }
    with_current_mut(|t| {
        let entry = t.fds[fd];
        if entry == FdEntry::Empty {
            return false;
        }
        fd_drop(entry);
        t.fds[fd] = if fd == 0 {
            FdEntry::Stdin
        } else if fd == 1 || fd == 2 {
            FdEntry::Console
        } else {
            FdEntry::Empty
        };
        true
    })
}

/// In-place exec: replace the current task's user image. Does not spawn,
/// does not bump USERS_ALIVE, does not note_exit. Keeps the fd table so
/// shell redirects and pipes survive exec.
pub fn replace_user(
    aspace: u64,
    user_rip: usize,
    user_rsp: usize,
    user_base: u64,
    image_span: usize,
    stack_off: u64,
    user_argc: usize,
    user_argv: usize,
) {
    with_current_mut(|t| {
        t.aspace = aspace;
        t.user_rip = user_rip;
        t.user_rsp = user_rsp;
        t.user_base = user_base;
        t.image_span = image_span;
        t.stack_off = stack_off;
        t.user_argc = user_argc;
        t.user_argv = user_argv;
        t.fork_regs = None;
        t.brk_cur = heap_base_for(user_base, stack_off);
    });
    user::switch_aspace(aspace);
    LOADED_ASPACE.store(aspace, Ordering::SeqCst);
}

