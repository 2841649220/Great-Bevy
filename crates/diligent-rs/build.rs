use std::env;
use std::path::PathBuf;

fn find_release_dir() -> Option<PathBuf> {
    let manifest = env::var("CARGO_MANIFEST_DIR").ok()?;
    let mut search_root = PathBuf::from(&manifest);
    if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
        search_root = PathBuf::from(target_dir);
    } else {
        search_root.push("target");
    }
    // M1-4b-1: probe both the debug and release build-dir layouts. The old
    // hard-coded `target/debug/build` probe returned nothing on release
    // builds, silently skipping the legacy-copy link path and leaving the
    // link to fail on the fragile repo-layout fallback alone.
    for profile in ["debug", "release"] {
        let builds = search_root.join(profile).join("build");
        let Ok(entries) = std::fs::read_dir(builds) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            if name.is_some_and(|n| n.starts_with("diligent-sys-")) {
                let release = path
                    .join("out")
                    .join("diligent-build")
                    .join("Release");
                if release.join("DiligentCore.lib").exists() {
                    return Some(release);
                }
            }
        }
    }
    // Fallback: the shared persistent cmake build directory next to the
    // diligent-sys crate (<repo>/third_party/diligent-build/Release). The
    // legacy copy under the build-script out dir may be missing when this
    // script runs ahead of diligent-sys's own build script (parallel build
    // units / replayed script output), and the `-l static=` directive alone
    // is dropped on this toolchain (see main()). The shared directory always
    // holds the freshly built lib.
    let manifest_dir = PathBuf::from(manifest);
    let shared = manifest_dir
        .parent()?
        .parent()?
        .join("third_party")
        .join("diligent-build")
        .join("Release");
    shared.join("DiligentCore.lib").exists().then_some(shared)
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../third_party/diligent-build/Release/DiligentCore.lib");

    // diligent-sys's own build script emits
    // `cargo:rustc-link-lib=static=DiligentCore`, but that directive is
    // recorded in the (multi-crate-type) sys rlib metadata and is not passed
    // through to the final link on this toolchain. Re-emitting it here applies
    // it directly to this package's own targets (lib, examples, tests), which
    // is the one propagation path that is guaranteed to work. The library is
    // resolved through the link-search path below (or through diligent-sys's
    // own link-search directive, which always propagates).
    if let Some(dir) = find_release_dir() {
        println!("cargo:rustc-link-search=native={}", dir.display());
        // Pass the library to the linker directly (no metadata relay involved)
        // as a fallback for cases where the `-l static=` directive is dropped.
        println!("cargo:rustc-link-arg={}", dir.join("DiligentCore.lib").display());
    } else {
        // M1-4b-1: explicit diagnostics instead of a silent link failure -
        // list every layout this script expects.
        println!(
            "cargo:warning=DiligentCore.lib not found: expected \
             target/<debug|release>/build/diligent-sys-*/out/diligent-build/Release/ \
             or <repo>/third_party/diligent-build/Release/DiligentCore.lib; \
             the link will fail if DiligentCore.lib is missing"
        );
    }
    println!("cargo:rustc-link-lib=static=DiligentCore");
}