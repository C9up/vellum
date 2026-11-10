//! Where hayro reads an untrusted document.
//!
//! hayro bounds its recursion by depth (512 nested objects, 50 nested
//! interpretations, 64 nested colour spaces or functions), and how much stack
//! those depths need is fixed by the code, not by the input. The thread the
//! call arrives on is not: a libuv worker, Node's main thread, or a 1 MiB
//! Windows thread. The work runs on a thread of our own whose stack is sized
//! for those depths, so the bounds hold wherever the call comes from.

use std::any::Any;

use hayro::hayro_syntax::budget::Budget;

/// Comfortably above what the depth limits need in a debug build.
const STACK_BYTES: usize = 16 * 1024 * 1024;

/// 256 MiB: a page scanned at 600 DPI decodes to about 100.
pub const DEFAULT_MAX_DECODED_BYTES: u64 = 256 * 1024 * 1024;

/// Ten million operators and glyphs: a dense technical drawing uses well
/// under one.
pub const DEFAULT_MAX_OPERATIONS: u64 = 10_000_000;

/// What reading one page may cost.
///
/// The size of a document says nothing about the work it asks for: a 1 MiB
/// Flate stream inflates to 1 GiB, once per reference to it, and Form
/// XObjects drawing one another ten times per level multiply a 2 KB file into
/// hours. Each page is read under these bounds, and refused past them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimits {
    /// Bytes of decoded stream data: content, images, fonts, object streams.
    pub max_decoded_bytes: u64,
    /// Content operators and glyphs interpreted, nested forms included.
    pub max_operations: u64,
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            max_decoded_bytes: DEFAULT_MAX_DECODED_BYTES,
            max_operations: DEFAULT_MAX_OPERATIONS,
        }
    }
}

/// Run `work` under `limits`, on the reader thread it is called from.
///
/// hayro answers an exhausted budget the way it answers a broken stream: it
/// draws without it. It answers an object, colour space or function nested
/// past its bound the same way, in the page tree and in content alike. Either
/// would hand back a page that silently lacks part of itself, so both are
/// errors here instead.
pub(crate) fn metered<T>(
    limits: &ReadLimits,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let budget = Budget::open(limits.max_decoded_bytes, limits.max_operations);
    let result = work();
    if let Some(what) = budget.too_deep() {
        return Err(format!("the document nests {what}"));
    }
    if budget.decoded_exhausted() {
        return Err(format!(
            "the document decodes to more than {} MiB for one page — raise maxDecodedBytes if you meant it",
            limits.max_decoded_bytes / (1024 * 1024)
        ));
    }
    if budget.operations_exhausted() {
        return Err(format!(
            "drawing the page takes more than {} operations — raise maxOperations if you meant it",
            limits.max_operations
        ));
    }
    result
}

/// Run `work` on a reader thread and wait for it.
///
/// A panic stays an error: it is caught at the join, the same as the binding
/// would catch it on the calling thread.
pub(crate) fn on_reader_thread<T, F>(work: F) -> Result<T, String>
where
    T: Send,
    F: FnOnce() -> Result<T, String> + Send,
{
    std::thread::scope(|scope| {
        let handle = std::thread::Builder::new()
            .name("vellum-reader".to_string())
            .stack_size(STACK_BYTES)
            .spawn_scoped(scope, work)
            .map_err(|error| format!("cannot start a reader thread: {error}"))?;
        handle
            .join()
            .unwrap_or_else(|payload| Err(panic_message(&payload)))
    })
}

fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown cause".to_string());
    format!("internal error while reading the PDF: {detail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_on_the_reader_thread_comes_back_as_an_error() {
        let result: Result<(), String> = on_reader_thread(|| panic!("boom"));
        let message = result.expect_err("the panic should surface as an error");
        assert!(message.contains("boom"), "got {message}");
    }

    #[test]
    fn the_result_is_handed_back() {
        let bytes = [1u8, 2, 3];
        let sum = on_reader_thread(|| Ok(bytes.iter().map(|&b| u32::from(b)).sum::<u32>()));
        assert_eq!(sum, Ok(6));
    }
}
