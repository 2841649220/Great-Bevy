//! NVIDIA Deep Learning Super Sampling (DLSS).
//!
//! M5a (task 16.3): this module is a **compile-time placeholder** after the
//! `dlss_wgpu`/`ash` chain was removed from the lockfile (wgpu-cleanup). It
//! keeps the public API surface (camera [`Dlss`] component, [`DlssProjectId`],
//! [`DlssPerfQualityMode`], the supported-resources) so consumers compile,
//! but performs no actual DLSS work. The native NGX DLSS 4.5 integration
//! (SR/MFG/DMFG/RR/Reflex/DLAA over the native `ID3D12Device` from
//! `RenderDevice::native_d3d12_device`, task 16.1) replaces these bodies in
//! task 16.2 (hardware-gated: needs an RTX device + the NVIDIA SDK).
//!
//! # Usage (unchanged from upstream)
//! 1. Enable Bevy's `dlss` feature
//! 2. During app setup, insert the `DlssProjectId` resource before `DefaultPlugins`
//! 3. Check for the presence of `Option<Res<DlssSuperResolutionSupported>>` at runtime to see if DLSS is supported on the current machine
//! 4. Add the `Dlss` component to your camera entity, optionally setting a specific `DlssPerfQualityMode` (defaults to `Auto`)
//! 5. Optionally add sharpening via `ContrastAdaptiveSharpening`
//!
//! Until the NGX backend lands (task 16.2), `DlssSuperResolutionSupported`
//! is never inserted, so the placeholder is inert at runtime.

mod extract;
mod node;
mod prepare;

use bevy_app::{App, Plugin};
use bevy_camera::Hdr;
use bevy_core_pipeline::prepass::{DepthPrepass, MotionVectorPrepass};
use bevy_ecs::prelude::*;
use bevy_math::{UVec2, Vec2};
use bevy_reflect::{reflect_remote, Reflect};
use bevy_render::{
    camera::{MipBias, TemporalJitter},
    texture::CachedTexture,
};
use std::marker::PhantomData;

/// DLSS quality presets (task 16.3: self-authored replacement for the
/// `dlss_wgpu::DlssPerfQualityMode` that used to be re-exported here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DlssPerfQualityMode {
    Auto,
    Dlaa,
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
}

impl Default for DlssPerfQualityMode {
    fn default() -> Self {
        Self::Auto
    }
}

/// DLSS feature flags (placeholder bit flags; consumed by the NGX backend
/// once task 16.2 lands). Upstream carried `dlss_wgpu::DlssFeatureFlags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DlssFeatureFlags(u32);

impl DlssFeatureFlags {
    /// Motion vectors are provided at the render resolution.
    pub const LOW_RESOLUTION_MOTION_VECTORS: Self = Self(1 << 0);
    /// Depth is inverted (D3D12 convention).
    pub const INVERTED_DEPTH: Self = Self(1 << 1);
    /// Color is HDR.
    pub const HIGH_DYNAMIC_RANGE: Self = Self(1 << 2);
    /// Exposure is supplied per-frame.
    pub const AUTO_EXPOSURE: Self = Self(1 << 3);

    /// Empty flag set.
    pub const NONE: Self = Self(0);
}

impl std::ops::BitOr for DlssFeatureFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl Default for DlssFeatureFlags {
    fn default() -> Self {
        Self::NONE
    }
}

/// Initializes DLSS support in the renderer. This must be registered before
/// [`RenderPlugin`](bevy_render::RenderPlugin).
///
/// M5a (task 16.3): the upstream implementation registered NGX instance/device
/// extensions through `raw_vulkan_init` (the wgpu escape hatch that is gone).
/// The placeholder validates the required [`DlssProjectId`] resource and does
/// nothing else; task 16.2 rewires this to the native D3D12 device path.
#[derive(Default)]
pub struct DlssInitPlugin;

impl Plugin for DlssInitPlugin {
    fn build(&self, app: &mut App) {
        let _ = app
            .world()
            .get_resource::<DlssProjectId>()
            .expect("The `dlss` feature is enabled, but DlssProjectId was not added to the App before DlssInitPlugin.");
        // Placeholder: no vendor SDK is loaded here. The NGX DLSS SDK
        // initialization over the native ID3D12Device lands in task 16.2.
    }
}

/// Enables DLSS support. This requires [`DlssInitPlugin`] to function, which
/// must be manually registered in the correct order prior to registering this
/// plugin.
#[derive(Default)]
pub struct DlssPlugin;

impl Plugin for DlssPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Dlss<DlssSuperResolutionFeature>>()
            .register_type::<Dlss<DlssRayReconstructionFeature>>();
    }

    fn finish(&self, app: &mut App) {
        // Placeholder (task 16.3): without the NGX SDK, DLSS is never
        // "supported", so neither the supported-resources nor the render
        // systems are registered. The prepare/node entry points stay alive
        // (referenced below) so the API surface keeps compiling; task 16.2
        // registers them with the real capability probe over the native
        // device.
        let _ = (
            extract::extract_dlss::<DlssSuperResolutionFeature>,
            extract::extract_dlss::<DlssRayReconstructionFeature>,
            prepare::prepare_dlss::<DlssSuperResolutionFeature>,
            prepare::prepare_dlss::<DlssRayReconstructionFeature>,
            node::dlss_super_resolution,
            node::dlss_ray_reconstruction,
        );
        let _ = app;
    }
}

/// Camera component to enable DLSS.
#[derive(Component, Reflect, Clone)]
#[reflect(Component)]
#[require(TemporalJitter, MipBias, DepthPrepass, MotionVectorPrepass, Hdr)]
pub struct Dlss<F: DlssFeature = DlssSuperResolutionFeature> {
    /// How much upscaling should be applied.
    #[reflect(remote = DlssPerfQualityModeRemoteReflect)]
    pub perf_quality_mode: DlssPerfQualityMode,
    /// Set to true to delete the saved temporal history (past frames).
    ///
    /// Useful for preventing ghosting when the history is no longer
    /// representative of the current frame, such as in sudden camera cuts.
    ///
    /// After setting this to true, it will automatically be toggled
    /// back to false at the end of the frame.
    pub reset: bool,
    #[reflect(ignore)]
    pub _phantom_data: PhantomData<F>,
}

impl Default for Dlss<DlssSuperResolutionFeature> {
    fn default() -> Self {
        Self {
            perf_quality_mode: Default::default(),
            reset: Default::default(),
            _phantom_data: Default::default(),
        }
    }
}

/// The DLSS context handle (placeholder). Task 16.2 replaces the methods
/// with the real NGX SDK calls.
pub trait DlssFeature: Reflect + Clone + Default {
    type Context: Send;

    fn upscaled_resolution(context: &Self::Context) -> UVec2;

    fn render_resolution(context: &Self::Context) -> UVec2;

    fn suggested_jitter(
        context: &Self::Context,
        frame_number: u32,
        render_resolution: UVec2,
    ) -> Vec2;

    fn suggested_mip_bias(context: &Self::Context, render_resolution: UVec2) -> f32;

    fn new_context(
        upscaled_resolution: UVec2,
        perf_quality_mode: DlssPerfQualityMode,
        feature_flags: DlssFeatureFlags,
        device: &bevy_render::renderer::RenderDevice,
        queue: &bevy_render::renderer::RenderQueue,
    ) -> Result<Self::Context, DlssError>;
}

/// A placeholder DLSS context: holds the configuration that the NGX backend
/// (task 16.2) will turn into a real SDK context.
#[derive(Debug, Clone)]
pub struct DlssContextPlaceholder {
    /// Upscaled (output) resolution.
    pub upscaled_resolution: UVec2,
    /// Render (input) resolution.
    pub render_resolution: UVec2,
    /// Quality preset.
    pub perf_quality_mode: DlssPerfQualityMode,
}

/// DLSS error (placeholder). The NGX backend (task 16.2) maps real SDK
/// errors onto this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlssError {
    /// The vendor SDK is not available/initialized (placeholder state).
    Unavailable,
}

/// DLSS Super Resolution feature.
#[derive(Reflect, Clone, Default)]
pub struct DlssSuperResolutionFeature;

impl DlssFeature for DlssSuperResolutionFeature {
    type Context = DlssContextPlaceholder;

    fn upscaled_resolution(context: &Self::Context) -> UVec2 {
        context.upscaled_resolution
    }

    fn render_resolution(context: &Self::Context) -> UVec2 {
        context.render_resolution
    }

    fn suggested_jitter(
        _context: &Self::Context,
        _frame_number: u32,
        _render_resolution: UVec2,
    ) -> Vec2 {
        Vec2::ZERO
    }

    fn suggested_mip_bias(_context: &Self::Context, _render_resolution: UVec2) -> f32 {
        0.0
    }

    fn new_context(
        _upscaled_resolution: UVec2,
        _perf_quality_mode: DlssPerfQualityMode,
        _feature_flags: DlssFeatureFlags,
        _device: &bevy_render::renderer::RenderDevice,
        _queue: &bevy_render::renderer::RenderQueue,
    ) -> Result<Self::Context, DlssError> {
        // Placeholder (task 16.3): no SDK, so creation cannot succeed. The
        // NGX backend (task 16.2) constructs the real DLSS context here.
        Err(DlssError::Unavailable)
    }
}

/// DLSS Ray Reconstruction feature.
#[derive(Reflect, Clone, Default)]
pub struct DlssRayReconstructionFeature;

impl DlssFeature for DlssRayReconstructionFeature {
    type Context = DlssContextPlaceholder;

    fn upscaled_resolution(context: &Self::Context) -> UVec2 {
        context.upscaled_resolution
    }

    fn render_resolution(context: &Self::Context) -> UVec2 {
        context.render_resolution
    }

    fn suggested_jitter(
        _context: &Self::Context,
        _frame_number: u32,
        _render_resolution: UVec2,
    ) -> Vec2 {
        Vec2::ZERO
    }

    fn suggested_mip_bias(_context: &Self::Context, _render_resolution: UVec2) -> f32 {
        0.0
    }

    fn new_context(
        _upscaled_resolution: UVec2,
        _perf_quality_mode: DlssPerfQualityMode,
        _feature_flags: DlssFeatureFlags,
        _device: &bevy_render::renderer::RenderDevice,
        _queue: &bevy_render::renderer::RenderQueue,
    ) -> Result<Self::Context, DlssError> {
        Err(DlssError::Unavailable)
    }
}

/// Additional textures needed as inputs for [`DlssRayReconstructionFeature`].
#[derive(Component)]
pub struct ViewDlssRayReconstructionTextures {
    pub diffuse_albedo: CachedTexture,
    pub specular_albedo: CachedTexture,
    pub normal_roughness: CachedTexture,
    pub specular_motion_vectors: CachedTexture,
}

#[reflect_remote(DlssPerfQualityMode)]
#[derive(Default)]
enum DlssPerfQualityModeRemoteReflect {
    #[default]
    Auto,
    Dlaa,
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
}

/// Application-specific ID for DLSS.
///
/// See the DLSS programming guide for more info.
#[derive(Resource, Clone)]
pub struct DlssProjectId(pub uuid::Uuid);

/// When DLSS Super Resolution is supported by the current system, this
/// resource will exist in the main world. Otherwise this resource will be
/// absent. (Placeholder: never inserted until task 16.2 lands.)
#[derive(Resource, Clone, Copy)]
pub struct DlssSuperResolutionSupported;

/// When DLSS Ray Reconstruction is supported by the current system, this
/// resource will exist in the main world. Otherwise this resource will be
/// absent. (Placeholder: never inserted until task 16.2 lands.)
#[derive(Resource, Clone, Copy)]
pub struct DlssRayReconstructionSupported;

