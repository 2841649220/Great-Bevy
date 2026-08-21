//! Resource-id -> Diligent object registry for the SRB binding path (M1b;
//! M1-4b-2: keyed by the bevy resource ids - clone-stable and collision-free
//! now that the transition wgpu objects are gone).
//!
//! `create_bind_group` receives `BindGroupEntry`s whose resources are
//! `&Buffer` / `&TextureView` / `&Sampler` references. The corresponding
//! Diligent objects are recovered by the atomic resource id every bevy
//! resource wrapper carries (`BufferId`/`TextureId`/`TextureViewId`/
//! `SamplerId` - identical across clones of the wrapper). Every resource
//! created through `RenderDevice` registers its Diligent handle under that
//! id; the register and lookup sides both derive the id through the same
//! `id()` accessors.
//!
//! # Safety
//!
//! The stored pointers are **non-owning**: the Diligent objects stay alive
//! through the `Arc` in the bevy_render wrapper that registered them, and
//! the `BindGroupEntry` borrows one of those wrappers while
//! `create_bind_group` runs, so the pointer is valid for every lookup that
//! can actually happen. A stale entry (wrapper dropped) can only be hit by
//! an id reuse of a *newly created* wrapper, which re-registers the same key
//! first.

use crate::render_resource::{BufferId, SamplerId, TextureId, TextureViewId};
use alloc::sync::Arc;
use std::sync::OnceLock;

use diligent_rs::diligent_sys::bindings as sys;

/// A Send/Sync carrier for Diligent resource/device handles.
///
/// The diligent-rs wrapper deliberately does not implement `Send`/`Sync`
/// ("the device and resource objects are thread safe in Diligent, but this
/// crate keeps them pinned to their creating thread **until a deliberate
/// opt-in**"). Bevy's render resources must be `Send + Sync` (world
/// resources, cross-thread storage in gpu_preprocessing & co.), so the M1b
/// integration takes that documented opt-in: Diligent resource objects are
/// ref-counted and thread-safe at the engine level (creation stays on the
/// render thread; cross-thread use is limited to storage, cloning and
/// `Release`).
///
/// The one exception is the immediate [`DeviceContext`](diligent_rs::DeviceContext),
/// which is **not** thread-safe: it is only ever used from the render thread
/// (see `RenderDevice::poll`), and the carrier documents that discipline.
pub(crate) struct DiligentHandle<T>(pub(crate) Arc<T>);

// SAFETY: see the struct docs - deliberate opt-in for engine thread-safe
// objects (resources, device, PSO/SRB/PRS) and for the single-context
// discipline of the immediate device context.
unsafe impl<T> Send for DiligentHandle<T> {}
unsafe impl<T> Sync for DiligentHandle<T> {}

impl<T> Clone for DiligentHandle<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> DiligentHandle<T> {
    pub(crate) fn new(value: Arc<T>) -> Self {
        Self(value)
    }
}

impl<T> core::ops::Deref for DiligentHandle<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Default)]
pub(crate) struct ResourceRegistry {
    buffers: std::sync::Mutex<std::collections::HashMap<u32, *mut sys::IBuffer>>,
    textures: std::sync::Mutex<std::collections::HashMap<u32, *mut sys::ITexture>>,
    texture_views: std::sync::Mutex<std::collections::HashMap<u32, *mut sys::ITextureView>>,
    samplers: std::sync::Mutex<std::collections::HashMap<u32, *mut sys::ISampler>>,
}

// SAFETY: all access happens under the internal mutexes; the stored raw
// pointers are non-owning and the pointed-to objects are kept alive by the
// wrapper `Arc`s that registered them (see the module docs).
unsafe impl Send for ResourceRegistry {}
unsafe impl Sync for ResourceRegistry {}

impl ResourceRegistry {
    pub(crate) fn register_buffer(&self, id: BufferId, buffer: *mut sys::IBuffer) {
        if !buffer.is_null() {
            self.buffers.lock().unwrap().insert(u32::from(core::num::NonZero::<u32>::from(id)), buffer);
        }
    }

    pub(crate) fn register_texture(&self, id: TextureId, texture: *mut sys::ITexture) {
        if !texture.is_null() {
            self.textures.lock().unwrap().insert(u32::from(core::num::NonZero::<u32>::from(id)), texture);
        }
    }

    pub(crate) fn register_texture_view(&self, id: TextureViewId, view: *mut sys::ITextureView) {
        if !view.is_null() {
            self.texture_views.lock().unwrap().insert(u32::from(core::num::NonZero::<u32>::from(id)), view);
        }
    }

    pub(crate) fn register_sampler(&self, id: SamplerId, sampler: *mut sys::ISampler) {
        if !sampler.is_null() {
            self.samplers.lock().unwrap().insert(u32::from(core::num::NonZero::<u32>::from(id)), sampler);
        }
    }

    pub(crate) fn resolve_buffer(&self, id: BufferId) -> Option<*mut sys::IBuffer> {
        self.buffers.lock().unwrap().get(&u32::from(core::num::NonZero::<u32>::from(id))).copied()
    }

    pub(crate) fn resolve_texture(&self, id: TextureId) -> Option<*mut sys::ITexture> {
        self.textures.lock().unwrap().get(&u32::from(core::num::NonZero::<u32>::from(id))).copied()
    }

    pub(crate) fn resolve_texture_view(&self, id: TextureViewId) -> Option<*mut sys::ITextureView> {
        self.texture_views.lock().unwrap().get(&u32::from(core::num::NonZero::<u32>::from(id))).copied()
    }

    pub(crate) fn resolve_sampler(&self, id: SamplerId) -> Option<*mut sys::ISampler> {
        self.samplers.lock().unwrap().get(&u32::from(core::num::NonZero::<u32>::from(id))).copied()
    }
}

/// The process-wide registry (multiple devices share it; entries are keyed
/// by resource id, and re-registration overwrites stale entries).
pub(crate) fn registry() -> &'static ResourceRegistry {
    static REGISTRY: OnceLock<ResourceRegistry> = OnceLock::new();
    REGISTRY.get_or_init(ResourceRegistry::default)
}

/// Serializes every access to the Diligent immediate device context.
///
/// M1-4b-2: the render-world schedules are multithreaded (bevy_ecs runs the
/// schedule's systems concurrently on the task pool), so multiple systems can
/// touch the immediate context in the same frame - while the diligent-rs
/// wrapper documents the immediate context as **not** thread-safe (the engine
/// records into a single D3D12 command list; concurrent calls corrupt it - a
/// D3D12 debug-layer `CORRUPTED_MULTITHREADING` break). Every context method
/// call takes this lock for the duration of the call.
pub(crate) static CONTEXT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquires the immediate-context lock for one context method call.
pub(crate) fn context_guard() -> std::sync::MutexGuard<'static, ()> {
    CONTEXT_LOCK.lock().unwrap()
}
