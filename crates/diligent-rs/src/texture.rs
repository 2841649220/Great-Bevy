//! Texture and texture-view wrappers (`ITexture`, `ITextureView`).
//!
//! Views come in two ownership flavors, mirroring the engine:
//!
//! * [`Texture::get_default_view`] returns a **borrowed** view (the engine
//!   does not `AddRef`, so the wrapper must not `Release`) - wrapped as
//!   [`crate::handle::NonOwning`], tied to the texture's lifetime;
//! * [`Texture::create_view`] (and
//!   [`crate::device::RenderDevice::create_texture_view`]) returns an
//!   **owned** view (the engine `AddRef`s, the wrapper `Release`s on drop).

use diligent_sys::bindings as sys;

use crate::error::{Error, Result};
use crate::handle::{impl_shared_ownership, Handle, NonOwning};

/// Owning handle to a texture (`ITexture`).
pub struct Texture {
    handle: Handle<sys::ITexture>,
}

impl_shared_ownership!(Texture, sys::ITexture);

impl Texture {
    /// Wraps a texture pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `ITexture` instance returned by the engine (which
    /// AddRefs it); ownership is transferred to the wrapper, which releases
    /// it on drop. Only engine-returned pointers may be passed here.
    pub unsafe fn from_raw(ptr: *mut sys::ITexture) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw texture pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::ITexture {
        self.handle.as_ptr()
    }

    /// The texture description (`ITexture::GetDesc` via the universal
    /// `IDeviceObject` slot; the `DeviceObjectAttribs` member is the first
    /// field of `TextureDesc`). Copied out.
    pub fn desc(&self) -> Result<sys::TextureDesc> {
        let get = unsafe {
            (*(*self.as_raw()).pVtbl)
                .DeviceObject
                .GetDesc
                .as_ref()
                .ok_or(Error::MissingMethod("ITexture::GetDesc"))?
        };
        // Safety: the engine returns a pointer to internal storage that is
        // valid while the texture is alive; the desc's first member is the
        // `DeviceObjectAttribs` the call returns.
        let ptr = unsafe { get(self.as_raw().cast::<sys::IDeviceObject>()) };
        if ptr.is_null() {
            return Err(Error::MissingMethod("ITexture::GetDesc returned null"));
        }
        Ok(unsafe { *ptr.cast::<sys::TextureDesc>() })
    }

    /// The texture's MSAA sample count (`TextureDesc::SampleCount`; 1 for
    /// single-sample textures).
    pub fn sample_count(&self) -> Result<u32> {
        Ok(self.desc()?.SampleCount)
    }

    /// The default view of the given type (borrowed - do not release).
    ///
    /// `ITexture::GetDefaultView` does **not** increment the reference
    /// count; the returned view lives as long as this texture and is wrapped
    /// as [`NonOwning`]. Returns `Err(NullPointer)` when the engine has no
    /// default view of that type (a view type can be absent when the
    /// texture's bind flags do not enable it).
    pub fn get_default_view(&self, view_type: sys::TEXTURE_VIEW_TYPE) -> Result<NonOwning<sys::ITextureView>> {
        let get = unsafe {
            (*(*self.as_raw()).pVtbl)
                .Texture
                .GetDefaultView
                .as_ref()
                .ok_or(Error::MissingMethod("ITexture::GetDefaultView"))?
        };
        // Safety: the returned pointer is owned by the texture (no refcount
        // increment) and is valid while the texture is alive.
        let view = unsafe { get(self.as_raw(), view_type) };
        NonOwning::from_raw_opt(view).ok_or(Error::NullPointer("texture default view"))
    }

    /// Creates an owned view of this texture (`ITexture::CreateView`,
    /// engine `AddRef`s the result).
    ///
    /// `view_desc.Format = TEX_FORMAT_UNKNOWN` matches the texture format;
    /// any other value overrides the view format - this is the sRGB
    /// dual-view entry point (see [`crate::format::srgb_view_format`]).
    pub fn create_view(&self, view_desc: &sys::TextureViewDesc) -> Result<TextureView> {
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .Texture
                .CreateView
                .as_ref()
                .ok_or(Error::MissingMethod("ITexture::CreateView"))?
        };
        let mut view: *mut sys::ITextureView = std::ptr::null_mut();
        // Safety: `view_desc` is a valid FFI struct and `view` is an out
        // param; the engine AddRefs the returned view.
        unsafe { create(self.as_raw(), view_desc, &mut view) };
        if view.is_null() {
            return Err(Error::CreateFailed("texture view"));
        }
        // Safety: the engine AddRefs the view before returning it, so
        // ownership transfer into the wrapper is sound.
        Ok(unsafe { TextureView::from_raw(view) })
    }
}

/// Owning handle to a texture view (`ITextureView`), created via
/// [`Texture::create_view`].
///
/// The view holds a strong reference to its texture, so dropping the view
/// keeps the texture alive until the view itself is released.
pub struct TextureView {
    handle: Handle<sys::ITextureView>,
}

impl_shared_ownership!(TextureView, sys::ITextureView);

impl TextureView {
    /// Wraps a texture-view pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `ITextureView` instance returned by the engine
    /// (which AddRefs it); ownership is transferred to the wrapper, which
    /// releases it on drop. Only engine-returned pointers may be passed
    /// here.
    pub unsafe fn from_raw(ptr: *mut sys::ITextureView) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw view pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::ITextureView {
        self.handle.as_ptr()
    }
}
