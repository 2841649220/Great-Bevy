//! Owning and non-owning interface handles.
//!
//! Diligent interfaces are COM-style reference counted objects. Every
//! generated interface struct is `#[repr(C)] { pVtbl: *mut Vtbl }` where the
//! first member of every Vtbl is `Object: IObjectMethods`, so the universal
//! `Release` slot is at a fixed offset for all interfaces. [`Handle`]
//! exploits that layout to release any interface on drop.

use std::ops::Deref;

use diligent_sys::bindings::IObject;

use crate::error::Result;

/// Owning handle to a Diligent interface instance.
///
/// Calls `IObject::Release` on drop. Never wraps a null pointer.
///
/// Deliberately does **not** implement `Send`/`Sync` - see the crate-level
/// threading notes. Device and resource objects are thread safe in Diligent,
/// but opt-in `unsafe` transmutation through [`Handle::as_ptr`] is left to
/// the caller.
pub struct Handle<T> {
    ptr: *mut T,
}

impl<T> Handle<T> {
    /// Wraps a raw pointer returned by the engine. Panics on null.
    ///
    /// Only engine-returned (AddRef'ed) pointers may be passed here.
    pub(crate) fn from_raw(ptr: *mut T) -> Self {
        assert!(!ptr.is_null(), "diligent-rs: refusing to wrap a null pointer");
        Self { ptr }
    }

    /// The raw interface pointer (for escape hatches into the C API).
    pub fn as_ptr(&self) -> *mut T {
        self.ptr
    }

    /// Reborrows the interface as a shared reference.
    pub fn as_ref(&self) -> &T {
        // Safety: the pointer is non-null and the engine guarantees the
        // object is alive for as long as the handle exists.
        unsafe { &*self.ptr }
    }

    /// Shares ownership of the same underlying object (COM `IObject::AddRef`).
    ///
    /// Both handles must be dropped; the engine object is destroyed when the
    /// last one goes away. This is the primitive M1b uses to give the
    /// `WgpuWrapper` clone semantics: `Arc`-wrap a `Handle` and clone the
    /// `Handle` with `add_ref` instead of `Clone`.
    pub fn add_ref(&self) -> Handle<T> {
        // Safety: the object is alive (this handle owns a reference), and
        // AddRef is balanced by the Release the returned handle performs on
        // drop.
        unsafe { Self::add_ref_raw(self.ptr) };
        Handle { ptr: self.ptr }
    }

    /// Number of strong references the engine tracks for this object
    /// (`IReferenceCounters::GetNumStrongRefs`), for tests and diagnostics.
    pub fn strong_ref_count(&self) -> i32 {
        let obj = self.ptr.cast::<IObject>();
        // Safety: `ptr` is a live interface; every Vtbl starts with the
        // IObjectMethods block.
        let vtbl = unsafe { &*(*obj).pVtbl };
        let get = vtbl
            .Object
            .GetReferenceCounters
            .expect("diligent-rs: IObject::GetReferenceCounters missing from vtable");
        // Safety: the counters object is owned by the engine and lives as
        // long as the interface does.
        let counters = unsafe { get(obj) };
        assert!(
            !counters.is_null(),
            "diligent-rs: engine returned null reference counters"
        );
        let counters_vtbl = unsafe { &*(*counters).pVtbl };
        let count = counters_vtbl
            .ReferenceCounters
            .GetNumStrongRefs
            .expect("diligent-rs: IReferenceCounters::GetNumStrongRefs missing from vtable");
        // Safety: no arguments beyond the counters object itself.
        unsafe { count(counters) }
    }

    /// Calls the universal `Object::AddRef` vtable slot.
    ///
    /// # Safety
    ///
    /// `ptr` must be a live Diligent interface instance; the added reference
    /// must eventually be released exactly once.
    unsafe fn add_ref_raw(ptr: *mut T) {
        let obj = ptr.cast::<IObject>();
        let vtbl = unsafe { &*(*obj).pVtbl };
        let add_ref = vtbl
            .Object
            .AddRef
            .expect("diligent-rs: IObject::AddRef missing from vtable");
        unsafe { add_ref(obj) };
    }

    /// Calls the universal `Object::Release` vtable slot.
    ///
    /// # Safety
    ///
    /// `ptr` must be a live Diligent interface instance (any `I*` type) and
    /// must not be released twice.
    unsafe fn release(ptr: *mut T) {
        let obj = ptr.cast::<IObject>();
        // Safety: every Diligent Vtbl starts with the IObjectMethods block,
        // so reading `(*obj).pVtbl` as *mut IObjectVtbl and invoking slot
        // `Object.Release` is valid for any interface instance.
        let vtbl = unsafe { &*(*obj).pVtbl };
        let release = vtbl
            .Object
            .Release
            .expect("diligent-rs: IObject::Release missing from vtable");
        unsafe { release(obj) };
    }
}

impl<T> Drop for Handle<T> {
    fn drop(&mut self) {
        // Safety: the handle owns the only reference represented by this
        // wrapper; Release is called exactly once here.
        unsafe { Self::release(self.ptr) };
    }
}

impl<T> Deref for Handle<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.as_ref()
    }
}

/// Non-owning view of an interface borrowed from another object.
///
/// Diligent methods such as `ISwapChain::GetCurrentBackBufferRTV` return a
/// pointer **without** incrementing the reference count; calling `Release`
/// on it is invalid. This wrapper exists so those pointers are still
/// represented safely: no `Drop` impl, `Copy`/`Clone`, and a documented
/// lifetime tied to the owning object.
#[derive(Clone, Copy)]
pub struct NonOwning<T> {
    ptr: *mut T,
}

impl<T> NonOwning<T> {
    pub(crate) fn from_raw_opt(ptr: *mut T) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self { ptr })
        }
    }

    /// The raw interface pointer (for escape hatches into the C API).
    pub fn as_ptr(&self) -> *mut T {
        self.ptr
    }

    /// Reborrows the interface as a shared reference.
    pub fn as_ref(&self) -> &T {
        // Safety: the owner keeps the object alive for the lifetime of this
        // view; the pointer was non-null at creation.
        unsafe { &*self.ptr }
    }
}

/// Converts `&str` into a NUL-terminated C string for Diligent's `*const Char`.
pub(crate) fn cstring(s: &str) -> Result<std::ffi::CString> {
    Ok(std::ffi::CString::new(s)?)
}

/// The engine's default "auto offset/stride" layout sentinel
/// (`LAYOUT_ELEMENT_AUTO_OFFSET` / `LAYOUT_ELEMENT_AUTO_STRIDE` in C).
pub(crate) const LAYOUT_ELEMENT_AUTO: u32 = u32::MAX;

/// Adds COM shared-ownership helpers to a wrapper struct holding a
/// `handle: Handle<T>` field: `add_ref` shares the underlying engine object
/// and `strong_ref_count` reports the engine's reference count. M1b's
/// `WgpuWrapper` clone semantics build on `add_ref`.
macro_rules! impl_shared_ownership {
    ($ty:ty, $raw:ty) => {
        impl $ty {
            /// Shares ownership of the underlying engine object (see
            /// [`Handle::add_ref`]).
            pub fn add_ref(&self) -> crate::handle::Handle<$raw> {
                self.handle.add_ref()
            }

            /// Number of strong references the engine tracks for the
            /// underlying object (see [`Handle::strong_ref_count`]).
            pub fn strong_ref_count(&self) -> i32 {
                self.handle.strong_ref_count()
            }
        }
    };
}
pub(crate) use impl_shared_ownership;
