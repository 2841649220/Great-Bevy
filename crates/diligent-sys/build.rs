//! Build script for `diligent-sys`.
//!
//! Two jobs:
//!
//! 1. Build the DiligentCore native library with CMake (D3D12 backend on
//!    Windows) and emit link directives for the resulting combined static
//!    library plus the required system libraries.
//! 2. Run bindgen over the Diligent C API headers and write `bindings.rs`
//!    into OUT_DIR.
//!
//! The cmake step is attempted for real by default. Set `DILIGENT_SKIP_CMAKE=1`
//! to decouple bindgen verification from the native build (useful when the
//! cmake dependency chain is blocked); the default path still tries the real
//! build and fails the build with a BLOCKED report if it cannot complete.
//!
//! When a ThirdParty submodule checkout is empty or incomplete, the missing
//! tarball is fetched from `https://codeload.github.com/DiligentGraphics/<name>`
//! (the only allowed download channel) and extracted in place, then cmake is
//! retried once.
//!
//! # Archiver switch (`DILIGENT_RS_ARCHIVER`)
//!
//! The Diligent "Archiver" (offline serialization device + `IArchiver`, the
//! write side of the PSO disk cache, `Graphics/Archiver`) is built as a
//! separate static library (`Diligent-Archiver-static`) and its interface
//! headers are NOT part of the default bindgen set, so the default build
//! keeps it disabled (`-DDILIGENT_NO_ARCHIVER=ON`, matching the original
//! CMakeCache). Set `DILIGENT_RS_ARCHIVER=1` to:
//!
//! 1. flip cmake to `-DDILIGENT_NO_ARCHIVER=OFF` (reconfigures the shared
//!    `third_party/diligent-build` tree in place; a later default build flips
//!    it back),
//! 2. build the `Diligent-Archiver-static` target and link `Archiver.lib`
//!    (before `DiligentCore.lib`, whose merged objects satisfy the archiver's
//!    references to Common/ShaderTools/GraphicsEngineD3D12),
//! 3. add the `Graphics/Archiver/interface/*.h` headers to the bindgen set so
//!    the regenerated `bindings.rs` contains `IArchiverFactory`,
//!    `ISerializationDevice`, `ISerializedPipelineState`, `IArchiver` and the
//!    `Diligent_GetArchiverFactory` entry point.
//!
//! The runtime load side (IDearchiver/IDataBlob/IEngineFactory::CreateDataBlob)
//! lives in `GraphicsEngine/interface` and is therefore always bound.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DILIGENT_MANIFEST_REL: &str = "../../third_party/DiligentEngine";

// ---------------------------------------------------------------------------
// Codeload submodule backfill
// ---------------------------------------------------------------------------

/// Codeload ref used to fetch missing submodule tarballs. `refs/heads/master`
/// is the default; pin it to a concrete commit SHA
/// (`https://codeload.github.com/DiligentGraphics/<repo>/tar.gz/<sha>`) for a
/// repo once a verified-good commit is recorded, and note the URL in the
/// checksum manifest (see `record_checksum`).
fn codeload_ref(_repo: &str) -> &'static str {
    "refs/heads/master"
}

/// Manifest file recording the URL and SHA-256 of every codeload tarball this
/// build script has backfilled. Lives next to the DiligentEngine checkout
/// (`third_party/diligent-fetched.sha256`), not inside it, so writing it does
/// not re-trigger `cargo:rerun-if-changed`.
fn checksums_manifest(diligent_dir: &Path) -> PathBuf {
    diligent_dir
        .parent()
        .map(|p| p.join("diligent-fetched.sha256"))
        .unwrap_or_else(|| diligent_dir.join("diligent-fetched.sha256"))
}

/// Read `<sha256>  <repo>  <url>` lines from the checksum manifest.
fn read_checksums(path: &Path) -> Vec<(String, String, String)> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let sha = parts.next()?;
            let repo = parts.next()?;
            let url = parts.next().unwrap_or_default();
            Some((sha.to_string(), repo.to_string(), url.to_string()))
        })
        .collect()
}

/// Record (or update) the checksum entry for `repo` in the manifest.
fn record_checksum(
    path: &Path,
    repo: &str,
    url: &str,
    sha: &str,
) -> Result<(), String> {
    let mut entries = read_checksums(path);
    entries.retain(|(_, r, _)| r != repo);
    entries.push((sha.to_string(), repo.to_string(), url.to_string()));
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    let mut out = String::from(
        "# diligent-sys codeload backfill checksums\n\
         # format: <sha256>  <repo>  <url>\n",
    );
    for (sha, repo, url) in entries {
        out.push_str(&format!("{sha}  {repo}  {url}\n"));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {parent:?}: {e}"))?;
    }
    fs::write(path, out).map_err(|e| format!("write {path:?}: {e}"))
}

/// SHA-256 of a file via `certutil -hashfile` (built into Windows; the
/// PowerShell `Get-FileHash` cmdlet is not available on all PS versions).
fn sha256_file(path: &Path) -> Result<String, String> {
    let out = Command::new("certutil")
        .arg("-hashfile")
        .arg(path)
        .arg("SHA256")
        .output()
        .map_err(|e| format!("certutil sha256 failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "sha256 failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let hash = stdout
        .lines()
        .map(str::trim)
        .find(|l| l.len() == 64 && l.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| format!("unexpected certutil output: {stdout:?}"))?;
    Ok(hash.to_lowercase())
}

/// Repository name derived from a `.gitmodules` url.
fn repo_from_url(url: &str) -> String {
    url.trim_end_matches(".git")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Submodule entries parsed from `.gitmodules`: (directory, repository name).
fn gitmodules_entries(diligent_dir: &Path) -> Vec<(String, String)> {
    let gm_path = diligent_dir.join(".gitmodules");
    let content = match fs::read_to_string(&gm_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut entries = Vec::new();
    let mut dir = String::new();
    let mut url = String::new();
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("[submodule") {
            if !dir.is_empty() && !url.is_empty() {
                entries.push((dir.clone(), url.clone()));
            }
            dir.clear();
            url.clear();
        } else if let Some(rest) = line.strip_prefix("path = ") {
            dir = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("url = ") {
            url = rest.to_string();
        }
    }
    if !dir.is_empty() && !url.is_empty() {
        entries.push((dir, url));
    }
    entries
}

/// Heuristic "this submodule checkout looks usable" check.
///
/// A non-empty directory is treated as a usable checkout: some repos in
/// `.gitmodules` legitimately have neither a root CMakeLists.txt nor an
/// `include/` dir (e.g. xxHash keeps `xxhash.c`/`xxhash.h` at the root), and
/// the existing checkout may be pinned to a specific commit. Only truly empty
/// directories (missing submodules) are backfilled from `master` tarballs.
fn submodule_usable(dir: &Path) -> bool {
    fs::read_dir(dir).map(|mut rd| rd.next().is_some()).unwrap_or(false)
}

/// Fetch a `<DiligentGraphics>/<repo>` tarball via codeload and extract it
/// into `dest` (replacing whatever is there, typically an empty dir). The
/// tarball URL and its SHA-256 are recorded in `manifest`; on a later build,
/// `ensure_submodules` skips the refetch while both the manifest entry and
/// the extracted checkout are present.
fn fetch_codeload_tarball(
    repo: &str,
    dest: &Path,
    manifest: &Path,
) -> Result<(), String> {
    let url = format!(
        "https://codeload.github.com/DiligentGraphics/{}/tar.gz/{}",
        repo,
        codeload_ref(repo)
    );
    println!("cargo:warning=diligent-sys: fetching missing submodule {repo} from {url}");
    let (resp, sha256) = reqwest_blocking(&url)?;
    let decoder = flate2::read::GzDecoder::new(resp.as_slice());
    let mut archive = tar::Archive::new(decoder);
    // The tarball has a single top-level dir named <repo>-master; strip it.
    let mut strip = 1usize;
    let entries: Vec<_> = archive
        .entries()
        .map_err(|e| format!("failed to list tarball {repo}: {e}"))?
        .filter_map(|e| e.ok())
        .collect();
    if let Some(first) = entries.first() {
        let path = first.path().unwrap_or_default();
        strip = path.components().count();
    }
    if dest.exists() {
        fs::remove_dir_all(dest).map_err(|e| format!("failed to clear {dest:?}: {e}"))?;
    }
    fs::create_dir_all(dest).map_err(|e| format!("failed to create {dest:?}: {e}"))?;
    let decoder = flate2::read::GzDecoder::new(resp.as_slice());
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(false);
    archive.set_overwrite(true);
    for entry in archive.entries().map_err(|e| format!("tarball {repo}: {e}"))? {
        let mut entry = entry.map_err(|e| format!("tarball {repo}: {e}"))?;
        let rel = {
            let path = entry.path().map_err(|e| format!("tarball {repo}: {e}"))?;
            let comps: Vec<_> = path.components().skip(strip.min(path.components().count())).collect();
            comps.iter().collect::<PathBuf>()
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out_path = dest.join(&rel);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {parent:?}: {e}"))?;
        }
        entry
            .unpack(&out_path)
            .map_err(|e| format!("extract {repo} -> {out_path:?}: {e}"))?;
    }
    if !submodule_usable(dest) {
        return Err(format!(
            "fetched {repo} but the result does not look like a usable checkout: {dest:?}"
        ));
    }
    record_checksum(manifest, repo, &url, &sha256)
        .map_err(|e| format!("failed to record checksum for {repo}: {e}"))?;
    println!(
        "cargo:warning=diligent-sys: submodule {repo} backfilled into {dest:?} \
         (sha256 {sha256})"
    );
    Ok(())
}

/// Minimal HTTP GET via PowerShell (no extra Rust dependencies); the payload
/// is written to a temp file by the child process, read back, and hashed.
/// Returns the payload bytes and the SHA-256 of the downloaded tarball.
fn reqwest_blocking(url: &str) -> Result<(Vec<u8>, String), String> {
    let tmp = env::temp_dir().join(format!(
        "diligent-sys-dl-{}.bin",
        std::process::id()
    ));
    let script = format!(
        "[System.Net.ServicePointManager]::SecurityProtocol=[System.Net.SecurityProtocolType]::Tls12; \
         $ProgressPreference='SilentlyContinue'; \
         Invoke-WebRequest -Uri '{url}' -UseBasicParsing -TimeoutSec 120 -OutFile '{}'",
        tmp.display(),
    );
    let out = Command::new("powershell")
        .arg("-NoProfile")
        .arg("-Command")
        .arg(script)
        .output()
        .map_err(|e| format!("powershell downloader failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "download failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let buf = fs::read(&tmp).map_err(|e| format!("read download: {e}"))?;
    let sha256 = sha256_file(&tmp)?;
    let _ = fs::remove_file(&tmp);
    if buf.is_empty() {
        return Err(format!("empty download from {url}"));
    }
    Ok((buf, sha256))
}

/// Ensure every submodule listed in `.gitmodules` has a usable checkout,
/// fetching via codeload when it does not. A usable checkout with a recorded
/// manifest entry (URL + SHA-256 on file) is never refetched. Returns the
/// list of fetched repos.
fn ensure_submodules(diligent_dir: &Path) -> Result<Vec<String>, String> {
    let manifest = checksums_manifest(diligent_dir);
    let recorded: std::collections::HashSet<String> =
        read_checksums(&manifest).into_iter().map(|(_, r, _)| r).collect();
    let mut fetched = Vec::new();
    let mut unrecorded = Vec::new();
    for (rel_path, url) in gitmodules_entries(diligent_dir) {
        let dir = diligent_dir.join(&rel_path);
        let repo = repo_from_url(&url);
        if repo.is_empty() {
            return Err(format!(
                "cannot derive repo name from submodule url {url} (dir {rel_path})"
            ));
        }
        if submodule_usable(&dir) {
            // Verify-on-reuse: a usable checkout that was backfilled by this
            // script has a manifest entry; otherwise its provenance predates
            // checksum recording and it is left untouched (never refetched).
            if !recorded.contains(&repo) {
                unrecorded.push(repo);
            }
            continue;
        }
        fetch_codeload_tarball(&repo, &dir, &manifest)?;
        fetched.push(repo);
    }
    if !unrecorded.is_empty() {
        println!(
            "cargo:warning=diligent-sys: usable submodules without a recorded codeload \
             checksum (pre-dates checksum recording; left as-is): {}",
            unrecorded.join(", ")
        );
    }
    Ok(fetched)
}

// ---------------------------------------------------------------------------
// Windows MSVC toolchain discovery
// ---------------------------------------------------------------------------

struct MsvcToolchain {
    cl: PathBuf,
    include: String,
    lib: String,
    bin_dir: PathBuf,
}

fn dirs_with(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(path) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                out.push(e.path());
            }
        }
    }
    out
}

fn find_highest_matching(root: &Path, probe: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    for candidate in dirs_with(root) {
        if !candidate.join(probe).exists() {
            continue;
        }
        let ver = candidate
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.split('.').next().and_then(|m| m.parse::<u64>().ok()))
            .unwrap_or(0);
        if best.as_ref().map(|(v, _)| ver > *v).unwrap_or(true) {
            best = Some((ver, candidate.clone()));
        }
    }
    best.map(|(_, p)| p)
}

fn find_windows_msvc() -> Result<MsvcToolchain, String> {
    // 1. MSVC tools directory.
    let mut msvc_tools_roots: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = env::var("DILIGENT_MSVC_TOOLS_DIR") {
        msvc_tools_roots.push(PathBuf::from(dir));
    }
    for root in [
        "C:/builder/tools/VC/Tools/MSVC",
        "C:/Program Files/Microsoft Visual Studio/2022/Enterprise/VC/Tools/MSVC",
        "C:/Program Files/Microsoft Visual Studio/2022/Professional/VC/Tools/MSVC",
        "C:/Program Files/Microsoft Visual Studio/2022/Community/VC/Tools/MSVC",
        "C:/Program Files (x86)/Microsoft Visual Studio/2022/Enterprise/VC/Tools/MSVC",
        "C:/Program Files (x86)/Microsoft Visual Studio/2022/Professional/VC/Tools/MSVC",
        "C:/Program Files (x86)/Microsoft Visual Studio/2022/Community/VC/Tools/MSVC",
    ] {
        msvc_tools_roots.push(PathBuf::from(root));
    }
    let mut msvc_dir = None;
    for root in &msvc_tools_roots {
        if let Some(d) = find_highest_matching(root, Path::new("bin/Hostx64/x64/cl.exe")) {
            msvc_dir = Some(d);
            break;
        }
    }
    let msvc_dir = msvc_dir.ok_or_else(|| {
        "MSVC not found: no cl.exe under any searched VC/Tools/MSVC root. \
         Set DILIGENT_MSVC_TOOLS_DIR to the MSVC tools directory."
            .to_string()
    })?;
    let cl = msvc_dir.join("bin/Hostx64/x64/cl.exe");
    let bin_dir = msvc_dir.join("bin/Hostx64/x64");

    // 2. Windows SDK.
    let mut sdk_roots: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = env::var("DILIGENT_WINDOWS_KITS_DIR") {
        sdk_roots.push(PathBuf::from(dir));
    }
    sdk_roots.push(PathBuf::from("C:/Program Files (x86)/Windows Kits/10"));
    sdk_roots.push(PathBuf::from("C:/Program Files/Windows Kits/10"));
    let mut sdk_dir = None;
    for root in &sdk_roots {
        if let Some(d) = find_highest_matching(&root.join("Include"), Path::new("um/d3d12.h")) {
            sdk_dir = Some(d);
            break;
        }
    }
    let sdk_dir = sdk_dir.ok_or_else(|| {
        "Windows SDK not found: no um/d3d12.h under any searched Windows Kits root. \
         Set DILIGENT_WINDOWS_KITS_DIR to the Windows Kits/10 directory."
            .to_string()
    })?;

    let mut include = String::new();
    let mut lib = String::new();
    for dir in [
        msvc_dir.join("include"),
        msvc_dir.join("atlmfc/include"),
        sdk_dir.join("ucrt"),
        sdk_dir.join("um"),
        sdk_dir.join("shared"),
        sdk_dir.join("winrt"),
    ] {
        if dir.is_dir() {
            include.push_str(&format!("{};", dir.display()));
        }
    }
    // sdk_dir is <kits>/Include/<version>; the import libs live under the
    // sibling <kits>/Lib/<version> tree.
    let kits_root = sdk_dir.parent().and_then(|p| p.parent()).unwrap_or(&sdk_dir);
    let sdk_lib_dir = kits_root.join("Lib").join(
        sdk_dir.file_name().unwrap_or_default(),
    );
    for dir in [
        msvc_dir.join("lib/x64"),
        msvc_dir.join("atlmfc/lib/x64"),
        sdk_lib_dir.join("ucrt/x64"),
        sdk_lib_dir.join("um/x64"),
    ] {
        if dir.is_dir() {
            lib.push_str(&format!("{};", dir.display()));
        }
    }
    Ok(MsvcToolchain { cl, include, lib, bin_dir })
}

// ---------------------------------------------------------------------------
// cmake build
// ---------------------------------------------------------------------------

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    println!("cargo:warning=diligent-sys: {what}");
    let out = cmd
        .output()
        .map_err(|e| format!("failed to spawn {what}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        let tail: String = stdout
            .lines()
            .chain(stderr.lines())
            .rev()
            .take(60)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "{what} failed ({}):\n--- tail ---\n{tail}\n--- end tail ---",
            out.status
        ));
    }
    println!("cargo:warning=diligent-sys: {what} ok");
    Ok(())
}

fn build_diligent_core(
    diligent_dir: &Path,
    out_dir: &Path,
    archiver: bool,
) -> Result<(PathBuf, Option<PathBuf>, Option<PathBuf>), String> {
    let tc = find_windows_msvc()?;

    let cmake = env::var("DILIGENT_CMAKE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("cmake"));

    // Locate ninja (optional; cmake can fall back to its own detection).
    let ninja = env::var("DILIGENT_NINJA").ok().map(PathBuf::from);
    let ninja = ninja.or_else(|| {
        Command::new("where")
            .arg("ninja")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8(o.stdout)
                    .ok()
                    .and_then(|s| s.lines().next().map(str::to_string))
                    .map(PathBuf::from)
            })
    });

    // Build directory lives OUTSIDE OUT_DIR so the expensive cmake build
    // (including FetchContent downloads like NVAPI) survives env-var changes
    // (DILIGENT_SKIP_CMAKE etc.) that cause cargo to rerun the build script
    // with a different OUT_DIR hash. Placing it next to the DiligentEngine
    // checkout keeps it consistent regardless of which crate depends on
    // diligent-sys.
    let build_dir = diligent_dir
        .parent()
        .unwrap_or(diligent_dir)
        .join("diligent-build");
    let install_dir = out_dir.join("diligent-install");
    fs::create_dir_all(&build_dir).map_err(|e| format!("mkdir {build_dir:?}: {e}"))?;

    // ---------------------------------------------------------------
    // Step 1: backfill missing submodules (codeload only), then configure.
    // ---------------------------------------------------------------
    let fetched = ensure_submodules(diligent_dir)?;
    if !fetched.is_empty() {
        println!(
            "cargo:warning=diligent-sys: backfilled submodules: {}",
            fetched.join(", ")
        );
    }

    let configure = |build_dir: &Path| -> Result<(), String> {
        let mut cmd = Command::new(&cmake);
        cmd.arg("-S").arg(diligent_dir).arg("-B").arg(build_dir);
        cmd.arg("-G").arg("Ninja");
        cmd.arg("-DCMAKE_BUILD_TYPE=Release");
        cmd.arg(format!("-DCMAKE_C_COMPILER={}", tc.cl.display()));
        cmd.arg(format!("-DCMAKE_CXX_COMPILER={}", tc.cl.display()));
        if let Some(n) = &ninja {
            cmd.arg(format!("-DCMAKE_MAKE_PROGRAM={}", n.display()));
        }
        // Compile-only checks: the D3D12/ATL capability probes only include
        // headers, so we do not need to link them.
        cmd.arg("-DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY");
        // Minimal backend set: D3D12 only.
        cmd.arg("-DDILIGENT_NO_DIRECT3D11=ON");
        cmd.arg("-DDILIGENT_NO_VULKAN=ON");
        cmd.arg("-DDILIGENT_NO_OPENGL=ON");
        cmd.arg("-DDILIGENT_NO_SUPER_RESOLUTION=ON");
        // Archiver (PSO disk-cache write side) is disabled by default; the
        // DILIGENT_RS_ARCHIVER=1 switch flips it on (see module docs).
        if archiver {
            cmd.arg("-DDILIGENT_NO_ARCHIVER=OFF");
        } else {
            cmd.arg("-DDILIGENT_NO_ARCHIVER=ON");
        }
        // No tests/tools/samples (they are not part of DiligentCore anyway).
        cmd.arg("-DDILIGENT_BUILD_TESTS=OFF");
        cmd.arg("-DDILIGENT_BUILD_TOOLS=OFF");
        cmd.arg("-DDILIGENT_BUILD_SAMPLES=OFF");
        cmd.arg("-DDILIGENT_USE_OPENXR=OFF");
        cmd.arg("-DDILIGENT_INSTALL_CORE=ON");
        cmd.arg(format!("-DCMAKE_INSTALL_PREFIX={}", install_dir.display()));
        // abseil-cpp is pulled in unconditionally by ThirdParty/CMakeLists.txt
        // via FetchContent from chromium.googlesource.com (unreachable here,
        // and nothing in DiligentCore links abseil). Pointing the FetchContent
        // source dir at the local stub makes the population a no-op.
        cmd.arg(format!(
            "-DFETCHCONTENT_SOURCE_DIR_ABSEIL-CPP={}",
            diligent_dir.join("ThirdParty/abseil-cpp").display()
        ));
        // NVAPI: the D3D12 backend FetchContent-declares nvapi from
        // https://github.com/NVIDIA/nvapi.  DILIGENT_NVAPI_PATH bypasses
        // FetchContent entirely when the source is already available
        // locally (copied to third_party/nvapi-src on first successful
        // build).  Falls back to FetchContent download if not present.
        let nvapi_local = diligent_dir
            .parent()
            .unwrap_or(diligent_dir)
            .join("nvapi-src");
        if nvapi_local.join("nvapi.h").exists() {
            cmd.arg(format!(
                "-DDILIGENT_NVAPI_PATH={}",
                nvapi_local.display()
            ));
        }
        cmd.env("INCLUDE", &tc.include);
        cmd.env("LIB", &tc.lib);
        let mut path = tc.bin_dir.display().to_string();
        if let Some(n) = &ninja {
            if let Some(dir) = n.parent() {
                path.push(';');
                path.push_str(&dir.display().to_string());
            }
        }
        path.push(';');
        path.push_str(&env::var("PATH").unwrap_or_default());
        cmd.env("PATH", path);
        run(&mut cmd, "cmake configure (DiligentCore, D3D12-only, Ninja/Release)")
    };

    // First attempt with the current (possibly partially populated) tree.
    if let Err(e) = configure(&build_dir) {
        // Retry once after a forced backfill of every missing submodule.
        println!("cargo:warning=diligent-sys: configure failed ({e}); retrying after submodule backfill");
        let _ = ensure_submodules(diligent_dir);
        if let Err(e2) = configure(&build_dir) {
            return Err(format!(
                "cmake configure BLOCKED. First attempt: {e}\nSecond attempt: {e2}"
            ));
        }
    }

    // ---------------------------------------------------------------
    // Step 2: build. Only the combined static lib target is requested:
    // building everything would also build the shared (DLL) backend targets
    // that are the default on Windows but are not needed here. With the
    // archiver switch on, Diligent-Archiver-static is built as well (a small
    // separate static lib; its interface headers are added to bindgen below).
    // ---------------------------------------------------------------
    let mut cmd = Command::new(&cmake);
    cmd.arg("--build").arg(&build_dir);
    cmd.arg("--target").arg("DiligentCore-static");
    if archiver {
        cmd.arg("Diligent-Archiver-static");
    }
    cmd.env("INCLUDE", &tc.include);
    cmd.env("LIB", &tc.lib);
    let mut path = tc.bin_dir.display().to_string();
    if let Some(n) = &ninja {
        if let Some(dir) = n.parent() {
            path.push(';');
            path.push_str(&dir.display().to_string());
        }
    }
    path.push(';');
    path.push_str(&env::var("PATH").unwrap_or_default());
    cmd.env("PATH", path);
    run(&mut cmd, "cmake --build (DiligentCore static libs)")?;

    // ---------------------------------------------------------------
    // Step 3: locate the combined static library.
    // ---------------------------------------------------------------
    let mut found: Option<PathBuf> = None;
    for entry in walkdir_files(&build_dir) {
        let name = entry
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name == "DiligentCore.lib" || name == "libDiligentCore.a" {
            found = Some(entry);
            break;
        }
    }
    let lib = found.ok_or_else(|| {
        "BLOCKED: cmake build succeeded but the combined DiligentCore static library was not \
         found under the build directory"
            .to_string()
    })?;

    // Refresh the legacy fallback copy at <out_dir>/diligent-build/Release/
    // DiligentCore.lib: an earlier build-script layout built cmake inside
    // OUT_DIR, and crates/diligent-rs/build.rs re-links DiligentCore through
    // that path (`find_release_dir`). Keeping it in sync makes the final link
    // use this freshly built lib (a stale copy there silently links an older
    // engine ABI, which breaks e.g. the dearchiver vtable layout vs. the
    // current interface headers).
    let legacy_release = out_dir.join("diligent-build").join("Release");
    if let Err(e) = fs::create_dir_all(&legacy_release) {
        println!(
            "cargo:warning=diligent-sys: cannot create legacy fallback dir {:?}: {e}",
            legacy_release
        );
    } else {
        let legacy_lib = legacy_release.join("DiligentCore.lib");
        match fs::copy(&lib, &legacy_lib) {
            Ok(_) => println!(
                "cargo:warning=diligent-sys: refreshed legacy fallback lib copy {:?}",
                legacy_lib
            ),
            Err(e) => println!(
                "cargo:warning=diligent-sys: cannot refresh legacy fallback lib copy {:?}: {e}",
                legacy_lib
            ),
        }
    }

    // ---------------------------------------------------------------
    // Step 3b: locate the archiver static library (only when the switch is
    // on). Its objects reference Diligent-Common/ShaderTools/GraphicsEngineD3D12
    // symbols, all merged into DiligentCore.lib, so the archiver lib must be
    // linked BEFORE DiligentCore.lib (the caller emits the directives in the
    // order returned).
    // ---------------------------------------------------------------
    let mut archiver_lib: Option<PathBuf> = None;
    if archiver {
        for entry in walkdir_files(&build_dir) {
            let name = entry
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name == "Diligent-Archiver-static.lib" || name == "libDiligent-Archiver-static.a" {
                archiver_lib = Some(entry);
                break;
            }
        }
        if archiver_lib.is_none() {
            return Err(
                "BLOCKED: DILIGENT_RS_ARCHIVER=1 but the Diligent-Archiver-static library \
                 (Diligent-Archiver-static.lib) was not found under the build directory"
                    .to_string(),
            );
        }
    }

    // The D3D12 backend references NvAPI_* symbols.  NVAPI can come from
    // two locations:
    //   1. FetchContent download into the build dir (_deps/nvapi-src)
    //   2. DILIGENT_NVAPI_PATH override (third_party/nvapi-src), which
    //      bypasses FetchContent entirely.
    // Search both locations for the import library.
    let nvapi = walkdir_files(&build_dir)
        .into_iter()
        .chain({
            let nvapi_local = diligent_dir
                .parent()
                .unwrap_or(diligent_dir)
                .join("nvapi-src");
            if nvapi_local.exists() {
                walkdir_files(&nvapi_local)
            } else {
                Vec::new()
            }
        })
        .find(|p| p.file_name().map(|n| n == "nvapi64.lib").unwrap_or(false));
    if nvapi.is_none() {
        println!(
            "cargo:warning=diligent-sys: nvapi64.lib not found under the build dir or nvapi-src; the final \
             link may fail on NvAPI_* symbols"
        );
    }
    Ok((lib, nvapi, archiver_lib))
}

fn walkdir_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// bindgen
// ---------------------------------------------------------------------------

fn bindgen_headers(diligent_dir: &Path, archiver: bool) -> Vec<PathBuf> {
    let interface = diligent_dir.join("Graphics/GraphicsEngine/interface");    let mut headers: Vec<PathBuf> = fs::read_dir(&interface)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().map(|e| e == "h").unwrap_or(false) &&
                    // LoadEngineDll.h unconditionally includes <Windows.h>,
                    // which collides with Diligent's C-mode `typedef Uint32
                    // BIND_FLAGS` (SDK objidl.h typedefs `BIND_FLAGS` too).
                    // It is only used for shared (DLL) engine loading, which
                    // this crate does not use (static link).
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n != "LoadEngineDll.h")
                        .unwrap_or(true)
                })
                .collect()
        })
        .unwrap_or_default();
    headers.push(diligent_dir.join("Graphics/GraphicsEngineD3D12/interface/EngineFactoryD3D12.h"));
    headers.push(diligent_dir.join("Graphics/GraphicsEngineD3D12/interface/RenderDeviceD3D12.h"));
    headers.push(diligent_dir.join("Graphics/GraphicsEngineVulkan/interface/EngineFactoryVk.h"));
    // Archiver (PSO disk-cache write side): only bound when the
    // DILIGENT_RS_ARCHIVER switch is on, matching the cmake side. The
    // ArchiverFactoryLoader.h declares the extern "C" Diligent_GetArchiverFactory
    // entry point exported by Archiver.lib.
    if archiver {
        for name in [
            "Archiver.h",
            "ArchiverFactory.h",
            "ArchiverFactoryLoader.h",
            "SerializationDevice.h",
            "SerializedPipelineState.h",
            "SerializedShader.h",
        ] {
            headers.push(diligent_dir.join("Graphics/Archiver/interface").join(name));
        }
    }
    headers.sort();
    headers.dedup();
    headers
}

/// Platform macro set passed to clang so the interface headers see the same
/// configuration they get when compiled by the engine build.
fn platform_clang_defines() -> Vec<String> {
    let mut defs: Vec<String> = Vec::new();
    #[cfg(target_os = "windows")]
    {
        defs.push("-DPLATFORM_WIN32=1".into());
        defs.push("-DD3D11_SUPPORTED=0".into());
        defs.push("-DD3D12_SUPPORTED=1".into());
        defs.push("-DGL_SUPPORTED=0".into());
        defs.push("-DGLES_SUPPORTED=0".into());
        // The Vk interface header is generated for documentation/future use
        // even though the Windows build is D3D12-only.
        defs.push("-DVULKAN_SUPPORTED=1".into());
        defs.push("-DMETAL_SUPPORTED=0".into());
        defs.push("-DWEBGPU_SUPPORTED=0".into());
        defs.push("-DDILIGENT_D3D12_SHARED=0".into());
        defs.push("-DDILIGENT_VK_SHARED=0".into());
    }
    // Linux (Vulkan) template - not verified on this machine:
    //   defs: PLATFORM_LINUX=1, VULKAN_SUPPORTED=1, D3D12_SUPPORTED=0,
    //         GL_SUPPORTED=1, GLES_SUPPORTED=0, DILIGENT_VK_SHARED=0
    //   clang args: -I <diligent>/ThirdParty/Vulkan-Headers/include
    // Android NDK template - not verified on this machine:
    //   clang from the NDK (aarch64-linux-android34-clang), defs:
    //   PLATFORM_ANDROID=1, VULKAN_SUPPORTED=1, GLES_SUPPORTED=1, ...
    //   and an -isystem for the NDK's sysroot usr/include.
    defs
}

fn generate_bindings(diligent_dir: &Path, out_dir: &Path, archiver: bool) -> Result<(), String> {
    // Make sure clang-sys can find libclang: prefer the directory of `clang`
    // on PATH, fall back to the user's LIBCLANG_PATH.
    if env::var_os("LIBCLANG_PATH").is_none() {
        if let Ok(o) = Command::new("where").arg("clang").output() {
            if o.status.success() {
                if let Some(line) = String::from_utf8(o.stdout)
                    .ok()
                    .and_then(|s| s.lines().next().map(str::to_string))
                {
                    let dir = PathBuf::from(&line);
                    if let Some(parent) = dir.parent() {
                        if parent.join("libclang.dll").exists()
                            || parent.join("libclang.so").exists()
                        {
                            unsafe { env::set_var("LIBCLANG_PATH", parent) };
                        }
                    }
                }
            }
        }
    }

    let headers = bindgen_headers(diligent_dir, archiver);
    if headers.is_empty() {
        return Err(format!(
            "no C API headers found under {diligent_dir:?}/Graphics/GraphicsEngine/interface"
        ));
    }
    let mut clang_args = platform_clang_defines();
    #[cfg(target_os = "windows")]
    {
        // The Diligent headers pull in MSVC/Windows SDK system headers
        // (corecrt.h, vcruntime.h, ...), so clang needs those include dirs.
        // Reuse the same discovery as the cmake step.
        match find_windows_msvc() {
            Ok(tc) => {
                for dir in tc.include.split(';').filter(|d| !d.is_empty()) {
                    clang_args.push("-isystem".to_string());
                    clang_args.push(dir.to_string());
                }
                // M5a (task 16.1): `RenderDeviceD3D12.h` returns raw
                // `ID3D12Device*`/`ID3D12Resource*` without including
                // d3d12.h itself (the engine's C++ sources include it first).
                // Include the forward-declaration shim instead of <d3d12.h>
                // (which would collide with Diligent's `BIND_FLAGS` typedef -
                // the same clash that excludes LoadEngineDll.h) so
                // `IRenderDeviceD3D12` + `GetD3D12Device` bind to concrete
                // pointer types.
                let shim = std::path::PathBuf::from(
                    env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
                )
                .join("d3d12_forward.h");
                clang_args.push(format!("-include{}", shim.display()));
            }
            Err(e) => println!(
                "cargo:warning=diligent-sys: MSVC discovery failed ({e}); bindgen will likely \
                 fail on system headers"
            ),
        }
    }
    let mut builder = bindgen::Builder::default()
        // Rust enums: Diligent declares explicit underlying types and explicit
        // values (DILIGENT_TYPED_ENUM / #defines), so typed enums preserve
        // values and are FFI-safe; moduleconsts would lose the type identity.
        .default_enum_style(bindgen::EnumVariation::Rust { non_exhaustive: false })
        .generate_comments(true)
        .layout_tests(true)
        .clang_args(clang_args);
    for h in &headers {
        builder = builder.header(h.to_str().ok_or_else(|| "non-utf8 header path")?);
        println!("cargo:rerun-if-changed={}", h.display());
    }
    let bindings = builder
        .generate()
        .map_err(|e| format!("bindgen failed: {e}"))?;
    let out_path = out_dir.join("bindings.rs");
    bindings
        .write_to_file(&out_path)
        .map_err(|e| format!("failed to write {out_path:?}: {e}"))?;
    println!("cargo:warning=diligent-sys: bindings written to {out_path:?}");
    Ok(())
}

// ---------------------------------------------------------------------------

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let diligent_dir = manifest_dir.join(DILIGENT_MANIFEST_REL);
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    if !diligent_dir.join("CMakeLists.txt").is_file() {
        eprintln!(
            "BLOCKED: DiligentCore checkout not found at {diligent_dir:?} \
             (expected {DILIGENT_MANIFEST_REL} relative to the crate manifest)"
        );
        std::process::exit(1);
    }

    // Rerun whenever anything under the DiligentEngine tree changes.
    println!("cargo:rerun-if-changed={}", diligent_dir.display());
    println!("cargo:rerun-if-env-changed=DILIGENT_SKIP_CMAKE");
    println!("cargo:rerun-if-env-changed=DILIGENT_RS_ARCHIVER");
    println!("cargo:rerun-if-env-changed=LIBCLANG_PATH");
    println!("cargo:rustc-check-cfg=cfg(diligent_native_linked)");

    let skip_cmake = env::var("DILIGENT_SKIP_CMAKE").map(|v| v == "1").unwrap_or(false);
    // Archiver switch: DILIGENT_RS_ARCHIVER=1 builds + links
    // Diligent-Archiver-static and adds its interface headers to bindgen.
    let archiver = env::var("DILIGENT_RS_ARCHIVER").map(|v| v == "1").unwrap_or(false);

    if !skip_cmake {
        match build_diligent_core(&diligent_dir, &out_dir, archiver) {
            Ok((lib, nvapi, archiver_lib)) => {
                let lib_dir = lib.parent().expect("lib has a parent").to_path_buf();
                println!("cargo:rustc-link-search=native={}", lib_dir.display());
                // The archiver lib must come BEFORE DiligentCore: its objects
                // reference symbols defined in the merged DiligentCore lib.
                if let Some(archiver_lib) = &archiver_lib {
                    let archiver_dir = archiver_lib
                        .parent()
                        .expect("archiver lib has a parent")
                        .to_path_buf();
                    println!("cargo:rustc-link-search=native={}", archiver_dir.display());
                    println!("cargo:rustc-link-lib=static=Diligent-Archiver-static");
                    println!(
                        "cargo:warning=diligent-sys: linking Diligent-Archiver-static from {:?} \
                         (DILIGENT_RS_ARCHIVER=1)",
                        archiver_lib.display()
                    );
                }
                println!("cargo:rustc-link-lib=static=DiligentCore");
                println!("cargo:rustc-cfg=diligent_native_linked");
                // NVAPI: the D3D12 backend references NvAPI_* symbols; the
                // nvapi64.lib (fetched by cmake into _deps) is required.
                if let Some(nvapi) = nvapi {
                    let nvapi_dir = nvapi.parent().unwrap_or(&lib_dir).to_path_buf();
                    println!("cargo:rustc-link-search=native={}", nvapi_dir.display());
                    println!("cargo:rustc-link-lib=nvapi64");
                    println!(
                        "cargo:warning=diligent-sys: linking NVAPI from {}",
                        nvapi.display()
                    );
                }
                // System libraries required by the D3D12 backend and the
                // Win32 platform layer (discovered from Diligent's own cmake
                // targets; extended empirically if the final link complains).
                for lib in ["d3d12", "dxgi", "d3dcompiler", "shlwapi", "comdlg32"] {
                    println!("cargo:rustc-link-lib={lib}");
                }
                println!("cargo:warning=diligent-sys: linking DiligentCore from {lib:?}");
            }
            Err(e) => {
                eprintln!("BLOCKED: {e}");
                eprintln!(
                    "diligent-sys: to verify bindgen independently of the native build, set \
                     DILIGENT_SKIP_CMAKE=1 (the default path always attempts the real cmake build)."
                );
                std::process::exit(1);
            }
        }
    } else {
        println!(
            "cargo:warning=diligent-sys: DILIGENT_SKIP_CMAKE=1 - skipping the cmake build of \
             DiligentCore (bindgen only); the final link will not include DiligentCore."
        );
    }

    if let Err(e) = generate_bindings(&diligent_dir, &out_dir, archiver) {
        eprintln!("BLOCKED: bindgen failed: {e}");
        std::process::exit(1);
    }
}
