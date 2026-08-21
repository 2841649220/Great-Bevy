use crate::{
    render_resource::{WgpuSampler, WgpuTextureView},
    renderer::{diligent_registry::DiligentHandle, RenderDevice},
};
use alloc::sync::Arc;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    resource::Resource,
    world::{FromWorld, World},
};
use bevy_image::ImageSamplerDescriptor;
use bevy_utils::define_atomic_id;
use core::ops::Deref;
use diligent_rs::diligent_sys::bindings as sys;

define_atomic_id!(TextureId);

/// A GPU-accessible texture.
///
/// The primary handle is a Diligent [`Texture`](diligent_rs::Texture)
/// (M1-4b-2: the transition wgpu handle is gone; `format`/`dimension`/
/// `size`/`usage` are inherent methods).
/// Can be created via [`RenderDevice::create_texture`](crate::renderer::RenderDevice::create_texture).
///
/// Other options for storing GPU-accessible data are:
/// * [`BufferVec`](crate::render_resource::BufferVec)
/// * [`DynamicStorageBuffer`](crate::render_resource::DynamicStorageBuffer)
/// * [`DynamicUniformBuffer`](crate::render_resource::DynamicUniformBuffer)
/// * [`GpuArrayBuffer`](crate::render_resource::GpuArrayBuffer)
/// * [`RawBufferVec`](crate::render_resource::RawBufferVec)
/// * [`StorageBuffer`](crate::render_resource::StorageBuffer)
/// * [`UniformBuffer`](crate::render_resource::UniformBuffer)
#[derive(Clone)]
pub struct Texture {
    pub(crate) id: TextureId,
    /// The Diligent texture (`None` when the Diligent creation failed).
    pub(crate) value: Option<DiligentHandle<diligent_rs::Texture>>,
    /// The format of the texture.
    pub(crate) format: wgpu_types::TextureFormat,
    /// The size of the texture.
    pub(crate) size: wgpu_types::Extent3d,
    /// The dimension of the texture.
    pub(crate) dimension: wgpu_types::TextureDimension,
    /// The number of mip levels of the texture.
    pub(crate) mip_level_count: u32,
    /// The sample count of the texture.
    pub(crate) sample_count: u32,
    /// The usages the texture was created with.
    pub(crate) usage: wgpu_types::TextureUsages,
    /// The Diligent bind flags this texture was created with (drives the
    /// view-type heuristic).
    pub(crate) bind_flags: sys::BIND_FLAGS,
}

impl core::fmt::Debug for Texture {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Texture")
            .field("id", &self.id)
            .field("diligent", &self.value.is_some())
            .finish()
    }
}

impl Texture {
    /// Returns the [`TextureId`].
    #[inline]
    pub fn id(&self) -> TextureId {
        self.id
    }

    /// Creates a view of this texture.
    pub fn create_view(&self, desc: &crate::render_resource::TextureViewDescriptor) -> TextureView {
        let value = self.diligent_view(desc);
        let id = TextureViewId::new();
        if let Some(view) = &value {
            crate::renderer::diligent_registry::registry()
                .register_texture_view(id, view.as_raw());
        }
        TextureView {
            id,
            inner: Arc::new(WgpuTextureView {
                id,
                value,
                format: self.format,
                size: self.size,
                dimension: desc
                    .dimension
                    .unwrap_or(wgpu_types::TextureViewDimension::D2),
            }),
        }
    }

    /// Creates the Diligent view for a wgpu view descriptor.
    ///
    /// The view type is derived from the texture's bind flags (wgpu views
    /// are usage-agnostic; Diligent needs a concrete type at creation).
    /// Depth formats never pick `TEXTURE_VIEW_RENDER_TARGET` (the engine
    /// cannot create an RTV on a depth format); the render-pass path derives
    /// its own attachment views of the required type on demand
    /// (`diligent_draw::resolve_attachment_view`).
    fn diligent_view(
        &self,
        desc: &crate::render_resource::TextureViewDescriptor,
    ) -> Option<DiligentHandle<diligent_rs::TextureView>> {
        let texture = self.value.as_ref()?;
        let is_depth = self.format.is_depth_stencil_format();
        let view_type = if self.bind_flags & (sys::_BIND_FLAGS::BIND_SHADER_RESOURCE as u32) != 0 {
            sys::_TEXTURE_VIEW_TYPE::TEXTURE_VIEW_SHADER_RESOURCE
        } else if self.bind_flags & (sys::_BIND_FLAGS::BIND_UNORDERED_ACCESS as u32) != 0 {
            sys::_TEXTURE_VIEW_TYPE::TEXTURE_VIEW_UNORDERED_ACCESS
        } else if !is_depth && self.bind_flags & (sys::_BIND_FLAGS::BIND_RENDER_TARGET as u32) != 0 {
            sys::_TEXTURE_VIEW_TYPE::TEXTURE_VIEW_RENDER_TARGET
        } else if self.bind_flags & (sys::_BIND_FLAGS::BIND_DEPTH_STENCIL as u32) != 0 {
            sys::_TEXTURE_VIEW_TYPE::TEXTURE_VIEW_DEPTH_STENCIL
        } else {
            sys::_TEXTURE_VIEW_TYPE::TEXTURE_VIEW_SHADER_RESOURCE
        } as sys::TEXTURE_VIEW_TYPE;

        let format_override = match desc.format {
            Some(format) => match diligent_rs::format::to_diligent(format) {
                Ok(format) => Some(format),
                Err(err) => {
                    bevy_log::warn!("diligent: texture view format: {err}");
                    return None;
                }
            },
            None => None,
        };

        let dimension = match desc
            .dimension
            .unwrap_or(wgpu_types::TextureViewDimension::D2)
        {
            wgpu_types::TextureViewDimension::D1 => sys::_RESOURCE_DIMENSION::RESOURCE_DIM_TEX_1D,
            wgpu_types::TextureViewDimension::D2 => sys::_RESOURCE_DIMENSION::RESOURCE_DIM_TEX_2D,
            wgpu_types::TextureViewDimension::D2Array => {
                sys::_RESOURCE_DIMENSION::RESOURCE_DIM_TEX_2D_ARRAY
            }
            wgpu_types::TextureViewDimension::Cube => sys::_RESOURCE_DIMENSION::RESOURCE_DIM_TEX_CUBE,
            wgpu_types::TextureViewDimension::CubeArray => {
                sys::_RESOURCE_DIMENSION::RESOURCE_DIM_TEX_CUBE_ARRAY
            }
            wgpu_types::TextureViewDimension::D3 => sys::_RESOURCE_DIMENSION::RESOURCE_DIM_TEX_3D,
        } as sys::RESOURCE_DIMENSION;

        let mut view_desc = diligent_rs::desc::texture_view(
            view_type,
            format_override,
            desc.base_mip_level,
            desc.mip_level_count.unwrap_or(0),
            desc.base_array_layer,
            desc.array_layer_count.unwrap_or(0),
        );
        view_desc.TextureDim = dimension;
        if desc.aspect != wgpu_types::TextureAspect::All {
            // Diligent has no aspect planes; depth-only views are created as
            // plain SRVs, stencil-only views are not supported
            // (TODO-REMOVE-M1-4: re-evaluate in the M2 binding model).
            bevy_log::warn!(
                "diligent: texture view aspect {:?} is not representable; using a plain view",
                desc.aspect
            );
        }
        match texture.create_view(&view_desc) {
            Ok(view) => Some(DiligentHandle::new(Arc::new(view))),
            Err(err) => {
                bevy_log::warn!("diligent: texture view creation failed: {err}");
                None
            }
        }
    }

    /// The Diligent texture, when this instance has one.
    pub(crate) fn diligent(&self) -> Option<&diligent_rs::Texture> {
        self.value.as_deref()
    }

    /// The format of the texture.
    pub fn format(&self) -> wgpu_types::TextureFormat {
        self.format
    }

    /// The dimension of the texture.
    pub fn dimension(&self) -> wgpu_types::TextureDimension {
        self.dimension
    }

    /// The size of the texture.
    pub fn size(&self) -> wgpu_types::Extent3d {
        self.size
    }

    /// The number of mip levels of the texture.
    pub fn mip_level_count(&self) -> u32 {
        self.mip_level_count
    }

    /// The sample count of the texture.
    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    /// The usages the texture was created with.
    pub fn usage(&self) -> wgpu_types::TextureUsages {
        self.usage
    }

    /// The width of the texture.
    pub fn width(&self) -> u32 {
        self.size.width
    }

    /// The height of the texture.
    pub fn height(&self) -> u32 {
        self.size.height
    }

    /// The number of layers or depth of the texture.
    pub fn depth_or_array_layers(&self) -> u32 {
        self.size.depth_or_array_layers
    }

    /// The number of layers of the texture.
    pub fn array_layer_count(&self) -> u32 {
        self.size.depth_or_array_layers
    }

    /// Returns an info describing the entire texture, for use in copy
    /// operations (the wgpu `Texture::as_image_copy` shape).
    pub fn as_image_copy(&self) -> crate::render_resource::TexelCopyTextureInfo<'_> {
        crate::render_resource::TexelCopyTextureInfo {
            texture: self,
            mip_level: 0,
            origin: wgpu_types::Origin3d::ZERO,
            aspect: wgpu_types::TextureAspect::All,
        }
    }
}

define_atomic_id!(TextureViewId);

/// Describes a [`Texture`] with its associated metadata required by a pipeline or [`BindGroup`](super::BindGroup).
#[derive(Clone)]
pub struct TextureView {
    pub(crate) id: TextureViewId,
    /// The handle the wrapper dereferences to (the diligent view carrier;
    /// the address is stable across clones, so the SRB binding path can
    /// resolve the diligent object by id).
    pub(crate) inner: Arc<WgpuTextureView>,
}

impl core::fmt::Debug for TextureView {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TextureView")
            .field("id", &self.id)
            .field("diligent", &self.inner.value.is_some())
            .finish()
    }
}

impl TextureView {
    /// Returns the [`TextureViewId`].
    #[inline]
    pub fn id(&self) -> TextureViewId {
        self.id
    }

    /// The format of the underlying texture.
    pub fn format(&self) -> wgpu_types::TextureFormat {
        self.inner.format
    }

    /// The size of the underlying texture.
    pub fn size(&self) -> wgpu_types::Extent3d {
        self.inner.size
    }

    /// The dimension of the view.
    pub fn dimension(&self) -> wgpu_types::TextureViewDimension {
        self.inner.dimension
    }

    /// The Diligent texture view, when this instance has one.
    #[expect(dead_code, reason = "consumed by the diligent readback paths (screenshots, gpu_readback)")]
    pub(crate) fn diligent(&self) -> Option<&diligent_rs::TextureView> {
        self.inner.value.as_deref()
    }
}

impl Deref for TextureView {
    type Target = WgpuTextureView;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

define_atomic_id!(SamplerId);

/// A Sampler defines how a pipeline will sample from a [`TextureView`].
/// They define image filters (including anisotropy) and address (wrapping) modes, among other things.
///
/// The primary handle is a Diligent [`Sampler`](diligent_rs::Sampler)
/// (M1-4b-2: the transition wgpu handle is gone).
/// Can be created via [`RenderDevice::create_sampler`](crate::renderer::RenderDevice::create_sampler).
#[derive(Clone)]
pub struct Sampler {
    pub(crate) id: SamplerId,
    /// The handle the wrapper dereferences to (the diligent sampler carrier).
    pub(crate) inner: Arc<WgpuSampler>,
}

impl core::fmt::Debug for Sampler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Sampler")
            .field("id", &self.id)
            .field("diligent", &self.inner.value.is_some())
            .finish()
    }
}

impl Sampler {
    /// Returns the [`SamplerId`].
    #[inline]
    pub fn id(&self) -> SamplerId {
        self.id
    }

    /// The Diligent sampler, when this instance has one.
    #[expect(dead_code, reason = "consumed by the SRB binding path through the registry")]
    pub(crate) fn diligent(&self) -> Option<&diligent_rs::Sampler> {
        self.inner.value.as_deref()
    }
}

impl Deref for Sampler {
    type Target = WgpuSampler;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Stores the [`ImageSamplerDescriptor`] used to create the [`DefaultImageSampler`].
///
/// This is kept as a resource so that [`DefaultImageSampler`] can be recreated on GPU device recovery.
#[derive(Resource, Debug, Clone, Deref)]
pub struct DefaultImageSamplerDescriptor(pub ImageSamplerDescriptor);

/// A rendering resource for the default image sampler which is set during renderer
/// initialization.
///
/// The [`ImagePlugin`](bevy_image::ImagePlugin) can be set during app initialization to change the default
/// image sampler.
#[derive(Resource, Debug, Clone, Deref, DerefMut)]
pub struct DefaultImageSampler(pub(crate) Sampler);

impl FromWorld for DefaultImageSampler {
    fn from_world(world: &mut World) -> Self {
        let descriptor = world.resource::<DefaultImageSamplerDescriptor>();
        let wgpu_descriptor = descriptor.as_wgpu();
        let device = world.resource::<RenderDevice>();
        let sampler = device.create_sampler(&wgpu_descriptor);
        Self(sampler)
    }
}
