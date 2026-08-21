//! TODO-REMOVE-M1-4 (M1-4 dependency cleanup).
//!
//! Emits the native link directives for the Diligent engine (DiligentCore +
//! NVAPI + the D3D12 system libraries).
//!
//! Why this exists: `diligent-rs`/`diligent-sys` are standalone workspaces
//! consumed as non-member path dependencies. Cargo builds their artifacts
//! under the dependency's own target directory and propagates the
//! `rustc-link-search` directives from their build scripts into the root
//! workspace's final links - but not all `rustc-link-lib` directives
//! (empirically, `static=DiligentCore` from the dependency metadata is
//! dropped on some downstream binary links while the plain `nvapi64` /
//! system-lib flags survive - M1-2 finding, reproduced for the root
//! examples in M1-3). This build script therefore re-emits the `-l` flags
//! so any final artifact that links `bevy_render` (tests, binaries,
//! examples) resolves the Diligent symbols.
//!
//! The DiligentCore static library itself is re-exported under a
//! bevy-unique name (`bevy_diligent_core.lib`) from this crate's own
//! OUT_DIR: the name cannot collide with (or be deduplicated away by) the
//! dependency metadata's `DiligentCore` entries, and the OUT_DIR
//! link-search always propagates. The copied file is byte-identical to the
//! engine's combined static library (refreshed on every rebuild via
//! `rerun-if-changed`; the up-to-date check is content-based - see
//! `copy_up_to_date`).
//!
//! M1-3 review fix round (TODO-REMOVE-M1-4): everything Windows-specific -
//! the link directives AND the engine copy mechanism - is cfg-guarded to
//! `target_os = "windows"`: non-Windows binary builds would otherwise fail
//! at link (d3d12/dxgi/d3dcompiler/shlwapi/comdlg32/nvapi64 do not exist
//! there). Cross-platform engine backends are an M1-4/M2 concern.

use std::env;
#[cfg(target_os = "windows")]
use std::fs;
#[cfg(target_os = "windows")]
use std::io::Read;
#[cfg(target_os = "windows")]
use std::path::Path;
use std::path::PathBuf;

/// Candidate locations of the combined DiligentCore static library, in
/// priority order. The diligent-sys build script puts the freshly built lib
/// either at the root of the persisted cmake build directory
/// (`third_party/diligent-build`) or under its `Release` subdirectory, and
/// refreshes a fallback copy under its OUT_DIR
/// (`target/<profile>/build/diligent-sys-*/out/diligent-build/Release`).
#[cfg(target_os = "windows")]
fn diligent_core_candidates(manifest_dir: &Path) -> Vec<PathBuf> {
    let third_party = manifest_dir
        .join("..")
        .join("..")
        .join("third_party")
        .join("diligent-build");
    let mut candidates = vec![
        third_party.join("DiligentCore.lib"),
        third_party.join("Release").join("DiligentCore.lib"),
    ];
    // The fallback copies under any diligent-sys build-script OUT_DIR.
    let target_dir = env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("..").join("..").join("target"));
    if let Ok(builds) = fs::read_dir(target_dir.join("debug").join("build")) {
        for entry in builds.flatten() {
            let release = entry
                .path()
                .join("out")
                .join("diligent-build")
                .join("Release")
                .join("DiligentCore.lib");
            if release.is_file() {
                candidates.push(release);
            }
        }
    }
    candidates
}

/// Streaming FNV-1a 64-bit content hash (std-only; the engine lib is
/// ~600 MB, so the hash reads the file in chunks).
#[cfg(target_os = "windows")]
fn content_hash(path: &Path) -> Option<u64> {
    let mut file = fs::File::open(path).ok()?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buf).ok()?;
        if read == 0 {
            break;
        }
        for &byte in &buf[..read] {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    Some(hash)
}

/// Whether the OUT_DIR copy can be reused for `source`.
///
/// M1-3 review fix round (TODO-REMOVE-M1-4): the previous check compared
/// the file LENGTHS only - a same-size engine rebuild left the stale
/// ~600 MB copy in place and the final link picked up the old engine. The
/// sizes are compared first (the cheap gate); equal sizes hash both files
/// (the copy is byte-identical to the source when current, so the hashes
/// match).
#[cfg(target_os = "windows")]
fn copy_up_to_date(copy: &Path, source: &Path) -> bool {
    if !copy.is_file() {
        return false;
    }
    if copy.metadata().ok().map(|m| m.len()) != source.metadata().ok().map(|m| m.len()) {
        return false;
    }
    content_hash(copy) == content_hash(source)
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    #[cfg(target_os = "windows")]
    emit_windows_diligent_links(&manifest_dir);

    // TODO-REMOVE-M1-4 (M1-3 review, fix 4): compile-time sanity - on
    // non-Windows hosts `emit_windows_diligent_links` is cfg'd out entirely,
    // so no Diligent link directive (nor the engine copy mechanism) can
    // reach a non-Windows final link. The engine artifacts are
    // Windows/D3D12-only in this phase; cross-platform backends are an
    // M1-4/M2 concern.
    #[cfg(not(target_os = "windows"))]
    let _ = manifest_dir;
}

/// Windows-only: re-exports DiligentCore under a bevy-unique name + search
/// path and emits the Diligent engine + D3D12 system library link
/// directives (see the module docs). TODO-REMOVE-M1-4: cfg guard added in
/// the M1-3 review fix round; the whole mechanism must move into
/// diligent-rs proper during the M1-4 dependency cleanup.
#[cfg(target_os = "windows")]
fn emit_windows_diligent_links(manifest_dir: &Path) {
    // Re-export DiligentCore under a bevy-unique name + search path (see the
    // module docs: the plain `-l static=DiligentCore` from the dependency
    // metadata is not reliable on downstream binary links).
    if let Some(source) = diligent_core_candidates(manifest_dir)
        .into_iter()
        .find(|candidate| candidate.is_file())
    {
        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
        let copy = out_dir.join("bevy_diligent_core.lib");
        // Skip the copy when an identical copy is already in place (the
        // engine lib is ~600 MB; the script re-runs on `rerun-if-changed`).
        let up_to_date = copy_up_to_date(&copy, &source);
        if up_to_date || fs::copy(&source, &copy).is_ok() {
            println!("cargo:rustc-link-search=native={}", out_dir.display());
            println!("cargo:rustc-link-lib=bevy_diligent_core");
        } else {
            println!(
                "cargo:warning=bevy_render: could not copy DiligentCore from {:?}; \
                 the final link may fail",
                source
            );
        }
        // Refresh the copy whenever the engine library changes.
        println!("cargo:rerun-if-changed={}", source.display());
    } else {
        println!(
            "cargo:warning=bevy_render: DiligentCore.lib not found; \
             the final link will fail (run the diligent-sys build first)"
        );
    }

    println!("cargo:rustc-link-lib=static=DiligentCore");
    println!("cargo:rustc-link-lib=nvapi64");
    for lib in ["d3d12", "dxgi", "d3dcompiler", "shlwapi", "comdlg32"] {
        println!("cargo:rustc-link-lib={lib}");
    }
}
