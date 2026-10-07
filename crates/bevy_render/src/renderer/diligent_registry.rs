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
//! through the `Arc` in the bevy_render wrapper that registered them.
//! Clone-shared registration tokens remove each pointer before its final
//! native handle is released, and `BindGroupEntry` borrows its wrapper while
//! `create_bind_group` runs, so every lookup sees a live object.

use crate::render_resource::{BufferId, SamplerId, TextureId, TextureViewId};
use alloc::sync::Arc;
use core::{cell::RefCell, marker::PhantomData};
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
// SAFETY: the same deliberate opt-in as the `Send` impl above: the wrapped
// engine objects are ref-counted and thread-safe, and `&DiligentHandle` only
// hands out `&T` / `Arc` clones - the one non-thread-safe object (the
// immediate `DeviceContext`) stays confined to the render thread, as the
// struct docs describe.
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
    // `RwLock`, not `Mutex`: the resolve side is on the per-draw hot path
    // (`set_vertex_buffer` / `set_index_buffer` / indirect-draw lookups) and
    // strictly dominates the register side, which only runs once per resource
    // creation. A `Mutex` would serialize every one of those reads against all
    // other render-world systems.
    buffers: std::sync::RwLock<std::collections::HashMap<u32, *mut sys::IBuffer>>,
    textures: std::sync::RwLock<std::collections::HashMap<u32, *mut sys::ITexture>>,
    texture_views: std::sync::RwLock<std::collections::HashMap<u32, *mut sys::ITextureView>>,
    samplers: std::sync::RwLock<std::collections::HashMap<u32, *mut sys::ISampler>>,
}

// SAFETY: all access happens under the internal `RwLock`s; the stored raw
// pointers are non-owning and the pointed-to objects are kept alive by the
// wrapper `Arc`s that registered them (see the module docs).
unsafe impl Send for ResourceRegistry {}
// SAFETY: same invariant as the `Send` impl above: every map is behind its
// own `RwLock`, so shared access only ever touches guarded state, and the
// stored raw pointers are non-owning - the pointed-to engine objects are kept
// alive by the wrapper `Arc`s that registered them (see the module docs).
unsafe impl Sync for ResourceRegistry {}

/// Poison-tolerant registry locking.
///
/// The guarded maps only hold `u32` keys and non-owning raw pointers, so a
/// panic cannot leave partially-updated state behind. The diligent-rs wrapper
/// panics on a missing vtable slot, though, and letting that poison these locks
/// would turn one bad frame into a renderer that can never be used again.
fn lock_read<T>(lock: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_write<T>(lock: &std::sync::RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl ResourceRegistry {
    pub(crate) fn register_buffer(&self, id: BufferId, buffer: *mut sys::IBuffer) {
        if !buffer.is_null() {
            lock_write(&self.buffers)
                .insert(u32::from(core::num::NonZero::<u32>::from(id)), buffer);
        }
    }

    pub(crate) fn register_texture(&self, id: TextureId, texture: *mut sys::ITexture) {
        if !texture.is_null() {
            lock_write(&self.textures)
                .insert(u32::from(core::num::NonZero::<u32>::from(id)), texture);
        }
    }

    pub(crate) fn register_texture_view(&self, id: TextureViewId, view: *mut sys::ITextureView) {
        if !view.is_null() {
            lock_write(&self.texture_views)
                .insert(u32::from(core::num::NonZero::<u32>::from(id)), view);
        }
    }

    pub(crate) fn register_sampler(&self, id: SamplerId, sampler: *mut sys::ISampler) {
        if !sampler.is_null() {
            lock_write(&self.samplers)
                .insert(u32::from(core::num::NonZero::<u32>::from(id)), sampler);
        }
    }

    pub(crate) fn track_buffer(
        &self,
        id: BufferId,
        buffer: *mut sys::IBuffer,
    ) -> Option<Arc<RegistryRegistration>> {
        if buffer.is_null() {
            return None;
        }
        self.register_buffer(id, buffer);
        Some(Arc::new(RegistryRegistration {
            id: u32::from(core::num::NonZero::<u32>::from(id)),
            kind: RegistryEntryKind::Buffer,
        }))
    }

    pub(crate) fn track_texture(
        &self,
        id: TextureId,
        texture: *mut sys::ITexture,
    ) -> Option<Arc<RegistryRegistration>> {
        if texture.is_null() {
            return None;
        }
        self.register_texture(id, texture);
        Some(Arc::new(RegistryRegistration {
            id: u32::from(core::num::NonZero::<u32>::from(id)),
            kind: RegistryEntryKind::Texture,
        }))
    }

    pub(crate) fn track_texture_view(
        &self,
        id: TextureViewId,
        view: *mut sys::ITextureView,
    ) -> Option<Arc<RegistryRegistration>> {
        if view.is_null() {
            return None;
        }
        self.register_texture_view(id, view);
        Some(Arc::new(RegistryRegistration {
            id: u32::from(core::num::NonZero::<u32>::from(id)),
            kind: RegistryEntryKind::TextureView,
        }))
    }

    /// Creates a lifetime token for a registry id whose native pointer is
    /// supplied later (the swap-chain back buffer changes each frame).
    pub(crate) fn reserve_texture_view(&self, id: TextureViewId) -> Arc<RegistryRegistration> {
        Arc::new(RegistryRegistration {
            id: u32::from(core::num::NonZero::<u32>::from(id)),
            kind: RegistryEntryKind::TextureView,
        })
    }

    pub(crate) fn track_sampler(
        &self,
        id: SamplerId,
        sampler: *mut sys::ISampler,
    ) -> Option<Arc<RegistryRegistration>> {
        if sampler.is_null() {
            return None;
        }
        self.register_sampler(id, sampler);
        Some(Arc::new(RegistryRegistration {
            id: u32::from(core::num::NonZero::<u32>::from(id)),
            kind: RegistryEntryKind::Sampler,
        }))
    }

    pub(crate) fn resolve_buffer(&self, id: BufferId) -> Option<*mut sys::IBuffer> {
        lock_read(&self.buffers)
            .get(&u32::from(core::num::NonZero::<u32>::from(id)))
            .copied()
    }

    pub(crate) fn resolve_texture(&self, id: TextureId) -> Option<*mut sys::ITexture> {
        lock_read(&self.textures)
            .get(&u32::from(core::num::NonZero::<u32>::from(id)))
            .copied()
    }

    pub(crate) fn resolve_texture_view(&self, id: TextureViewId) -> Option<*mut sys::ITextureView> {
        lock_read(&self.texture_views)
            .get(&u32::from(core::num::NonZero::<u32>::from(id)))
            .copied()
    }

    pub(crate) fn resolve_sampler(&self, id: SamplerId) -> Option<*mut sys::ISampler> {
        lock_read(&self.samplers)
            .get(&u32::from(core::num::NonZero::<u32>::from(id)))
            .copied()
    }

    fn unregister(&self, id: u32, kind: RegistryEntryKind) {
        match kind {
            RegistryEntryKind::Buffer => {
                lock_write(&self.buffers).remove(&id);
            }
            RegistryEntryKind::Texture => {
                lock_write(&self.textures).remove(&id);
            }
            RegistryEntryKind::TextureView => {
                lock_write(&self.texture_views).remove(&id);
            }
            RegistryEntryKind::Sampler => {
                lock_write(&self.samplers).remove(&id);
            }
        }
    }

    pub(crate) fn clear_texture_view(&self, id: TextureViewId) {
        lock_write(&self.texture_views).remove(&u32::from(core::num::NonZero::<u32>::from(id)));
    }
}

#[derive(Clone, Copy)]
enum RegistryEntryKind {
    Buffer,
    Texture,
    TextureView,
    Sampler,
}

/// Shared lifetime token for a non-owning registry entry. Resource clones
/// share this token; the last clone removes the raw pointer before its native
/// handle is released.
pub(crate) struct RegistryRegistration {
    id: u32,
    kind: RegistryEntryKind,
}

impl Drop for RegistryRegistration {
    fn drop(&mut self) {
        registry().unregister(self.id, self.kind);
    }
}

/// The process-wide registry (multiple devices share it; entries are keyed by
/// globally unique resource ids and removed with their wrapper's last clone).
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

struct ContextLockState {
    depth: usize,
    lock: Option<std::sync::MutexGuard<'static, ()>>,
}

thread_local! {
    static CONTEXT_LOCK_STATE: RefCell<ContextLockState> = RefCell::new(ContextLockState {
        depth: 0,
        lock: None,
    });
}

/// An immediate-context lock that can be nested on its owning thread.
///
/// Render passes hold this guard for their full begin/record/end sequence;
/// per-method guards inside that sequence are reentrant, while other render
/// systems remain serialized behind the process-wide context lock.
pub(crate) struct ContextGuard {
    _not_send: PhantomData<alloc::rc::Rc<()>>,
}

impl Drop for ContextGuard {
    fn drop(&mut self) {
        CONTEXT_LOCK_STATE.with(|state| {
            let mut state = state.borrow_mut();
            debug_assert!(state.depth > 0, "unbalanced Diligent context guard");
            state.depth = state.depth.saturating_sub(1);
            if state.depth == 0 {
                drop(state.lock.take());
            }
        });
    }
}

/// Acquires the immediate-context lock. Calls made while a guard is already
/// held on the same thread reuse that lock rather than deadlocking.
pub(crate) fn context_guard() -> ContextGuard {
    CONTEXT_LOCK_STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.depth == 0 {
            state.lock = Some(
                CONTEXT_LOCK
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }
        state.depth += 1;
    });
    ContextGuard {
        _not_send: PhantomData,
    }
}
