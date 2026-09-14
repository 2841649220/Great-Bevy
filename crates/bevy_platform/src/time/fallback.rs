//! Provides a fallback implementation of `Instant` from the standard library.

#![expect(
    unsafe_code,
    reason = "Instant fallback requires unsafe to allow users to update the internal value"
)]

use crate::sync::atomic::{AtomicPtr, Ordering};

use core::{
    fmt,
    ops::{Add, AddAssign, Sub, SubAssign},
    time::Duration,
};

static ELAPSED_GETTER: AtomicPtr<()> = AtomicPtr::new(unset_getter as *mut _);

/// Fallback implementation of `Instant` suitable for a `no_std` environment.
///
/// If you are on any of the following target architectures, this is a drop-in replacement:
///
/// - `x86`
/// - `x86_64`
/// - `aarch64`
///
/// On any other architecture, you must call [`Instant::set_elapsed`], providing a method
/// which when called supplies a monotonically increasing count of elapsed nanoseconds relative
/// to some arbitrary point in time.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(Duration);

impl Instant {
    /// Returns an instant corresponding to "now".
    #[must_use]
    pub fn now() -> Instant {
        let getter = ELAPSED_GETTER.load(Ordering::Acquire);

        // A null pointer is never stored (`ELAPSED_GETTER` is initialized with the real
        // `unset_getter` and `Instant::set_elapsed` only accepts a `fn() -> Duration`, which the
        // type system guarantees to be non-null). The check makes the transmute below total:
        // `fn` pointers are non-nullable, so transmuting a null `*mut ()` into one would be
        // immediate undefined behavior rather than merely a crash.
        if getter.is_null() {
            return Self(unset_getter());
        }

        // SAFETY:
        // 1. Calling convention and signature: `getter` is either [`unset_getter`] (stored by the
        //    initializer of [`ELAPSED_GETTER`]) or a value that was passed to
        //    [`Instant::set_elapsed`], which only accepts `fn() -> Duration`. It therefore always
        //    points to executable code using the standard Rust calling convention with exactly that
        //    signature, and the bit patterns of `*mut ()` and `fn() -> Duration` are interchangeable
        //    on every platform Bevy supports (both are pointer-sized).
        // 2. Non-null and valid: the check above rejects the null pointer, and the `Release`/`Acquire
        //    pair used by [`Instant::set_elapsed`] / this load guarantees the stored pointer is
        //    visible with all of its writes.
        // 3. Lifetime: [`Instant::set_elapsed`] is `unsafe` and requires the caller to keep the
        //    function valid for as long as it can be observed here.
        let getter = unsafe { core::mem::transmute::<*mut (), fn() -> Duration>(getter) };

        Self((getter)())
    }

    /// Provides a function returning the amount of time that has elapsed since execution began.
    /// The getter provided to this method will be used by [`now`](Instant::now).
    ///
    /// # Safety
    ///
    /// - The function provided must accurately represent the elapsed time.
    /// - The function must preserve all invariants of the [`Instant`] type.
    /// - The pointer to the function must be valid whenever [`Instant::now`] is called.
    pub unsafe fn set_elapsed(getter: fn() -> Duration) {
        ELAPSED_GETTER.store(getter as *mut _, Ordering::Release);
    }

    /// Returns the amount of time elapsed from another instant to this one,
    /// or zero duration if that instant is later than this one.
    #[must_use]
    pub fn duration_since(&self, earlier: Instant) -> Duration {
        self.saturating_duration_since(earlier)
    }

    /// Returns the amount of time elapsed from another instant to this one,
    /// or None if that instant is later than this one.
    ///
    /// Due to monotonicity bugs, even under correct logical ordering of the passed `Instant`s,
    /// this method can return `None`.
    #[must_use]
    pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> {
        self.0.checked_sub(earlier.0)
    }

    /// Returns the amount of time elapsed from another instant to this one,
    /// or zero duration if that instant is later than this one.
    #[must_use]
    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        self.0.saturating_sub(earlier.0)
    }

    /// Returns the amount of time elapsed since this instant.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        Instant::now().saturating_duration_since(*self)
    }

    /// Returns `Some(t)` where `t` is the time `self + duration` if `t` can be represented as
    /// `Instant` (which means it's inside the bounds of the underlying data structure), `None`
    /// otherwise.
    pub fn checked_add(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_add(duration).map(Instant)
    }

    /// Returns `Some(t)` where `t` is the time `self - duration` if `t` can be represented as
    /// `Instant` (which means it's inside the bounds of the underlying data structure), `None`
    /// otherwise.
    pub fn checked_sub(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_sub(duration).map(Instant)
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;

    /// # Panics
    ///
    /// This function may panic if the resulting point in time cannot be represented by the
    /// underlying data structure. See [`Instant::checked_add`] for a version without panic.
    fn add(self, other: Duration) -> Instant {
        self.checked_add(other)
            .expect("overflow when adding duration to instant")
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, other: Duration) {
        *self = *self + other;
    }
}

impl Sub<Duration> for Instant {
    type Output = Instant;

    fn sub(self, other: Duration) -> Instant {
        self.checked_sub(other)
            .expect("overflow when subtracting duration from instant")
    }
}

impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, other: Duration) {
        *self = *self - other;
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;

    /// Returns the amount of time elapsed from another instant to this one,
    /// or zero duration if that instant is later than this one.
    fn sub(self, other: Instant) -> Duration {
        self.duration_since(other)
    }
}

impl fmt::Debug for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

fn unset_getter() -> Duration {
    crate::cfg::switch! {
        #[cfg(target_arch = "x86")] => {
            // SAFETY:
            // 1. `_rdtsc` (Read Time-Stamp Counter) is an x86 CPU intrinsic reading the 64-bi
            //    cycle count into register pair EDX:EAX. It does not access or dereference memory.
            // 2. The intrinsic is executable from user-mode (ring 3) on modern x86 processors
            //    unless CR4.TSD is explicitly set by the kernel, which standard OS environments
            //    leave cleared for high-resolution timing.
            // 3. No preconditions or invariants can be violated by reading this register; it has
            //    no side-effects on CPU register state beyond the returned cycle count.
            let nanos = unsafe {
                core::arch::x86::_rdtsc()
            };
            Duration::from_nanos(nanos)
        }
        #[cfg(target_arch = "x86_64")] => {
            // SAFETY:
            // 1. `_rdtsc` (Read Time-Stamp Counter) is an x86_64 CPU intrinsic reading the 64-bi
            //    cycle count into RDX:RAX. It does not access or dereference memory.
            // 2. The intrinsic is executable from user-mode (ring 3) on x86_64 processors
            //    unless CR4.TSD is explicitly set by the kernel, which standard OS environments
            //    leave cleared for high-resolution timing.
            // 3. No preconditions or invariants can be violated by reading this register; it has
            //    no side-effects on CPU register state beyond the returned cycle count.
            let nanos = unsafe {
                core::arch::x86_64::_rdtsc()
            };
            Duration::from_nanos(nanos)
        }
        #[cfg(target_arch = "aarch64")] => {
            // SAFETY:
            // 1. `cntvct_el0` is the standard ARMv8-A Virtual Counter register, accessible at EL0 (user space)
            //    when enabled by EL1 (the OS kernel via CNTHCTL_EL2/CNTKCTL_EL1, which standard OS environments
            //    like Linux, macOS, and Windows always configure).
            // 2. The inline assembly instruction `mrs {}, cntvct_el0` only writes to an allocated 64-bi
            //    output register (`out(reg) ticks`) without modifying any memory or system flags.
            // 3. No preconditions on memory or pointer validity are required; the instruction has no memory side-effects.
            let nanos = unsafe {
                let mut ticks: u64;
                core::arch::asm!("mrs {}, cntvct_el0", out(reg) ticks);
                ticks
            };
            Duration::from_nanos(nanos)
        }
        _ => {
            panic!("An elapsed time getter has not been provided to `Instant`. Please use `Instant::set_elapsed(...)` before calling `Instant::now()`")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::atomic::AtomicU64;

    #[test]
    fn test_fallback_instant_now() {
        let t1 = Instant::now();
        let t2 = Instant::now();
        assert!(t2 >= t1);
        let diff = t2 - t1;
        assert_eq!(t2.duration_since(t1), diff);
        assert_eq!(t1.duration_since(t2), Duration::ZERO);
        assert_eq!(t1.saturating_duration_since(t2), Duration::ZERO);
        let elapsed = t1.elapsed();
        assert!(elapsed >= Duration::ZERO);
    }

    #[test]
    fn test_fallback_instant_set_elapsed() {
        static COUNTER: AtomicU64 = AtomicU64::new(100_000);

        fn mock_getter() -> Duration {
            Duration::from_nanos(COUNTER.fetch_add(50_000, Ordering::Relaxed))
        }

        // SAFETY: `mock_getter` is a valid fn pointer with standard Rust ABI matching
        // `fn() -> Duration`. It returns monotonically increasing nanoseconds and is valid
        // for the lifetime of the process.
        unsafe {
            Instant::set_elapsed(mock_getter);
        }

        let t1 = Instant::now();
        let t2 = Instant::now();
        assert!(t2 > t1);
        assert_eq!(t2.duration_since(t1), Duration::from_nanos(50_000));
        assert_eq!(
            t2.checked_duration_since(t1),
            Some(Duration::from_nanos(50_000))
        );
        assert_eq!(t1.checked_duration_since(t2), None);
    }
}
