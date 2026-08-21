//! M0 reference-frame capture toolchain.
//!
//! Captures deterministic reference frames from the wgpu 29.0.4 fork before
//! the wgpu path is removed in M1. The tool is layered on top of Bevy via
//! public APIs only (plugins/systems); no fork source is modified.
//!
//! Subcommands:
//! - `capture <scene>` : run a registered scene with a fixed camera rig and
//!   capture N frames as PNG + color EXR + depth EXR + JSON under
//!   `tests/reference-frames/<platform>/<scene>/`.
//! - `scenes`          : list the scene registry.
//! - `compare <a> <b>` : SSIM / PSNR / pixel-diff histogram between two images.
//! - `depth-stats <e>` : statistics of a captured depth EXR (validation aid).

#![expect(clippy::print_stdout, reason = "CLI tool.")]

mod capture;
mod depth;
mod metadata;
mod metrics;
mod scenes;
mod whitelist;

use std::{path::PathBuf, process::ExitCode};

use argh::FromArgs;
use bevy::{
    log::{info, Level, LogPlugin},
    prelude::*,
    window::{PresentMode, Window, WindowPlugin, WindowResolution},
};

pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BEVY_VERSION: &str = "0.19.0 (fork)";
pub const WGPU_VERSION: &str = "29.0.4";

#[derive(FromArgs)]
/// M0 reference-frame capture toolchain (wgpu 29.0.4 fork).
struct Cli {
    #[argh(subcommand)]
    command: Command,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum Command {
    Capture(CaptureArgs),
    Scenes(ScenesArgs),
    Compare(CompareArgs),
    DepthStats(DepthStatsArgs),
}

#[derive(FromArgs)]
#[argh(subcommand, name = "capture")]
/// Capture reference frames for a registered scene.
struct CaptureArgs {
    /// scene id from the registry (see the `scenes` subcommand)
    #[argh(positional)]
    scene: String,

    /// number of frames to capture
    #[argh(option, default = "30")]
    frames: u32,

    /// frames to skip before the first capture (shader/pipeline warmup).
    /// PBR pipelines compile asynchronously; in debug builds this can take
    /// tens of seconds of wall time (~200 frames on this machine). Frames
    /// captured before pipelines are ready are uniform (see README §4).
    #[argh(option, default = "200")]
    warmup: u32,

    /// output root directory (platform/scene subdirs are created below it)
    #[argh(option, default = "PathBuf::from(\"tests/reference-frames\")")]
    out: PathBuf,

    /// window resolution as WxH (default 1280x720)
    #[argh(option)]
    size: Option<String>,
}

#[derive(FromArgs)]
#[argh(subcommand, name = "scenes")]
/// List the scene registry.
struct ScenesArgs {}

#[derive(FromArgs)]
#[argh(subcommand, name = "compare")]
/// SSIM / PSNR / pixel-diff histogram between two same-sized images, and
/// optional whitelist verdict when `--whitelist` is provided.
struct CompareArgs {
    /// reference image (PNG or EXR)
    #[argh(positional)]
    reference: PathBuf,

    /// candidate image (PNG or EXR)
    #[argh(positional)]
    candidate: PathBuf,

    /// whitelist JSON to evaluate the comparison against (scene-level)
    #[argh(option)]
    whitelist: Option<PathBuf>,

    /// frame id (e.g. "0210") for per-frame whitelist overrides
    #[argh(option, default = "String::from(\"0000\")")]
    frame: String,
}

#[derive(FromArgs)]
#[argh(subcommand, name = "depth-stats")]
/// Print statistics of a depth EXR (R=G=B=depth) to validate depth capture.
struct DepthStatsArgs {
    /// depth EXR produced by `capture` (depth_<n>.exr)
    #[argh(positional)]
    exr: PathBuf,

    /// sample a pixel (x,y), repeatable
    #[argh(option)]
    pixel: Vec<String>,
}

fn main() -> ExitCode {
    let cli: Cli = argh::from_env();
    match cli.command {
        Command::Capture(args) => run_capture(args),
        Command::Scenes(_) => run_scenes(),
        Command::Compare(args) => run_compare(args),
        Command::DepthStats(args) => run_depth_stats(args),
    }
}

fn run_capture(args: CaptureArgs) -> ExitCode {
    let (width, height) = parse_size(args.size.as_deref());

    let Some(spec) = scenes::get(&args.scene) else {
        eprintln!(
            "error: unknown scene '{}'. Run `cargo run -p reference_frames -- scenes` to list the registry.",
            args.scene
        );
        return ExitCode::from(2);
    };
    if !spec.implemented {
        let features = if spec.required_features.is_empty() {
            "none".to_string()
        } else {
            spec.required_features.join(",")
        };
        eprintln!(
            "error: scene '{}' is registered but not implemented yet.\n  notes: {}\n  required features: {}\n  needs assets: {}",
            spec.id, spec.notes, features, spec.needs_assets,
        );
        return ExitCode::from(3);
    }

    let out_dir = args.out.join(platform_dir()).join(spec.id);
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("error: cannot create output directory {}: {e}", out_dir.display());
        return ExitCode::from(4);
    }
    if args.frames == 0 {
        eprintln!("error: --frames must be >= 1");
        return ExitCode::from(4);
    }

    info!(
        "capture {}: {}x{}, frames {} (warmup {}), out {}",
        spec.id, width, height, args.frames, args.warmup, out_dir.display()
    );

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(LogPlugin {
                level: Level::INFO,
                filter: "wgpu_core=warn,wgpu_hal=warn,naga=warn,bevy_render=warn".into(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: format!("reference_frames:{}", spec.title),
                    // Pin the scale factor so the physical surface equals the
                    // requested resolution regardless of OS display DPI.
                    resolution: WindowResolution::new(width, height).with_scale_factor_override(1.0),
                    present_mode: PresentMode::AutoNoVsync,
                    ..default()
                }),
                ..default()
            }),
    );
    (spec.setup.expect("implemented scene must have a setup fn"))(&mut app, spec.camera);
    app.add_plugins(capture::CapturePlugin::new(
        out_dir,
        spec.id,
        args.frames,
        args.warmup,
        width,
        height,
    ));
    match app.run() {
        AppExit::Success => ExitCode::SUCCESS,
        AppExit::Error(code) => ExitCode::from(code.get()),
    }
}

fn run_scenes() -> ExitCode {
    println!("{:<22} {:<8} {:<6} {:<6} notes", "id", "impl", "assets", "features");
    for spec in scenes::SCENES {
        let features = if spec.required_features.is_empty() {
            "-".to_string()
        } else {
            spec.required_features.join(",")
        };
        println!(
            "{:<22} {:<8} {:<6} {:<6} {}",
            spec.id,
            if spec.implemented { "yes" } else { "no" },
            if spec.needs_assets { "yes" } else { "no" },
            features,
            spec.notes,
        );
    }
    ExitCode::SUCCESS
}

fn run_compare(args: CompareArgs) -> ExitCode {
    let a = match image::open(&args.reference) {
        Ok(img) => img.to_rgb8(),
        Err(e) => {
            eprintln!("error: cannot load {}: {e}", args.reference.display());
            return ExitCode::from(5);
        }
    };
    let b = match image::open(&args.candidate) {
        Ok(img) => img.to_rgb8(),
        Err(e) => {
            eprintln!("error: cannot load {}: {e}", args.candidate.display());
            return ExitCode::from(5);
        }
    };
    if a.dimensions() != b.dimensions() {
        eprintln!(
            "error: dimensions differ ({}x{} vs {}x{})",
            a.width(), a.height(), b.width(), b.height()
        );
        return ExitCode::from(6);
    }
    let (w, h) = a.dimensions();
    let Some(stats) = metrics::compare_rgb(a.as_raw(), b.as_raw(), w, h) else {
        eprintln!("error: image buffers are malformed");
        return ExitCode::from(7);
    };
    println!("{}", serde_json::to_string_pretty(&stats).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}")));

    if let Some(path) = &args.whitelist {
        let whitelist: whitelist::Whitelist = match serde_json::from_str(
            &match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: cannot read whitelist {}: {e}", path.display());
                    return ExitCode::from(8);
                }
            },
        ) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("error: cannot parse whitelist {}: {e}", path.display());
                return ExitCode::from(8);
            }
        };
        let verdict = whitelist::judge(&stats, &whitelist, &args.frame);
        println!("{}", serde_json::to_string_pretty(&verdict).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}")));
        if !verdict.passed {
            eprintln!("error: whitelist verdict FAILED for scene '{}'", verdict.scene);
            return ExitCode::from(9);
        }
    }
    ExitCode::SUCCESS
}

fn run_depth_stats(args: DepthStatsArgs) -> ExitCode {
    let img = match image::open(&args.exr) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("error: cannot load {}: {e}", args.exr.display());
            return ExitCode::from(5);
        }
    };
    let rgb = img.to_rgb32f();
    let (w, h) = rgb.dimensions();
    let data: Vec<f32> = rgb.pixels().map(|p| p.0[0]).collect();
    let stats = depth::compute_stats(&data);
    let mut samples = serde_json::Map::new();
    for spec in &args.pixel {
        let Some((x, y)) = spec.split_once(',') else {
            eprintln!("error: cannot parse pixel '{spec}', expected x,y");
            return ExitCode::from(5);
        };
        let Ok(x) = x.trim().parse::<u32>() else {
            eprintln!("error: cannot parse pixel '{spec}', expected integer x,y");
            return ExitCode::from(5);
        };
        let Ok(y) = y.trim().parse::<u32>() else {
            eprintln!("error: cannot parse pixel '{spec}', expected integer x,y");
            return ExitCode::from(5);
        };
        if x >= w || y >= h {
            eprintln!("error: pixel ({x},{y}) outside {w}x{h}");
            return ExitCode::from(5);
        }
        samples.insert(
            format!("{x},{y}"),
            serde_json::json!(data[(y as usize) * w as usize + x as usize]),
        );
    }
    let out = serde_json::json!({
        "file": args.exr.to_string_lossy(),
        "width": w,
        "height": h,
        "value_range": "1.0 = near plane, 0.0 = far (reverse-Z)",
        "uniform": depth::is_uniform(&data),
        "stats": {
            "min": stats.min,
            "max": stats.max,
            "mean": stats.mean,
            "non_finite": stats.non_finite,
        },
        "pixels": samples,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&out).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
    );
    ExitCode::SUCCESS
}

fn parse_size(size: Option<&str>) -> (u32, u32) {
    let Some(size) = size else { return (1280, 720) };
    let Some((w, h)) = size.split_once('x') else {
        eprintln!("warning: cannot parse size '{size}', using 1280x720");
        return (1280, 720);
    };
    match (w.parse::<u32>(), h.parse::<u32>()) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => (w, h),
        _ => {
            eprintln!("warning: cannot parse size '{size}', using 1280x720");
            (1280, 720)
        }
    }
}

fn platform_dir() -> &'static str {
    std::env::consts::OS
}
