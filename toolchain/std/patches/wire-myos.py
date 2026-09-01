#!/usr/bin/env python3
"""Insert target_os = \"myos\" wiring into a copied Rust library tree."""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("target/rust-std-patch/library")

NOT_TRUSTY_OR_MYOS = '#[cfg(not(target_os = "trusty"))]'
NOT_TRUSTY_OR_MYOS_NEW = '#[cfg(not(any(target_os = "trusty", target_os = "myos")))]'

REPLACEMENTS: list[tuple[str, list[tuple[str, str]]]] = [
    (
        "std/src/sys/time/mod.rs",
        [
            (
                '    target_os = "hermit" => {\n        mod hermit;\n        use hermit as imp;\n    }',
                '    target_os = "hermit" => {\n        mod hermit;\n        use hermit as imp;\n    }\n    target_os = "myos" => {\n        mod myos;\n        use myos as imp;\n    }',
            ),
        ],
    ),
    (
        "std/src/lib.rs",
        [
            (
                "#![feature(staged_api)]",
                "#![feature(staged_api)]\n#![feature(myos_ext)]",
            ),
        ],
    ),
    (
        "std/build.rs",
        [
            (
                '        || target_os == "vexos"',
                '        || target_os == "vexos"\n        || target_os == "myos"',
            ),
        ],
    ),
    (
        "std/src/os/mod.rs",
        [
            (
                '    target_os = "hermit",',
                '    target_os = "hermit",\n    target_os = "myos",',
            ),
            (
                '#[cfg(target_os = "hermit")]\npub mod hermit;',
                '#[cfg(target_os = "hermit")]\npub mod hermit;\n#[cfg(target_os = "myos")]\npub mod myos;\n#[cfg(target_os = "myos")]\npub mod unix {\n    #![stable(feature = "rust1", since = "1.0.0")]\n    pub mod io {\n        #![stable(feature = "rust1", since = "1.0.0")]\n        #[stable(feature = "rust1", since = "1.0.0")]\n        pub use crate::os::fd::*;\n    }\n    pub mod fs {\n        #![stable(feature = "rust1", since = "1.0.0")]\n        #[stable(feature = "rust1", since = "1.0.0")]\n        pub use crate::os::myos::fs::*;\n    }\n    pub mod ffi {\n        #![stable(feature = "rust1", since = "1.0.0")]\n        #[stable(feature = "rust1", since = "1.0.0")]\n        pub use crate::os::myos::ffi::*;\n    }\n    pub mod prelude {\n        #![stable(feature = "rust1", since = "1.0.0")]\n        #[stable(feature = "rust1", since = "1.0.0")]\n        pub use crate::os::myos::ffi::OsStrExt;\n    }\n}',
            ),
        ],
    ),
    (
        "std/src/os/fd/raw.rs",
        [
            (
                '#[cfg(target_os = "hermit")]\nuse hermit_abi as libc;',
                '#[cfg(target_os = "hermit")]\nuse hermit_abi as libc;\n#[cfg(target_os = "myos")]\nuse crate::sys::myos::abi as libc;',
            ),
            (
                '#[cfg(target_os = "hermit")]\nuse crate::os::hermit::io::OwnedFd;',
                '#[cfg(target_os = "hermit")]\nuse crate::os::hermit::io::OwnedFd;\n#[cfg(target_os = "myos")]\nuse crate::os::myos::io::OwnedFd;',
            ),
            (
                '#[cfg(all(not(target_os = "hermit"), not(target_os = "motor")))]\nuse crate::os::raw;',
                '#[cfg(all(not(target_os = "hermit"), not(target_os = "motor"), not(target_os = "myos")))]\nuse crate::os::raw;',
            ),
            (
                '#[cfg(all(not(target_os = "hermit"), not(target_os = "motor")))]\npub type RawFd = raw::c_int;',
                '#[cfg(all(not(target_os = "hermit"), not(target_os = "motor"), not(target_os = "myos")))]\npub type RawFd = raw::c_int;',
            ),
            (
                '#[cfg(any(target_os = "hermit", target_os = "motor"))]',
                '#[cfg(any(target_os = "hermit", target_os = "motor", target_os = "myos"))]',
            ),
        ],
    ),
    (
        "std/src/os/fd/owned.rs",
        [
            (
                '    target_os = "motor"\n)))]',
                '    target_os = "motor",\n    target_os = "myos"\n)))]',
            ),
            (
                '    #[cfg(not(any(\n        all(target_arch = "wasm32", not(target_os = "emscripten")),\n        target_os = "hermit",\n        target_os = "trusty",\n        target_os = "motor"\n    )))]',
                '    #[cfg(not(any(\n        all(target_arch = "wasm32", not(target_os = "emscripten")),\n        target_os = "hermit",\n        target_os = "myos",\n        target_os = "trusty",\n        target_os = "motor"\n    )))]',
            ),
            (
                '    #[cfg(any(\n        all(target_arch = "wasm32", not(target_os = "emscripten")),\n        target_os = "hermit",\n        target_os = "trusty"\n    ))]',
                '    #[cfg(any(\n        all(target_arch = "wasm32", not(target_os = "emscripten")),\n        target_os = "hermit",\n        target_os = "myos",\n        target_os = "trusty"\n    ))]',
            ),
            (
                '            #[cfg(not(target_os = "hermit"))]\n            {\n                #[cfg(unix)]\n                crate::sys::fs::debug_assert_fd_is_open(self.fd.as_inner());\n\n                let _ = libc::close(self.fd.as_inner());\n            }\n            #[cfg(target_os = "hermit")]\n            let _ = hermit_abi::close(self.fd.as_inner());',
                '            #[cfg(not(any(target_os = "hermit", target_os = "myos")))]\n            {\n                #[cfg(unix)]\n                crate::sys::fs::debug_assert_fd_is_open(self.fd.as_inner());\n\n                let _ = libc::close(self.fd.as_inner());\n            }\n            #[cfg(target_os = "hermit")]\n            let _ = hermit_abi::close(self.fd.as_inner());\n            #[cfg(target_os = "myos")]\n            let _ = crate::sys::myos::abi::close(self.fd.as_inner());',
            ),
        ],
    ),
    (
        "std/src/sys/fs/mod.rs",
        [
            (
                '    target_os = "hermit" => {\n        mod hermit;\n        use hermit as imp;\n    }',
                '    target_os = "hermit" => {\n        mod hermit;\n        use hermit as imp;\n    }\n    target_os = "myos" => {\n        mod myos;\n        use myos as imp;\n    }',
            ),
        ],
    ),
    (
        "std/src/sys/process/mod.rs",
        [
            (
                '    target_os = "motor" => {\n        mod motor;\n        use motor as imp;\n    }',
                '    target_os = "motor" => {\n        mod motor;\n        use motor as imp;\n    }\n    target_os = "myos" => {\n        mod myos;\n        use myos as imp;\n    }',
            ),
        ],
    ),
    (
        "std/src/sys/pal/mod.rs",
        [
            (
                '    target_os = "hermit" => {\n        mod hermit;\n        pub use self::hermit::*;\n    }',
                '    target_os = "hermit" => {\n        mod hermit;\n        pub use self::hermit::*;\n    }\n    target_os = "myos" => {\n        mod myos;\n        pub use self::myos::*;\n    }',
            ),
        ],
    ),
    (
        "std/src/sys/mod.rs",
        [
            (
                'pub mod alloc;',
                '#[cfg(target_os = "myos")]\npub mod myos;\n\npub mod alloc;',
            ),
        ],
    ),
    (
        "std/src/sys/alloc/mod.rs",
        [
            (
                '    target_os = "hermit" => {\n        mod hermit;\n        use hermit as imp;\n    }',
                '    target_os = "hermit" => {\n        mod hermit;\n        use hermit as imp;\n    }\n    target_os = "myos" => {\n        mod myos;\n        use myos as imp;\n    }',
            ),
            (
                '        target_os = "hermit",',
                '        target_os = "hermit",\n        target_os = "myos",',
            ),
        ],
    ),
    (
        "std/src/sys/stdio/mod.rs",
        [
            (
                '    target_os = "motor" => {\n        mod motor;\n        pub use motor::*;\n    }',
                '    target_os = "motor" => {\n        mod motor;\n        pub use motor::*;\n    }\n    target_os = "myos" => {\n        mod myos;\n        pub use myos::*;\n    }',
            ),
        ],
    ),
    (
        "std/src/sys/args/mod.rs",
        [
            (
                '    target_os = "hermit",',
                '    target_os = "hermit",\n    target_os = "myos",',
            ),
            (
                '        target_os = "hermit",\n    ) => {\n        mod unix;\n        pub use unix::*;\n    }',
                '        target_os = "hermit",\n    ) => {\n        mod unix;\n        pub use unix::*;\n    }\n    target_os = "myos" => {\n        mod myos;\n        pub use myos::*;\n    }',
            ),
        ],
    ),
    (
        "std/src/sys/args/unix.rs",
        [
            (
                '#[cfg(target_os = "hermit")]\nuse crate::os::hermit::ffi::OsStringExt;\n#[cfg(not(target_os = "hermit"))]\nuse crate::os::unix::ffi::OsStringExt;',
                '#[cfg(target_os = "hermit")]\nuse crate::os::hermit::ffi::OsStringExt;\n#[cfg(target_os = "myos")]\nuse crate::os::myos::ffi::OsStringExt;\n#[cfg(not(any(target_os = "hermit", target_os = "myos")))]\nuse crate::os::unix::ffi::OsStringExt;',
            ),
            (
                '    target_os = "hermit",',
                '    target_os = "hermit",\n    target_os = "myos",',
            ),
        ],
    ),
    (
        "std/src/sys/fd/mod.rs",
        [
            (
                '    target_os = "hermit" => {\n        mod hermit;\n        pub use hermit::*;\n    }',
                '    target_os = "hermit" => {\n        mod hermit;\n        pub use hermit::*;\n    }\n    target_os = "myos" => {\n        mod myos;\n        pub use myos::*;\n    }',
            ),
        ],
    ),
    (
        "std/src/sys/io/error/mod.rs",
        [
            (
                '        target_os = "trusty",\n    ) => {\n        mod generic;',
                '        target_os = "trusty",\n        target_os = "myos",\n    ) => {\n        mod generic;',
            ),
        ],
    ),
    (
        "std/src/sys/io/mod.rs",
        [
            (
                '        target_os = "hermit" => {\n            mod hermit;\n            pub use hermit::*;\n        }',
                '        target_os = "hermit" => {\n            mod hermit;\n            pub use hermit::*;\n        }\n        target_os = "myos" => {\n            mod unsupported;\n            pub use unsupported::*;\n        }',
            ),
        ],
    ),
    (
        "std/src/process.rs",
        [
            (
                "pub fn exit(code: i32) -> ! {\n    crate::rt::cleanup();\n    crate::sys::exit::exit(code)\n}",
                "pub fn exit(code: i32) -> ! {\n    #[cfg(not(target_os = \"myos\"))]\n    crate::rt::cleanup();\n    crate::sys::exit::exit(code)\n}",
            ),
        ],
    ),
    (
        "std/src/sys/exit.rs",
        [
            (
                '        target_os = "hermit" => {\n            unsafe { hermit_abi::exit(code) }\n        }',
                '        target_os = "hermit" => {\n            unsafe { hermit_abi::exit(code) }\n        }\n        target_os = "myos" => {\n            crate::sys::myos::abi::exit(code)\n        }',
            ),
        ],
    ),
    (
        "std/src/os/fd/mod.rs",
        [
            (
                '#[cfg(not(target_os = "trusty"))]\nmod net;',
                '#[cfg(not(any(target_os = "trusty", target_os = "myos")))]\nmod net;',
            ),
        ],
    ),
    (
        "std/src/sys/thread_local/mod.rs",
        [
            (
                '        target_os = "uefi",\n        target_os = "zkvm",',
                '        target_os = "uefi",\n        target_os = "myos",\n        target_os = "zkvm",',
            ),
            (
                '            target_os = "uefi",\n            target_os = "zkvm",',
                '            target_os = "uefi",\n            target_os = "myos",\n            target_os = "zkvm",',
            ),
        ],
    ),
    (
        "std/src/io/stdio.rs",
        [
            (
                "#[cfg(not(test))]\npub fn _print(args: fmt::Arguments<'_>) {\n    print_to(args, stdout, \"stdout\");\n}",
                "#[cfg(not(test))]\n#[cfg(target_os = \"myos\")]\npub fn _print(args: fmt::Arguments<'_>) {\n    crate::sys::stdio::print_args(args);\n}\n\n#[cfg(not(test))]\n#[cfg(not(target_os = \"myos\"))]\npub fn _print(args: fmt::Arguments<'_>) {\n    print_to(args, stdout, \"stdout\");\n}",
            ),
        ],
    ),
    (
        "std/src/sys/env/mod.rs",
        [
            (
                '    target_os = "xous",\n))]\nmod common;',
                '    target_os = "xous",\n    target_os = "myos",\n))]\nmod common;',
            ),
            (
                '    target_os = "xous" => {\n        mod xous;\n        pub use xous::*;\n    }\n    target_os = "zkvm" => {',
                '    target_os = "xous" => {\n        mod xous;\n        pub use xous::*;\n    }\n    target_os = "myos" => {\n        mod myos;\n        pub use myos::*;\n    }\n    target_os = "zkvm" => {',
            ),
        ],
    ),
    (
        "std/src/sys/paths/mod.rs",
        [
            (
                '    target_os = "motor" => {\n        mod motor;\n        #[expect(dead_code)]\n        mod unsupported;\n        mod imp {',
                '    target_os = "myos" => {\n        mod myos;\n        use myos as imp;\n    }\n    target_os = "motor" => {\n        mod motor;\n        #[expect(dead_code)]\n        mod unsupported;\n        mod imp {',
            ),
        ],
    ),
    (
        "std/src/sys/random/mod.rs",
        [
            (
                '    target_os = "zkvm" => {\n        mod zkvm;\n        pub use zkvm::fill_bytes;\n    }\n    any(',
                '    target_os = "zkvm" => {\n        mod zkvm;\n        pub use zkvm::fill_bytes;\n    }\n    target_os = "myos" => {\n        mod myos;\n        pub use myos::{fill_bytes, hashmap_random_keys};\n    }\n    any(',
            ),
            (
                '    target_os = "vexos",\n)))]',
                '    target_os = "vexos",\n    target_os = "myos"\n)))]',
            ),
        ],
    ),
    (
        "alloc/src/raw_vec/mod.rs",
        [
            (
                """        let cap = cmp::max(self.cap.as_inner() * 2, required_cap);
        let cap = cmp::max(min_non_zero_cap(elem_layout.size()), cap);

        // SAFETY:
        // - cap >= len + additional
        // - other preconditions passed to caller
        let ptr = unsafe { self.finish_grow(cap, elem_layout)? };

        // SAFETY: `finish_grow` would have failed if `cap > isize::MAX`
        unsafe { self.set_ptr_and_cap(ptr, cap) };
        Ok(())""",
                """        let cap = cmp::max(self.cap.as_inner() * 2, required_cap);
        let cap = cmp::max(min_non_zero_cap(elem_layout.size()), cap);

        // myos: first allocation on an unallocated `RawVec` must use the same
        // path as `try_allocate_in`; `finish_grow` + `set_ptr_and_cap` aborts.
        #[cfg(target_os = "myos")]
        if unsafe { self.current_memory(elem_layout).is_none() } {
            let layout = match layout_array(cap, elem_layout) {
                Ok(layout) => layout,
                Err(_) => return Err(CapacityOverflow.into()),
            };
            if layout.size() == 0 {
                let alloc = unsafe { core::mem::transmute_copy(&self.alloc) };
                *self = Self::new_in(alloc, elem_layout.alignment());
                return Ok(());
            }
            let ptr = match self.alloc.allocate(layout) {
                Ok(ptr) => ptr,
                Err(_) => return Err(AllocError { layout, non_exhaustive: () }.into()),
            };
            self.ptr = Unique::from(ptr.cast());
            self.cap = unsafe { Cap::new_unchecked(cap) };
            return Ok(());
        }

        // SAFETY:
        // - cap >= len + additional
        // - other preconditions passed to caller
        let ptr = unsafe { self.finish_grow(cap, elem_layout)? };

        // SAFETY: `finish_grow` would have failed if `cap > isize::MAX`
        unsafe { self.set_ptr_and_cap(ptr, cap) };
        Ok(())""",
            ),
            (
                """        let cap = len.checked_add(additional).ok_or(CapacityOverflow)?;

        // SAFETY: preconditions passed to caller
        let ptr = unsafe { self.finish_grow(cap, elem_layout)? };

        // SAFETY: `finish_grow` would have failed if `cap > isize::MAX`
        unsafe { self.set_ptr_and_cap(ptr, cap) };
        Ok(())""",
                """        let cap = len.checked_add(additional).ok_or(CapacityOverflow)?;

        #[cfg(target_os = "myos")]
        if unsafe { self.current_memory(elem_layout).is_none() } {
            let layout = match layout_array(cap, elem_layout) {
                Ok(layout) => layout,
                Err(_) => return Err(CapacityOverflow.into()),
            };
            if layout.size() == 0 {
                let alloc = unsafe { core::mem::transmute_copy(&self.alloc) };
                *self = Self::new_in(alloc, elem_layout.alignment());
                return Ok(());
            }
            let ptr = match self.alloc.allocate(layout) {
                Ok(ptr) => ptr,
                Err(_) => return Err(AllocError { layout, non_exhaustive: () }.into()),
            };
            self.ptr = Unique::from(ptr.cast());
            self.cap = unsafe { Cap::new_unchecked(cap) };
            return Ok(());
        }

        // SAFETY: preconditions passed to caller
        let ptr = unsafe { self.finish_grow(cap, elem_layout)? };

        // SAFETY: `finish_grow` would have failed if `cap > isize::MAX`
        unsafe { self.set_ptr_and_cap(ptr, cap) };
        Ok(())""",
            ),
        ],
    ),
]


def apply_myos_file_fd_impls(path: Path) -> None:
    marker = "MYOS_FILE_FD_IMPLS"
    text = path.read_text()
    if marker in text:
        return
    impls = f'''
// {marker}
#[stable(feature = "io_safety", since = "1.63.0")]
#[cfg(target_os = "myos")]
impl AsFd for crate::fs::File {{
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {{
        crate::sys::AsInner::as_inner(self).as_fd()
    }}
}}

#[stable(feature = "io_safety", since = "1.63.0")]
#[cfg(target_os = "myos")]
impl From<crate::fs::File> for OwnedFd {{
    #[inline]
    fn from(file: crate::fs::File) -> OwnedFd {{
        crate::sys::IntoInner::into_inner(crate::sys::IntoInner::into_inner(
            crate::sys::IntoInner::into_inner(file),
        ))
    }}
}}

#[stable(feature = "io_safety", since = "1.63.0")]
#[cfg(target_os = "myos")]
impl From<OwnedFd> for crate::fs::File {{
    #[inline]
    fn from(owned_fd: OwnedFd) -> crate::fs::File {{
        crate::sys::FromInner::from_inner(crate::sys::FromInner::from_inner(
            crate::sys::FromInner::from_inner(owned_fd),
        ))
    }}
}}
'''
    path.write_text(text + impls)
    print(f"added myos File/Stdio fd impls in {path}")


def apply_replacements(path: Path, pairs: list[tuple[str, str]]) -> None:
    text = path.read_text()
    original = text
    for old, new in pairs:
        if old not in text:
            raise SystemExit(f"missing expected snippet in {path}:\n---\n{old}\n---")
        text = text.replace(old, new, 1)
    if text != original:
        path.write_text(text)
        print(f"patched {path}")


def patch_fd_impl_guards(path: Path) -> None:
    text = path.read_text()
    updated = text.replace(NOT_TRUSTY_OR_MYOS, NOT_TRUSTY_OR_MYOS_NEW)
    if updated != text:
        path.write_text(updated)
        print(f"patched fd impl guards in {path}")


def copy_tree(src: Path, dst: Path) -> None:
    import shutil

    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst)


def main() -> None:
    repo = Path(__file__).resolve().parents[3]
    rust_src = Path(
        sys.argv[2]
        if len(sys.argv) > 2
        else "/usr/local/rustup/toolchains/nightly-2026-07-26-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library"
    )
    patch_root = ROOT
    if not (rust_src / "std").is_dir():
        raise SystemExit(f"rust source not found at {rust_src}")

    copy_tree(rust_src, patch_root)

    for rel, pairs in REPLACEMENTS:
        apply_replacements(patch_root / rel, pairs)

    patch_fd_impl_guards(patch_root / "std/src/os/fd/raw.rs")
    patch_fd_impl_guards(patch_root / "std/src/os/fd/owned.rs")
    apply_myos_file_fd_impls(patch_root / "std/src/os/fd/owned.rs")

    overlays = [
        (repo / "toolchain/std/pal/myos", patch_root / "std/src/sys/pal/myos"),
        (repo / "toolchain/std/sys/myos", patch_root / "std/src/sys/myos"),
        (repo / "toolchain/std/sys/myos/alloc.rs", patch_root / "std/src/sys/alloc/myos.rs"),
        (repo / "toolchain/std/sys/myos/fd.rs", patch_root / "std/src/sys/fd/myos.rs"),
        (repo / "toolchain/std/sys/myos/stdio.rs", patch_root / "std/src/sys/stdio/myos.rs"),
        (repo / "toolchain/std/sys/args/myos.rs", patch_root / "std/src/sys/args/myos.rs"),
        (repo / "toolchain/std/sys/env/myos.rs", patch_root / "std/src/sys/env/myos.rs"),
        (repo / "toolchain/std/sys/paths/myos.rs", patch_root / "std/src/sys/paths/myos.rs"),
        (repo / "toolchain/std/sys/random/myos.rs", patch_root / "std/src/sys/random/myos.rs"),
        (repo / "toolchain/std/sys/fs/myos.rs", patch_root / "std/src/sys/fs/myos.rs"),
        (repo / "toolchain/std/sys/process/myos.rs", patch_root / "std/src/sys/process/myos.rs"),
        (repo / "toolchain/std/sys/time/myos.rs", patch_root / "std/src/sys/time/myos.rs"),
        (repo / "toolchain/std/os/myos", patch_root / "std/src/os/myos"),
    ]
    import shutil

    for src, dst in overlays:
        if src.is_dir():
            if dst.exists():
                shutil.rmtree(dst)
            shutil.copytree(src, dst)
        else:
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
        print(f"installed {dst.relative_to(patch_root)}")

    print(f"\nPatched tree ready at {patch_root}")


if __name__ == "__main__":
    main()
