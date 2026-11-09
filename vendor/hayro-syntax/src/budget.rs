//! vellum: per-thread work budgets.
//!
//! Nothing bounded what reading a document cost. A 1 MiB Flate stream
//! inflated to 1 GiB, once per reference to it, and an allocation failure
//! aborts the process rather than returning `None`; a fan-out of Form
//! XObjects drawing one another ten times per level turned a 2 KB file into
//! hours of interpretation. A caller that reads untrusted documents opens a
//! [`Budget`] on its thread; decoding and interpreting charge it, and stop
//! once it runs out. Without one, nothing is limited, as upstream.

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
    }

    pub(super) fn close() {
        DECODED.with(|meter| meter.set(UNLIMITED));
        OPERATIONS.with(|meter| meter.set(UNLIMITED));
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
