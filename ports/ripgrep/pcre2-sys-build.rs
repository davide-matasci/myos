// myos: prefer prebuilt PCRE2 via PCRE2_LIB_DIR/PCRE2_INCLUDE_DIR; disable JIT on myos.
// Based on upstream pcre2-sys 0.2.10 build.rs (Unlicense/MIT).

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=PCRE2_SYS_STATIC");
    println!("cargo:rerun-if-env-changed=PCRE2_LIB_DIR");
    println!("cargo:rerun-if-env-changed=PCRE2_INCLUDE_DIR");

    let target = std::env::var("TARGET").unwrap();

    if let (Ok(lib), Ok(inc)) = (std::env::var("PCRE2_LIB_DIR"), std::env::var("PCRE2_INCLUDE_DIR")) {
        println!("cargo:rustc-link-search=native={lib}");
        println!("cargo:rustc-link-lib=static=pcre2-8");
        println!("cargo:include={inc}");
        println!("cargo:root={lib}");
        return;
    }

    let want_static = pcre2_sys_static().unwrap_or(target.contains("musl") || target.contains("myos"));
    if !want_static && pkg_config::probe_library("libpcre2-8").is_ok() {
        return;
    }

    let upstream = PathBuf::from("upstream");
    let mut builder = cc::Build::new();
    builder
        .define("PCRE2_CODE_UNIT_WIDTH", "8")
        .define("HAVE_STDLIB_H", "1")
        .define("HAVE_MEMMOVE", "1")
        .define("HAVE_CONFIG_H", "1")
        .define("PCRE2_STATIC", "1")
        .define("STDC_HEADERS", "1")
        .define("SUPPORT_PCRE2_8", "1")
        .define("SUPPORT_UNICODE", "1");
    if target.contains("windows") {
        builder.define("HAVE_WINDOWS_H", "1");
    }
    enable_jit(&target, &mut builder);

    builder
        .include(upstream.join("src"))
        .include(upstream.join("deps"))
        .include(upstream.join("include"));
    for result in std::fs::read_dir(upstream.join("src")).unwrap() {
        let dent = result.unwrap();
        let path = dent.path();
        if path.extension().map_or(true, |ext| ext != "c") {
            continue;
        }
        if path.ends_with("pcre2_jit_match.c")
            || path.ends_with("pcre2_jit_misc.c")
            || path.ends_with("pcre2_ucptables.c")
        {
            continue;
        }
        builder.file(path);
    }

    if std::env::var("PCRE2_SYS_DEBUG").unwrap_or(String::new()) == "1"
        || std::env::var("DEBUG").unwrap_or(String::new()) == "1"
    {
        builder.debug(true);
    }
    builder.compile("libpcre2.a");
}

fn pcre2_sys_static() -> Option<bool> {
    match std::env::var("PCRE2_SYS_STATIC") {
        Err(_) => None,
        Ok(s) => {
            if s == "1" {
                Some(true)
            } else if s == "0" {
                Some(false)
            } else {
                None
            }
        }
    }
}

fn enable_jit(target: &str, builder: &mut cc::Build) {
    if target.contains("myos") {
        return;
    }
    if target == "aarch64-linux-android" {
        return;
    }
    if target == "armv7-linux-androideabi" {
        return;
    }
    if target == "aarch64-unknown-linux-musl" {
        return;
    }
    if target.contains("musleabi") {
        return;
    }
    if target.contains("apple-ios") || target.contains("apple-tvos") {
        return;
    }
    builder.define("SUPPORT_JIT", "1");
}
