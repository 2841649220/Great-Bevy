mod fallback_image;
mod gpu_image;
mod manual_texture_view;
mod texture_attachment;
mod texture_cache;

pub use crate::render_resource::DefaultImageSampler;
use bevy_image::{CompressedImageFormatSupport, CompressedImageFormats, ImageLoader, ImagePlugin};
pub use fallback_image::*;
pub use gpu_image::*;
pub use manual_texture_view::*;
pub use texture_attachment::*;
pub use texture_cache::*;

/// A pending texture readback recorded by
/// `CommandEncoder::copy_texture_to_buffer` (M1-4b-2: the diligent copy API
/// has no texture-to-buffer direction, so the readback stages through a
/// same-format staging texture when `BufferSlice::map_async` executes).
pub struct TextureReadbackPending {
    /// The device that created the destination buffer (its immediate
    /// context executes the readback).
    pub(crate) device: crate::renderer::RenderDevice,
    /// The source texture (owned clone).
    pub(crate) source: crate::render_resource::Texture,
    /// The source mip level.
    pub(crate) mip_level: u32,
    /// The source array slice.
    pub(crate) array_slice: u32,
}

/// Executes a pending texture readback into `buffer`'s mapped slot (blocking
/// map; the copy is recorded on the immediate context in command order).
pub(crate) fn execute_texture_readback(
    pending: &TextureReadbackPending,
    buffer: &crate::render_resource::Buffer,
) -> Result<(), crate::render_resource::BufferAsyncError> {
    let Some(context) = pending.device.diligent_context_handle() else {
        return Err(crate::render_resource::BufferAsyncError);
    };
    let Some(texture) = pending.source.diligent() else {
        return Err(crate::render_resource::BufferAsyncError);
    };
    let Some(device) = pending.device.diligent_device() else {
        return Err(crate::render_resource::BufferAsyncError);
    };
    let format = match diligent_rs::format::to_diligent(pending.source.format()) {
        Ok(format) => format,
        Err(_) => return Err(crate::render_resource::BufferAsyncError),
    };
    let size = pending.source.size();
    let staging = match device.create_staging_texture(
        "bevy_texture_readback_staging",
        size.width,
        size.height,
        format,
    ) {
        Ok(staging) => staging,
        Err(_) => return Err(crate::render_resource::BufferAsyncError),
    };
    let _guard = crate::renderer::diligent_registry::context_guard();
    if context
        .copy_texture(texture, pending.mip_level, pending.array_slice, &staging, 0, 0)
        .is_err()
    {
        return Err(crate::render_resource::BufferAsyncError);
    }
    let mapped = match context.map_texture_subresource(
        &staging,
        0,
        0,
        diligent_rs::diligent_sys::bindings::_MAP_TYPE::MAP_READ
            as diligent_rs::diligent_sys::bindings::MAP_TYPE,
        false, // blocking: the copy must complete first
    ) {
        Ok(Some(mapped)) => mapped,
        _ => return Err(crate::render_resource::BufferAsyncError),
    };
    let stride = mapped.stride();
    let mut data = Vec::with_capacity(stride * size.height as usize);
    for row in 0..size.height {
        data.extend_from_slice(unsafe {
            core::slice::from_raw_parts(mapped.row(row as usize), stride)
        });
    }
    drop(mapped);
    buffer.store_mapped(data);
    Ok(())
}

use crate::{
    extract_resource::ExtractResourcePlugin, init_gpu_resource, render_asset::RenderAssetPlugin,
    render_resource::DefaultImageSamplerDescriptor, GpuResourceAppExt, Render, RenderApp,
    RenderStartup, RenderSystems,
};
use bevy_app::{App, Plugin};
use bevy_asset::AssetApp;
use bevy_ecs::prelude::*;
use bevy_log::warn;

#[derive(Default)]
pub struct TexturePlugin;

impl Plugin for TexturePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            RenderAssetPlugin::<GpuImage>::default(),
            ExtractResourcePlugin::<ManualTextureViews>::default(),
        ))
        .init_resource::<ManualTextureViews>();
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<ManualTextureViews>()
                .init_gpu_resource::<TextureCache>()
                .allow_ambiguous_resource::<TextureCache>()
                .add_systems(
                    Render,
                    update_texture_cache_system.in_set(RenderSystems::Cleanup),
                );
        }
    }

    fn finish(&self, app: &mut App) {
        if !ImageLoader::SUPPORTED_FORMATS.is_empty() {
            let supported_compressed_formats = if let Some(resource) =
                app.world().get_resource::<CompressedImageFormatSupport>()
            {
                resource.0
            } else {
                warn!("CompressedImageFormatSupport resource not found. It should either be initialized in finish() of \
                       RenderPlugin, or manually if not using the RenderPlugin or the WGPU backend.");
                CompressedImageFormats::NONE
            };

            app.register_asset_loader(ImageLoader::new(supported_compressed_formats));
        }
        let default_sampler = app.get_added_plugins::<ImagePlugin>()[0]
            .default_sampler
            .clone();

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(DefaultImageSamplerDescriptor(default_sampler.clone()));
            render_app.add_systems(
                RenderStartup,
                (
                    init_gpu_resource::<DefaultImageSampler>,
                    init_gpu_resource::<FallbackImage>,
                    init_gpu_resource::<FallbackImageZero>,
                    init_gpu_resource::<FallbackImageCubemap>,
                    init_gpu_resource::<FallbackImageFormatMsaaCache>,
                )
                    .chain()
                    .ambiguous_with_all(),
            );
        }
    }
}
