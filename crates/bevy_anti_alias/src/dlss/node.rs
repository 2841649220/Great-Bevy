use super::prepare::DlssRenderContext;
use super::{
    Dlss, DlssRayReconstructionFeature, DlssSuperResolutionFeature,
    ViewDlssRayReconstructionTextures,
};
use bevy_camera::MainPassResolutionOverride;
use bevy_core_pipeline::prepass::ViewPrepassTextures;
use bevy_render::{
    camera::TemporalJitter,
    renderer::{RenderContext, ViewQuery},
    view::ViewTarget,
};

/// DLSS Super Resolution render node.
///
/// Placeholder (task 16.3): the upstream implementation issued a
/// `dlss_wgpu` command buffer; that backend is gone. The node keeps the
/// signature so the schedule wiring compiles and does no GPU work until the
/// NGX backend (task 16.2) supplies the real command.
pub fn dlss_super_resolution(
    _view: ViewQuery<(
        &Dlss<DlssSuperResolutionFeature>,
        &DlssRenderContext<DlssSuperResolutionFeature>,
        &MainPassResolutionOverride,
        &TemporalJitter,
        &ViewTarget,
        &ViewPrepassTextures,
    )>,
    _ctx: RenderContext,
) {
}

/// DLSS Ray Reconstruction render node.
///
/// Placeholder (task 16.3): see [`dlss_super_resolution`].
pub fn dlss_ray_reconstruction(
    _view: ViewQuery<(
        &Dlss<DlssRayReconstructionFeature>,
        &DlssRenderContext<DlssRayReconstructionFeature>,
        &MainPassResolutionOverride,
        &TemporalJitter,
        &ViewTarget,
        &ViewPrepassTextures,
        &ViewDlssRayReconstructionTextures,
    )>,
    _ctx: RenderContext,
) {
}
