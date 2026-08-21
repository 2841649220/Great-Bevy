//! Vendor plugin interfaces for the rendering-backend-replacement fork
//! (M3a deliverable).
//!
//! Five plugin contracts + probe protocol + adapter templates + TAAU
//! exclusivity. This crate is the *contract* layer: it defines the trait
//! surface a vendor SDK adapter implements and the data shapes the renderer
//! hands to it, but never bundles or links any vendor SDK. An engine with no
//! vendor SDK remains fully functional through the built-in FXAA/SMAA/TAA/CAS
//! paths (spec §5.7, design §2.2.2.4, construction plan §7.1).
//!
//! Directory convention (construction §7.1.1): adapters live under
//! `plugins/<backend>/<vendor>/` (e.g. `plugins/dx12/nvidia/`,
//! `plugins/vk/arm/`). At startup the probe protocol scans the directory for
//! SDK presence + version; a hit registers the adapter, a miss logs the
//! reason and silently masks the plugin.

use bevy_app::{App, Plugin};
use bevy_render::view::ViewTarget;

// ---------------------------------------------------------------------------
// Quality tiers and input/output data shapes (construction §7.1.2)
// ---------------------------------------------------------------------------

/// Upscaler quality presets (construction §7.1.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpscaleQuality {
    /// Native resolution, ML AA only.
    NativeAA,
    /// Quality tier (highest fidelity upscale).
    Quality,
    /// Balanced tier (fidelity/performance midpoint).
    Balanced,
    /// Performance tier (higher upscale factor).
    Performance,
    /// Ultra performance tier (highest upscale factor).
    UltraPerf,
}

/// Uniform input handed to every SR/TAAU upscaler (construction §7.1.2.1).
///
/// The renderer fills these from the view's color/depth/motion-vector
/// targets plus the current jitter phase, the reactive mask and the mip
/// bias control. Adapters map them onto their SDK-specific inputs.
///
/// (Not `Debug`: `ViewTarget` is not `Debug`.)
pub struct UpscaleInput<'a> {
    /// Input color target (HDR or LDR depending on the pipeline stage).
    pub color: &'a ViewTarget,
    /// Linear depth texture view, when available.
    pub depth_texture: Option<&'a bevy_render::render_resource::TextureView>,
    /// Motion vector texture view, when available.
    pub motion_vectors: Option<&'a bevy_render::render_resource::TextureView>,
    /// Camera jitter phase (subpixel offset in pixels, current frame).
    pub jitter_phase: [f32; 2],
    /// Reactive mask (0..1 per texel; 1 = fully reactive to upscaling).
    pub reactive_mask: Option<&'a bevy_render::render_resource::TextureView>,
    /// Output mip bias to apply when sampling user textures.
    pub mip_bias: f32,
}

/// Output viewport configuration of an upscale operation.
#[derive(Debug, Clone, Copy)]
pub struct UpscaleOutput {
    /// Output size (scaled).
    pub size: (u32, u32),
}

/// Input handed to a frame-generation plugin (construction §7.1.2.2).
///
/// Frame generation presents through a proxy swapchain: the adapter owns the
/// present path and injects interpolated frames between the rendered ones.
pub struct FrameGenInput<'a> {
    /// The most recent rendered frame's target.
    pub current: &'a ViewTarget,
    /// Whether to backfill a missing frame (camera jitter history gaps).
    pub backfill: bool,
}

/// The denoiser signal set (construction §7.1.2.3; §4.4.4 input spec).
///
/// A denoiser consumes a subset of these signals; unused ones are `None`.
#[derive(Default)]
pub struct DenoiseSignals<'a> {
    /// Direct diffuse irradiance buffer (RT output, typically low-sample).
    pub direct_diffuse: Option<&'a bevy_render::render_resource::TextureView>,
    /// Indirect diffuse irradiance buffer.
    pub indirect_diffuse: Option<&'a bevy_render::render_resource::TextureView>,
    /// Direct specular irradiance buffer.
    pub direct_specular: Option<&'a bevy_render::render_resource::TextureView>,
    /// Indirect specular irradiance buffer.
    pub indirect_specular: Option<&'a bevy_render::render_resource::TextureView>,
    /// Ambient occlusion buffer.
    pub ao: Option<&'a bevy_render::render_resource::TextureView>,
    /// Specular occlusion buffer.
    pub specular_occlusion: Option<&'a bevy_render::render_resource::TextureView>,
    /// Dominant light buffer.
    pub dominant_light: Option<&'a bevy_render::render_resource::TextureView>,
    /// Linear depth buffer.
    pub linear_depth: Option<&'a bevy_render::render_resource::TextureView>,
    /// Motion vector buffer.
    pub motion_vectors: Option<&'a bevy_render::render_resource::TextureView>,
}


/// A probe hit: an SDK discovered under `plugins/<backend>/<vendor>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkProbe {
    /// Backend the adapter targets (dx12 / vk / ...).
    pub backend: String,
    /// Vendor directory name (nvidia / amd / intel / arm / ...).
    pub vendor: String,
    /// Discovered SDK version (best-effort; empty when unknown).
    pub version: Option<String>,
    /// Whether the SDK is loadable on this platform (e.g. D3D12-only SDKs
    /// on a Vulkan build report `false`).
    pub available: bool,
}

// ---------------------------------------------------------------------------
// Five plugin contracts (design §2.2.2.4)
// ---------------------------------------------------------------------------

/// Super-resolution / TAAU upscaler (DLSS SR, FSR Upscaling, XeSS-SR,
/// ARM ASR, SGSR 1/2, 星速引擎 AI 超分).
///
/// Implementations are [`Plugin`]s so they can register their render
/// resources, pipelines and systems; the upscaling system itself runs
/// `.after(Core3dSystems::PostProcess)` on the upscaling set of the
/// `Core3dSystems` chain (`bevy_core_pipeline/src/upscaling/`).
pub trait UpscalerPlugin: Plugin {
    /// The quality tiers this adapter supports.
    fn quality_modes(&self) -> &[UpscaleQuality];
    /// Runs one upscale step for the view.
    fn upscale(&mut self, input: UpscaleInput<'_>) -> UpscaleOutput;
}

/// Frame generation (`DLSS MFG/DMFG`, `FSR FG`, `XeSS FG/MFG`) via a proxy
/// swapchain.
pub trait FrameGenPlugin: Plugin {
    /// Injects/generates frames for the given input; the adapter owns the
    /// proxy swapchain present path.
    fn generate(&mut self, input: FrameGenInput<'_>);
}

/// Denoiser (DLSS Ray Reconstruction, FSR Ray Regeneration).
pub trait DenoiserPlugin: Plugin {
    /// Denoises the given signal set; each signal is optional because
    /// backends differ in what they consume.
    fn denoise(&mut self, signals: DenoiseSignals<'_>);
}

/// Latency reduction (NVIDIA Reflex, Radeon Anti-Lag 2, `XeLL`).
pub trait LatencyPlugin: Plugin {
    /// Marks the start/end of a frame in the latency chain.
    fn frame_mark(&mut self);
}

/// Pure AA (DLAA, FSR Native AA) - classified with the built-in AA family
/// (FXAA/SMAA/TAA/CAS).
pub trait PureAaPlugin: Plugin {
    /// Applies the AA pass at native resolution.
    fn apply(&mut self, input: UpscaleInput<'_>);
}

// ---------------------------------------------------------------------------
// Probe protocol (construction §7.1.1)
// ---------------------------------------------------------------------------

/// Scans the `plugins/<backend>/<vendor>/` directory convention for SDK
/// presence and returns the discovered probes (hit or miss).
pub trait PluginProbe {
    /// Backend key this probe scans (e.g. "dx12" or "vk").
    fn backend(&self) -> &str;

    /// Runs the probe over the plugin root and returns per-vendor results.
    fn probe(&self, plugin_root: &std::path::Path) -> Vec<SdkProbe>;
}

/// The built-in probe implementation: a pure directory scan that reports
/// SDK availability from a marker file/dir inside each
/// `plugins/<backend>/<vendor>/` directory. No SDK is loaded here - the
/// marker is adapter-defined (e.g. a `sdk-version.txt` written by the
/// developer when they drop the SDK in).
pub struct DirectoryProbe {
    /// Backend key (dx12 / vk / ...).
    backend: String,
    /// Marker file name inside a vendor dir that signals "SDK present".
    marker: String,
}

impl DirectoryProbe {
    /// Creates a probe for `backend` that treats a file/dir named `marker`
    /// inside a vendor directory as "SDK present".
    pub fn new(backend: impl Into<String>, marker: impl Into<String>) -> Self {
        Self {
            backend: backend.into(),
            marker: marker.into(),
        }
    }
}

impl PluginProbe for DirectoryProbe {
    fn backend(&self) -> &str {
        &self.backend
    }

    fn probe(&self, plugin_root: &std::path::Path) -> Vec<SdkProbe> {
        let backend_dir = plugin_root.join(&self.backend);
        let Ok(read) = std::fs::read_dir(&backend_dir) else {
            return Vec::new();
        };
        let mut probes = Vec::new();
        for entry in read.flatten() {
            let vendor = entry.file_name().to_string_lossy().into_owned();
            let marker_path = entry.path().join(&self.marker);
            let version = std::fs::read_to_string(&marker_path)
                .ok()
                .map(|v| v.trim().to_string());
            probes.push(SdkProbe {
                backend: self.backend.clone(),
                vendor,
                version: version.clone(),
                available: version.is_some(),
            });
        }
        probes
    }
}

// ---------------------------------------------------------------------------
// TAAU / independent-TAA exclusivity (construction §7.1.4)
// ---------------------------------------------------------------------------

/// Whether a TAAU upscaler should replace the independent TAA pass.
///
/// Every TAAU upscaler (FSR / ASR / SGSR2 / DLSS SR) carries its own temporal
/// AA; enabling both would double-AA. This mirrors the existing Camera
/// TAA/DLSS one-or-the-other exclusivity model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaaExclusivity {
    /// TAAU upscaler active: the independent TAA pass must be disabled.
    TaauActive,
    /// No TAAU upscaler: keep the independent TAA pass.
    IndependentTaa,
}

/// Resolves the TAA exclusivity decision from whether a TAAU upscaler is
/// registered for the view.
///
/// The renderer calls this when building the camera pipeline and skips the
/// standalone TAA node accordingly; the pure function keeps the model
/// testable without a GPU.
pub fn resolve_taa_exclusivity(taau_upscaler_registered: bool) -> TaaExclusivity {
    if taau_upscaler_registered {
        TaaExclusivity::TaauActive
    } else {
        TaaExclusivity::IndependentTaa
    }
}

/// Convenience app-level helpers for plugin registration (contract surface).
pub mod prelude {
    pub use super::{
        DenoiseSignals, DenoiserPlugin, DenoiserTemplate, DirectoryProbe, FrameGenInput,
        FrameGenPlugin, FrameGenTemplate, LatencyPlugin, LatencyTemplate, PluginProbe,
        PureAaPlugin, PureAaTemplate, SdkProbe, TaaExclusivity, UpscaleInput, UpscaleOutput,
        UpscaleQuality, UpscalerPlugin, UpscalerTemplate, denoiser_template, frame_gen_template,
        latency_template, pure_aa_template, resolve_taa_exclusivity, upscaler_template,
    };
}

/// Example adapter template: a no-op `UpscalerPlugin` a developer copies and
/// fills with their SDK calls (construction §7.1.3). It is *not* a real
/// adapter - it exists so the contract compiles and the wiring is
/// self-documenting.
pub struct UpscalerTemplate {
    modes: Vec<UpscaleQuality>,
}

impl Default for UpscalerTemplate {
    fn default() -> Self {
        Self {
            modes: vec![
                UpscaleQuality::Quality,
                UpscaleQuality::Balanced,
                UpscaleQuality::Performance,
            ],
        }
    }
}

impl Plugin for UpscalerTemplate {
    fn build(&self, _app: &mut App) {}
}

impl UpscalerPlugin for UpscalerTemplate {
    fn quality_modes(&self) -> &[UpscaleQuality] {
        &self.modes
    }

    fn upscale(&mut self, input: UpscaleInput<'_>) -> UpscaleOutput {
        // Template body: map the uniform inputs onto the vendor SDK here.
        let extent = input.color.main_texture().size();
        UpscaleOutput {
            size: (extent.width, extent.height),
        }
    }
}

/// Convenience constructor for the upscaler template (so the adapter template
/// is one `let` away from a working adapter).
pub fn upscaler_template() -> UpscalerTemplate {
    UpscalerTemplate::default()
}

/// Example adapter template: a no-op `FrameGenPlugin` a developer copies and
/// fills with their SDK calls (construction §7.1.3). Frame generation presents
/// through a proxy swapchain; the template documents the wiring point without
/// touching the real present path.
#[derive(Default)]
pub struct FrameGenTemplate {
    /// Whether the template would request a backfill frame.
    backfill: bool,
}

impl Plugin for FrameGenTemplate {
    fn build(&self, _app: &mut App) {}
}

impl FrameGenPlugin for FrameGenTemplate {
    fn generate(&mut self, input: FrameGenInput<'_>) {
        // Template body: hand `input.current` to the vendor FG SDK and let it
        // present through its proxy swapchain here.

        self.backfill = input.backfill;
    }
}

/// Convenience constructor for the frame-gen template.
pub fn frame_gen_template() -> FrameGenTemplate {
    FrameGenTemplate::default()
}

/// Example adapter template: a no-op `DenoiserPlugin` a developer copies and
/// fills with their SDK calls (construction §7.1.3). Each signal is optional;
/// the template simply records which signals were provided so the contract
/// wiring is self-documenting and testable.
#[derive(Default)]
pub struct DenoiserTemplate {
    /// Number of non-`None` signals the last `denoise` call received.
    provided_signals: usize,
}

impl Plugin for DenoiserTemplate {
    fn build(&self, _app: &mut App) {}
}

impl DenoiserPlugin for DenoiserTemplate {
    fn denoise(&mut self, signals: DenoiseSignals<'_>) {
        // Template body: map the signal set onto the vendor denoiser SDK here
        // (DLSS Ray Reconstruction / FSR Ray Regeneration).
        self.provided_signals = [
            signals.direct_diffuse,
            signals.indirect_diffuse,
            signals.direct_specular,
            signals.indirect_specular,
            signals.ao,
            signals.specular_occlusion,
            signals.dominant_light,
            signals.linear_depth,
            signals.motion_vectors,
        ]
        .iter()
        .filter(|s| s.is_some())
        .count();
    }
}

/// Convenience constructor for the denoiser template.
pub fn denoiser_template() -> DenoiserTemplate {
    DenoiserTemplate::default()
}

/// Example adapter template: a no-op `LatencyPlugin` a developer copies and
/// fills with their SDK calls (construction §7.1.3). It marks frame start/end
/// for the latency chain (Reflex / Anti-Lag 2 / `XeLL`).
#[derive(Default)]
pub struct LatencyTemplate {
    /// Frame mark count since construction (mirrors SDK call volume).
    frame_marks: u64,
}

impl Plugin for LatencyTemplate {
    fn build(&self, _app: &mut App) {}
}

impl LatencyPlugin for LatencyTemplate {
    fn frame_mark(&mut self) {
        // Template body: call the vendor latency SDK frame boundary here.
        self.frame_marks += 1;
    }
}

/// Convenience constructor for the latency template.
pub fn latency_template() -> LatencyTemplate {
    LatencyTemplate::default()
}

/// Example adapter template: a no-op `PureAaPlugin` a developer copies and
/// fills with their SDK calls (construction §7.1.3). Pure AA runs at native
/// resolution (DLAA / FSR Native AA).
#[derive(Default)]
pub struct PureAaTemplate {
    /// Last applied quality tier, if any.
    last_quality: Option<UpscaleQuality>,
}

impl Plugin for PureAaTemplate {
    fn build(&self, _app: &mut App) {}
}

impl PureAaPlugin for PureAaTemplate {
    fn apply(&mut self, input: UpscaleInput<'_>) {
        // Template body: hand the native-res color target to the vendor AA SDK.
        let _ = input.color;
        self.last_quality = Some(UpscaleQuality::NativeAA);
    }
}

/// Convenience constructor for the pure-AA template.
pub fn pure_aa_template() -> PureAaTemplate {
    PureAaTemplate::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_detects_present_and_absent_sdks() {
        let root =
            std::env::temp_dir().join(format!("bevy_vp_probe_{}", std::process::id()));
        let vendor_dir = root.join("dx12").join("nvidia");
        std::fs::create_dir_all(&vendor_dir).unwrap();
        std::fs::write(vendor_dir.join("sdk-version.txt"), "5.1.0\n").unwrap();
        std::fs::create_dir_all(root.join("dx12").join("amd")).unwrap();

        let probe = DirectoryProbe::new("dx12", "sdk-version.txt");
        let mut hits = probe.probe(&root);
        hits.sort_by(|a, b| a.vendor.cmp(&b.vendor));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].vendor, "amd");
        assert!(!hits[0].available, "amd has no marker -> not available");
        assert_eq!(hits[1].vendor, "nvidia");
        assert!(hits[1].available);
        assert_eq!(hits[1].version.as_deref(), Some("5.1.0"));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn probe_missing_backend_is_empty() {
        let root =
            std::env::temp_dir().join(format!("bevy_vp_empty_{}", std::process::id()));
        let probe = DirectoryProbe::new("vk", "sdk-version.txt");
        assert!(probe.probe(&root).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn quality_modes_default_template() {
        let template = upscaler_template();
        assert_eq!(template.quality_modes().len(), 3);
        assert!(template.quality_modes().contains(&UpscaleQuality::Performance));
    }

    #[test]
    fn taa_exclusivity_reflects_upscaler_registration() {
        assert_eq!(
            resolve_taa_exclusivity(true),
            TaaExclusivity::TaauActive
        );
        assert_eq!(
            resolve_taa_exclusivity(false),
            TaaExclusivity::IndependentTaa
        );
    }

    #[test]
    fn frame_gen_template_is_constructible_plugin() {
        // Contract-surface smoke: the template is a `Plugin` and `FrameGenPlugin`
        // without any GPU resource (SDK-free two-state guarantee).
        let template = frame_gen_template();
        let _: &dyn FrameGenPlugin = &template;
        assert!(!template.backfill);
    }

    #[test]
    fn denoiser_template_defaults_to_zero_signals() {
        // `DenoiseSignals::default()` carries no textures; the template must
        // observe zero provided signals without touching a GPU device.
        let mut template = denoiser_template();
        template.denoise(DenoiseSignals::default());
        assert_eq!(template.provided_signals, 0);
    }

    #[test]
    fn latency_template_counts_frame_marks() {
        let mut template = latency_template();
        template.frame_mark();
        template.frame_mark();
        template.frame_mark();
        assert_eq!(template.frame_marks, 3);
    }

    #[test]
    fn pure_aa_template_is_constructible_plugin() {
        // Contract-surface smoke: the template is a `Plugin` and `PureAaPlugin`
        // without any GPU resource.
        let template = pure_aa_template();
        let _: &dyn PureAaPlugin = &template;
        assert_eq!(template.last_quality, None);
    }
}
