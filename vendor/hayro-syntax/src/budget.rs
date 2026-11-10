//! vellum: per-thread work budgets.
//!
//! Nothing bounded what reading a document cost. A 1 MiB Flate stream
//! inflated to 1 GiB, once per reference to it, and an allocation failure
//! aborts the process rather than returning `None`; a fan-out of Form
//! XObjects drawing one another ten times per level turned a 2 KB file into
//! hours of interpretation. A caller that reads untrusted documents opens a
//! [`Budget`] on its thread; decoding and interpreting charge it, and stop
//! once it runs out. Without one, nothing is limited, as upstream.
//!
//! Nesting past a bound is answered the way garbage is: the object, colour
//! space or function is skipped, and the page is drawn or the page tree read
//! without it. The budget remembers what nested too deep, so a caller can
//! refuse the document instead of handing back a page that silently lacks
//! part of itself.

/// Limits charged by the work done on this thread while the guard lives.
pub struct Budget {
    // Bound to the thread whose counters it set.
    _not_send: core::marker::PhantomData<*const ()>,
}

impl Budget {
    /// Allow at most `decoded_bytes` of decoded stream data and
    /// `operations` content operators and glyphs on this thread.
    pub fn open(decoded_bytes: u64, operations: u64) -> Self {
        imp::open(decoded_bytes, operations);
        Self {
            _not_send: core::marker::PhantomData,
        }
    }

    /// Whether decoding was refused because the byte budget ran out.
    pub fn decoded_exhausted(&self) -> bool {
        imp::decoded_exhausted()
    }

    /// Whether interpreting stopped because the operation budget ran out.
    pub fn operations_exhausted(&self) -> bool {
        imp::operations_exhausted()
    }

    /// What was skipped first for nesting past its bound, if anything was:
    /// a phrase such as "arrays or dictionaries more than 512 levels deep".
    pub fn too_deep(&self) -> Option<&'static str> {
        imp::too_deep()
    }
}

impl Drop for Budget {
    fn drop(&mut self) {
        imp::close();
    }
}

/// How many more decoded bytes this thread may produce, if it is limited.
pub(crate) fn decode_allowance() -> Option<usize> {
    imp::decode_allowance()
}

/// Charge `bytes` of decoded data; `false` once the budget is exceeded.
pub(crate) fn charge_decoded(bytes: usize) -> bool {
    imp::charge_decoded(bytes)
}

/// Record that `what` was skipped for nesting past its bound; the first
/// record is the one reported.
#[doc(hidden)]
pub fn note_too_deep(what: &'static str) {
    imp::note_too_deep(what)
}

/// Charge one operation; `false` once the budget is exceeded.
#[doc(hidden)]
pub fn charge_operation() -> bool {
    imp::charge_operation()
}

#[cfg(feature = "std")]
mod imp {
    use std::cell::Cell;

    #[derive(Clone, Copy)]
    struct Meter {
        remaining: Option<u64>,
        exhausted: bool,
    }

    const UNLIMITED: Meter = Meter {
        remaining: None,
        exhausted: false,
    };

    thread_local! {
        static DECODED: Cell<Meter> = const { Cell::new(UNLIMITED) };
        static OPERATIONS: Cell<Meter> = const { Cell::new(UNLIMITED) };
        static TOO_DEEP: Cell<Option<&'static str>> = const { Cell::new(None) };
    }

    fn charge(meter: &Cell<Meter>, amount: u64) -> bool {
        let mut current = meter.get();
        let Some(remaining) = current.remaining else {
            return true;
        };
        if current.exhausted || amount > remaining {
            current.exhausted = true;
            current.remaining = Some(0);
            meter.set(current);
            return false;
        }
        current.remaining = Some(remaining - amount);
        meter.set(current);
        true
    }

    pub(super) fn open(decoded_bytes: u64, operations: u64) {
        DECODED.with(|meter| {
            meter.set(Meter {
                remaining: Some(decoded_bytes),
                exhausted: false,
            })
        });
        OPERATIONS.with(|meter| {
            meter.set(Meter {
                remaining: Some(operations),
                exhausted: false,
            })
        });
        TOO_DEEP.with(|first| first.set(None));
    }

    pub(super) fn close() {
        DECODED.with(|meter| meter.set(UNLIMITED));
        OPERATIONS.with(|meter| meter.set(UNLIMITED));
        TOO_DEEP.with(|first| first.set(None));
    }

    pub(super) fn too_deep() -> Option<&'static str> {
        TOO_DEEP.with(Cell::get)
    }

    pub(super) fn note_too_deep(what: &'static str) {
        TOO_DEEP.with(|first| {
            if first.get().is_none() {
                first.set(Some(what));
            }
        });
    }

    pub(super) fn decoded_exhausted() -> bool {
        DECODED.with(|meter| meter.get().exhausted)
    }

    pub(super) fn operations_exhausted() -> bool {
        OPERATIONS.with(|meter| meter.get().exhausted)
    }

    pub(super) fn decode_allowance() -> Option<usize> {
        DECODED.with(|meter| {
            meter
                .get()
                .remaining
                .map(|remaining| usize::try_from(remaining).unwrap_or(usize::MAX))
        })
    }

    pub(super) fn charge_decoded(bytes: usize) -> bool {
        DECODED.with(|meter| charge(meter, u64::try_from(bytes).unwrap_or(u64::MAX)))
    }

    pub(super) fn charge_operation() -> bool {
        OPERATIONS.with(|meter| charge(meter, 1))
    }
}

#[cfg(not(feature = "std"))]
mod imp {
    pub(super) fn open(_: u64, _: u64) {}
    pub(super) fn close() {}
    pub(super) fn decoded_exhausted() -> bool {
        false
    }
    pub(super) fn operations_exhausted() -> bool {
        false
    }
    pub(super) fn too_deep() -> Option<&'static str> {
        None
    }
    pub(super) fn note_too_deep(_: &'static str) {}
    pub(super) fn decode_allowance() -> Option<usize> {
        None
    }
    pub(super) fn charge_decoded(_: usize) -> bool {
        true
    }
    pub(super) fn charge_operation() -> bool {
        true
    }
}
