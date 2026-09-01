//! Minimal [`rustix`] stand-in for `target_os = "myos"`.
//!
//! uutils pulls rustix with many features on mainstream OSes. myos does not yet
//! expose the POSIX surface rustix expects, so this stub satisfies linkage for
//! utilities that do not call into it (e.g. echo/true/false).

#![allow(unused)]

pub mod io {
    pub type Result<T> = std::io::Result<T>;
}

pub mod fs {}
pub mod net {}
pub mod pipe {}
pub mod process {}
pub mod time {}
pub mod fd {}
