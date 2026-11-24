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

/// 1 GiB of decoded stream data across every page of one `render_all` or
/// `extract_text_all`.
pub const DEFAULT_MAX_TOTAL_DECODED_BYTES: u64 = 1024 * 1024 * 1024;

/// Ten million operators and glyphs across every page of one `render_all` or
/// `extract_text_all`: what one page may already take, and a few thousand
/// pages of ordinary text.
pub const DEFAULT_MAX_TOTAL_OPERATIONS: u64 = 10_000_000;

/// What reading one page may cost, and what reading all of them may.
///
/// The size of a document says nothing about the work it asks for: a 1 MiB
/// Flate stream inflates to 1 GiB, once per reference to it, and Form
/// XObjects drawing one another ten times per level multiply a 2 KB file into
/// hours. Each page is read under these bounds, and refused past them.
///
/// The page count is the document's to choose, so the calls that read every
/// page also carry the totals across them: twenty tiny pages each just under
/// the per-page bound were a minute of work from a 24 KB file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimits {
    /// Bytes of decoded stream data: content, images, fonts, object streams.
    /// Rendering charges it too with what drawing the page keeps until it
    /// is done: a page-sized mask per soft mask, an RGBA copy per image
    /// draw, pattern tiles and cached glyphs.
    pub max_decoded_bytes: u64,
    /// Content operators and glyphs interpreted, nested forms included.
    pub max_operations: u64,
    /// Bytes of decoded stream data across every page, for the calls that
    /// read them all.
    pub max_total_decoded_bytes: u64,
    /// Operators and glyphs across every page, for the calls that read them
    /// all.
    pub max_total_operations: u64,
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            max_decoded_bytes: DEFAULT_MAX_DECODED_BYTES,
            max_operations: DEFAULT_MAX_OPERATIONS,
            max_total_decoded_bytes: DEFAULT_MAX_TOTAL_DECODED_BYTES,
            max_total_operations: DEFAULT_MAX_TOTAL_OPERATIONS,
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
    Totals::single(limits).metered(work)
}

/// What a call reading every page has left of its totals, carried from one
/// page's budget to the next.
pub(crate) struct Totals {
    limits: ReadLimits,
    decoded_left: u64,
    operations_left: u64,
    /// Whether the totals apply: a call reading one page has only the
    /// per-page bounds.
    across_pages: bool,
}

impl Totals {
    /// The totals of a call that reads every page.
    pub(crate) fn across_pages(limits: &ReadLimits) -> Self {
        Self {
            limits: *limits,
            decoded_left: limits.max_total_decoded_bytes,
            operations_left: limits.max_total_operations,
            across_pages: true,
        }
    }

    fn single(limits: &ReadLimits) -> Self {
        Self {
            limits: *limits,
            decoded_left: u64::MAX,
            operations_left: u64::MAX,
            across_pages: false,
        }
    }

    /// Run `work` under the per-page bounds, lowered to what is left of the
    /// totals, and charge the totals with what it used.
    pub(crate) fn metered<T>(
        &mut self,
        work: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let decoded = self.limits.max_decoded_bytes.min(self.decoded_left);
        let operations = self.limits.max_operations.min(self.operations_left);
        let budget = Budget::open(decoded, operations);
        let result = work();
        self.decoded_left = self
            .decoded_left
            .saturating_sub(decoded.saturating_sub(budget.decoded_left()));
        self.operations_left = self
            .operations_left
            .saturating_sub(operations.saturating_sub(budget.operations_left()));
        if let Some(what) = budget.too_deep() {
            return Err(format!("the document nests {what}"));
        }
        if budget.decoded_exhausted() {
            if self.across_pages && decoded < self.limits.max_decoded_bytes {
                return Err(format!(
                    "the document decodes to more than {} MiB across its pages — read them one at a time, or raise maxTotalDecodedBytes if you meant it",
                    self.limits.max_total_decoded_bytes / (1024 * 1024)
                ));
            }
            return Err(format!(
                "the page decodes to, or keeps drawing buffers of, more than {} MiB — raise maxDecodedBytes if you meant it",
                self.limits.max_decoded_bytes / (1024 * 1024)
            ));
        }
        if budget.operations_exhausted() {
            if self.across_pages && operations < self.limits.max_operations {
                return Err(format!(
                    "reading every page takes more than {} operations — read them one at a time, or raise maxTotalOperations if you meant it",
                    self.limits.max_total_operations
                ));
            }
            return Err(format!(
                "drawing the page takes more than {} operations — raise maxOperations if you meant it",
                self.limits.max_operations
            ));
        }
        result
    }
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
