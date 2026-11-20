//! vellum: a per-thread budget on what loading one document may build.
//!
//! `LoadOptions::max_decompressed_size` bounds the inflated bytes of ONE
//! object or cross-reference stream, and nothing else. A document may carry
//! as many such streams as it likes, each inflated and kept, and what the
//! parser builds from them is not their size: 32 MiB of `0 ` is sixteen
//! million `Object::Integer` in one array, two gigabytes from a 32 KB file.
//!
//! A caller loading an untrusted document opens a [`LoadBudget`] on its
//! thread. Every object or cross-reference stream inflated charges its bytes
//! against one total, and every object the parser builds — an array element,
//! a dictionary value, a cross-reference entry — charges one against a count.
//! Once either runs out, decoding and parsing refuse, and the budget says why.
//! Without one, nothing is limited, as upstream.
//!
//! The counters are thread-local: build this crate without its `rayon`
//! feature, or the objects are parsed on threads the budget does not see.

use std::cell::Cell;
use std::marker::PhantomData;

#[derive(Clone, Copy)]
struct State {
    inflated_left: usize,
    objects_left: u64,
    objects_exhausted: bool,
}

thread_local! {
    static STATE: Cell<Option<State>> = const { Cell::new(None) };
}

/// Limits charged by loading on this thread while the guard lives.
pub struct LoadBudget {
    // Bound to the thread whose counters it set.
    _not_send: PhantomData<*const ()>,
}

impl LoadBudget {
    /// Allow at most `inflated_bytes` of object and cross-reference stream
    /// content in all, and `objects` parsed objects, on this thread.
    pub fn open(inflated_bytes: usize, objects: u64) -> Self {
        STATE.with(|state| {
            state.set(Some(State {
                inflated_left: inflated_bytes,
                objects_left: objects,
                objects_exhausted: false,
            }))
        });
        Self { _not_send: PhantomData }
    }

    /// Whether parsing was refused because the object count ran out.
    pub fn objects_exhausted(&self) -> bool {
        STATE.with(|state| state.get().is_some_and(|state| state.objects_exhausted))
    }
}

impl Drop for LoadBudget {
    fn drop(&mut self) {
        STATE.with(|state| state.set(None));
    }
}

/// Charge one parsed object; `false` once the count is spent.
pub(crate) fn charge_object() -> bool {
    STATE.with(|cell| {
        let Some(mut state) = cell.get() else {
            return true;
        };
        if state.objects_left == 0 {
            state.objects_exhausted = true;
            cell.set(Some(state));
            return false;
        }
        state.objects_left -= 1;
        cell.set(Some(state));
        true
    })
}

/// The most one stream may inflate to now: the caller's per-stream limit,
/// lowered to what is left of the total.
pub(crate) fn inflate_allowance(per_stream: Option<usize>) -> Option<usize> {
    STATE.with(|cell| match (cell.get(), per_stream) {
        (None, limit) => limit,
        (Some(state), None) => Some(state.inflated_left),
        (Some(state), Some(limit)) => Some(limit.min(state.inflated_left)),
    })
}

/// Charge `bytes` of inflated content against the total.
pub(crate) fn charge_inflated(bytes: usize) {
    STATE.with(|cell| {
        if let Some(mut state) = cell.get() {
            state.inflated_left = state.inflated_left.saturating_sub(bytes);
            cell.set(Some(state));
        }
    });
}

/// A stream inflated past its allowance: it was decoded up to the allowance
/// before being refused, so the allowance is spent, and the next stream is
/// not inflated at all. Without this, every stream just past the limit would
/// cost the limit again in work.
pub(crate) fn spend_inflation() {
    STATE.with(|cell| {
        if let Some(mut state) = cell.get() {
            state.inflated_left = 0;
            cell.set(Some(state));
        }
    });
}
